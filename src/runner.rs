use core::{
    cmp::min,
    future::{self},
    pin::Pin,
    sync::atomic::Ordering,
    task::Poll,
    time::Duration,
};

use alloc::{boxed::Box, collections::BTreeMap, sync::Arc, vec::Vec};
use arbitrary_int::{traits::Integer, u4, u7, u11, u15, u26, u31};
use debug_ignore::DebugIgnore;
use futures::FutureExt;
use volatile::{VolatilePtr, VolatileRef, access::ReadOnly};
use zerocopy::transmute;

use crate::{
    CtrlDsSegment, PeriodicFrameList, Qh,
    address_allocator::AddressAllocator,
    async_list_manager::{AsyncListManager, QhWithMetaA, QhWithMetaAVolatileFieldAccess},
    buffer_ptrs::BufferPtrs,
    capability_regs::{CapabilityRegs, CapabilityRegsVolatileFieldAccess},
    delay::Delay,
    device::Device,
    device_shared::{DeviceReq, DeviceSharedState},
    ehci_control_common::{ControlReq, ControlSharedState},
    endpoint_speed::EndpointSpeed,
    error::Error,
    interrupt_endpoint_shared::InterruptEndpointSharedState,
    irq_handler::IrqHandlerSharedState,
    oneshot::{OneshotReceiver, oneshot},
    operational_regs::{
        AsyncListAddrReg, ConfigFlagReg, OperationalRegs, OperationalRegsVolatileFieldAccess,
        PortScReg, UsbIntrReg, UsbStsReg,
    },
    periodic_list_manager::PeriodicListManager,
    qtd::{NextQtdPointer, Qtd, QtdVolatileFieldAccess},
    queue_head::{EndpointCapabilities, EndpointCharacteristics, QhHorizontalLinkPtr, SelectType},
    root_port_number::RootPortNumber,
    root_port_shared::{ResetDeviceResources, RootPortSharedState},
    setup_packet::SetupPacket,
    start_config::StartConfig,
    transfer_token::{PidCode, TransferToken},
    util::get_addr_offset,
};

enum ResetDeviceStep {
    WaitingToStopReset(Pin<Box<dyn Future<Output = ()>>>),
    WaitForPortToBeEnabled(Pin<Box<dyn Future<Output = ()>>>),
    WaitForSetAddr {
        addr: u7,
    },
    WaitForQhRemove {
        addr: u7,
        qh_manager_rx: OneshotReceiver<()>,
    },
}

struct ResettingDevice {
    req: ResetDeviceResources,
    root_port: RootPortNumber,
    step: ResetDeviceStep,
}

enum RunnerState {
    /// initially and when a host system error occurs
    Stopped { error: Option<Error> },
    /// wrote stop to run/stop, waiting for halted bit to be set
    Stopping {
        start_time: Duration,
        start_config: StartConfig,
    },
    /// wrote 1 to HCRESET, waiting for it to change to 0
    Resetting {
        start_time: Duration,
        start_config: StartConfig,
    },
    /// host controller is initialized and running with no known host system error
    Running {
        resetting_device: Option<ResettingDevice>,
        addr_allocator: AddressAllocator,
        ctrl_ds_segment: CtrlDsSegment,
        qh_manager: AsyncListManager,
        devices: BTreeMap<RootPortNumber, Arc<DeviceSharedState>>,
        endpoints: Vec<Arc<InterruptEndpointSharedState>>,
        periodic_list_manager: PeriodicListManager,
    },
}

pub struct Runner {
    state: RunnerState,
    delay: Box<dyn Delay>,
    control_shared_state: Arc<ControlSharedState>,
    capability_regs: VolatileRef<'static, CapabilityRegs, ReadOnly>,
    operational_regs: VolatileRef<'static, OperationalRegs>,
    root_ports_shared: Vec<Arc<RootPortSharedState>>,
    irq_handler_shared: Arc<IrqHandlerSharedState>,
    port_sc_regs: VolatileRef<'static, [PortScReg]>,
}

impl Runner {
    pub(crate) fn new(
        delay: Box<dyn Delay>,
        control_shared_state: Arc<ControlSharedState>,
        capability_regs: VolatileRef<'static, CapabilityRegs, ReadOnly>,
        operational_regs: VolatileRef<'static, OperationalRegs>,
        root_ports_shared: Vec<Arc<RootPortSharedState>>,
        irq_handler_shared: Arc<IrqHandlerSharedState>,
        port_sc_regs: VolatileRef<'static, [PortScReg]>,
    ) -> Self {
        Self {
            state: RunnerState::Stopped { error: None },
            delay,
            control_shared_state,
            capability_regs,
            operational_regs,
            root_ports_shared,
            irq_handler_shared,
            port_sc_regs,
        }
    }

    pub async fn run(&mut self) {
        loop {
            match &mut self.state {
                RunnerState::Stopped { error: _ } => {
                    match self.control_shared_state.req_signal.wait().await {
                        ControlReq::Start(config) => {
                            // Stop the eHCI in case it's actually running bit in a broken state
                            self.operational_regs
                                .as_mut_ptr()
                                .usb_cmd()
                                .update(|usb_cmd| usb_cmd.with_rs(false));
                            self.state = RunnerState::Stopping {
                                start_time: self.delay.now(),
                                start_config: config,
                            };
                        }
                    }
                }
                RunnerState::Stopping {
                    start_time,
                    start_config,
                } => {
                    if self
                        .operational_regs
                        .as_mut_ptr()
                        .usb_sts()
                        .read()
                        .hc_halted()
                    {
                        self.operational_regs
                            .as_mut_ptr()
                            .usb_cmd()
                            .update(|usb_cmd| usb_cmd.with_hc_reset(true));
                        self.state = RunnerState::Resetting {
                            start_time: self.delay.now(),
                            start_config: *start_config,
                        };
                    } else {
                        let now = self.delay.now();
                        if now - *start_time >= start_config.stop_strategy.timeout {
                            let error = Error::StopTimeout;
                            self.state = RunnerState::Stopped { error: Some(error) };
                            self.control_shared_state.res_signal.signal(Err(error));
                        } else {
                            self.delay
                                .delay_until(min(
                                    now + start_config.stop_strategy.check_interval,
                                    *start_time + start_config.stop_strategy.timeout,
                                ))
                                .await;
                        }
                    }
                }
                RunnerState::Resetting {
                    start_time,
                    start_config,
                } => {
                    let operational_regs = self.operational_regs.as_mut_ptr();
                    let usb_cmd_ptr = operational_regs.usb_cmd();
                    let usb_cmd = usb_cmd_ptr.read();
                    if !usb_cmd.hc_reset() {
                        // Initialize CTRLDSSEGMENT
                        let ctrl_ds_segment = start_config.ctrl_ds_segment;
                        if ctrl_ds_segment.base_addr_upper != 0
                            && !self
                                .capability_regs
                                .as_ptr()
                                .hcc_params()
                                .read()
                                ._64_bit_addressing_cap()
                        {
                            panic!(
                                "attempted to use non-zero CTRLDSSEGMENT with 64-bit addressing is not supported"
                            )
                        }
                        operational_regs
                            .ctrl_ds_segment()
                            .write(ctrl_ds_segment.base_addr_upper);
                        // Clear pending interrupts
                        operational_regs.usb_sts().write(
                            UsbStsReg::new_with_raw_value(0)
                                .with_usb_int(true)
                                .with_usb_err_int(true)
                                .with_port_change_detect(true)
                                .with_frame_list_rollover(true)
                                .with_host_system_error(true)
                                .with_interrupt_on_async_advance(true),
                        );
                        // Initialize USBINTR
                        operational_regs.usb_intr().write(
                            UsbIntrReg::builder()
                                .with_usb_interrupt_enable(true)
                                .with_usb_error_interrupt_enable(true)
                                .with_port_change_interrupt_enable(true)
                                .with_frame_list_rollover_interrupt_enable(false)
                                .with_host_system_error_interrupt_enable(true)
                                .with_interrupt_on_async_advance_enable(true)
                                .with_reserved(u26::ZERO)
                                .build(),
                        );
                        // Initialize the periodic frame list
                        let mut periodic_frame_list =
                            unsafe { VolatileRef::new(start_config.periodic_frame_list_mem.ptr) };
                        periodic_frame_list
                            .as_mut_ptr()
                            .write(PeriodicFrameList::EMPTY);
                        // Initialize PERIODICLIST BASE
                        operational_regs.periodic_list_base().write(get_addr_offset(
                            start_config.periodic_frame_list_mem.phys_addr,
                            ctrl_ds_segment,
                        ));
                        // Write to USBCMD to turn the host controller on
                        operational_regs.usb_cmd().write(usb_cmd.with_rs(true));
                        // Write a 1 to CONFIGFLAG register to route all ports to the EHCI controller (see Section 4.2).
                        operational_regs.config_flag().write(
                            ConfigFlagReg::builder()
                                .with_configure_flag(true)
                                .with_reserved(u31::ZERO)
                                .build(),
                        );
                        // Start the periodic list
                        operational_regs
                            .usb_cmd()
                            .update(|usb_cmd| usb_cmd.with_periodic_schedule_enable(true));
                        // Initialize the async circular linked list
                        let qh_addr_offset =
                            get_addr_offset(start_config.qh.phys_addr, ctrl_ds_segment);
                        let qh_ptr = unsafe { VolatilePtr::new(start_config.qh.ptr) };
                        qh_ptr.write(QhWithMetaA {
                            qh: {
                                let mut qh = Qh::new(
                                    EndpointCharacteristics::ZERO
                                        .with_head_of_reclamation_list_flag(true),
                                    EndpointCapabilities::ZERO,
                                );
                                qh.horizontal_link_ptr =
                                    QhHorizontalLinkPtr::new(SelectType::Qh, qh_addr_offset);
                                qh.transfer_token.set_halted(true);
                                qh
                            },
                            next: qh_ptr,
                            prev: qh_ptr,
                        });
                        // Initialize ASYNCLISTADDR
                        operational_regs
                            .async_list_addr()
                            .write(AsyncListAddrReg::new(qh_addr_offset));
                        // Enable the async list
                        operational_regs
                            .usb_cmd()
                            .update(|usb_cmd| usb_cmd.with_async_schedule_enable(true));
                        self.state = RunnerState::Running {
                            resetting_device: None,
                            addr_allocator: AddressAllocator::default(),
                            ctrl_ds_segment,
                            qh_manager: AsyncListManager::new(start_config.qh, ctrl_ds_segment),
                            devices: Default::default(),
                            periodic_list_manager: PeriodicListManager::new(
                                periodic_frame_list,
                                ctrl_ds_segment,
                            ),
                            endpoints: Default::default(),
                        };
                        self.control_shared_state.res_signal.signal(Ok(()));
                    } else {
                        let now = self.delay.now();
                        if now - *start_time >= start_config.reset_strategy.timeout {
                            let error = Error::ResetTimeout;
                            self.state = RunnerState::Stopped { error: Some(error) };
                            self.control_shared_state.res_signal.signal(Err(error));
                        } else {
                            self.delay
                                .delay_until(min(
                                    now + start_config.reset_strategy.check_interval,
                                    *start_time + start_config.reset_strategy.timeout,
                                ))
                                .await;
                        }
                    }
                }
                RunnerState::Running {
                    resetting_device: resetting_device_option,
                    addr_allocator,
                    ctrl_ds_segment,
                    qh_manager,
                    devices,
                    periodic_list_manager,
                    endpoints,
                } => {
                    future::poll_fn(|cx| {
                        self.irq_handler_shared.runner_waker.register(cx.waker());

                        let usb_int = self
                            .irq_handler_shared
                            .new_usb_int
                            .swap(false, Ordering::Relaxed);
                        let usb_err_int = self
                            .irq_handler_shared
                            .new_usb_err_int
                            .swap(false, Ordering::Relaxed);
                        if usb_int || usb_err_int {
                            for device in devices.values_mut() {
                                device.usb_int_signal.signal(());
                            }
                            for endpoint in endpoints.iter_mut() {
                                endpoint.int_signal.signal(());
                            }
                        }

                        let async_advance = self
                            .irq_handler_shared
                            .new_async_advance
                            .swap(false, Ordering::Relaxed);
                        if async_advance {
                            qh_manager.handle_async_advance(unsafe {
                                VolatileRef::new(
                                    self.operational_regs.as_mut_ptr().usb_cmd().as_raw_ptr(),
                                )
                            });
                        }

                        let new_port_change_detect = self
                            .irq_handler_shared
                            .new_port_change_detect
                            .swap(false, Ordering::Relaxed);
                        if new_port_change_detect {
                            for root_port in &self.root_ports_shared {
                                root_port.port_changed_signal.signal(());
                            }
                        }

                        for device in devices.values_mut() {
                            if let Poll::Ready(req) = device.req_signal.poll_wait(cx) {
                                match req {
                                    DeviceReq::AddQh(req) => {
                                        qh_manager.add_qh_to_async_list(req.add_qh);
                                        req.callback.send(());
                                    }
                                    DeviceReq::AddEndpoint(req) => {
                                        endpoints.push(req.endpoint_shared);
                                    }
                                }
                            }
                        }

                        for endpoint in endpoints.iter_mut() {
                            if let Poll::Ready(req) = endpoint.req_signal.poll_wait(cx) {
                                periodic_list_manager.add_qh(req.qh, req.interval);
                                req.callback.send(());
                            }
                        }

                        'reset: loop {
                            if resetting_device_option.is_none() {
                                for (i, root_port) in self.root_ports_shared.iter().enumerate() {
                                    let poll = root_port.req_signal.poll_wait(cx);
                                    if let Poll::Ready(req) = poll {
                                        self.port_sc_regs.as_mut_ptr().index(i).update(|port_sc| {
                                            port_sc
                                                .with_port_reset(true)
                                                .without_write_to_clear_bits()
                                        });
                                        *resetting_device_option = Some(ResettingDevice {
                                            req,
                                            root_port: u4::new(i.try_into().unwrap())
                                                .try_into()
                                                .unwrap(),
                                            step: ResetDeviceStep::WaitingToStopReset(
                                                self.delay.delay_until(
                                                    self.delay.now() + Duration::from_millis(50),
                                                ),
                                            ),
                                        });
                                        continue 'reset;
                                    }
                                }
                                break;
                            }
                            if let Some(resetting_device) = resetting_device_option {
                                let root_port_index =
                                    u4::from(resetting_device.root_port).as_usize();
                                let port_sc_ptr =
                                    self.port_sc_regs.as_mut_ptr().index(root_port_index);
                                match &mut resetting_device.step {
                                    ResetDeviceStep::WaitingToStopReset(f) => {
                                        let port_sc = port_sc_ptr.read();
                                        if !port_sc.current_connect_status() {
                                            // Got disconnected already
                                            self.root_ports_shared[root_port_index]
                                                .res_signal
                                                .signal(Err(Error::DeviceDisconnected));
                                            *resetting_device_option = None;
                                        } else if f.poll_unpin(cx).is_ready() {
                                            port_sc_ptr.update(|port_sc| {
                                                port_sc
                                                    .with_port_reset(false)
                                                    .without_write_to_clear_bits()
                                            });
                                            resetting_device.step =
                                                ResetDeviceStep::WaitForPortToBeEnabled(
                                                    self.delay.delay_until(
                                                        self.delay.now() + Duration::from_millis(2),
                                                    ),
                                                )
                                        } else {
                                            break;
                                        }
                                    }
                                    ResetDeviceStep::WaitForPortToBeEnabled(f) => {
                                        let port_sc = port_sc_ptr.read();
                                        if !port_sc.current_connect_status() {
                                            // Got disconnected already
                                            self.root_ports_shared[root_port_index]
                                                .res_signal
                                                .signal(Err(Error::DeviceDisconnected));
                                            *resetting_device_option = None;
                                        } else if port_sc.port_enabled() {
                                            let qh_ptr = unsafe {
                                                VolatilePtr::new(resetting_device.req.qh.ptr)
                                            };
                                            let qtd_ptrs = resetting_device
                                                .req
                                                .qtds
                                                .map(|qtd| unsafe { VolatilePtr::new(qtd.ptr) });
                                            let buffer_ptr = unsafe {
                                                VolatilePtr::new(resetting_device.req.buffer.ptr)
                                            };
                                            let addr_to_set =
                                                addr_allocator.allocate_addr().unwrap();
                                            buffer_ptr.write(transmute!(
                                                SetupPacket::new_set_address(addr_to_set)
                                            ));
                                            let mut qtd_0 = Qtd::new(
                                                TransferToken::new_active(
                                                    PidCode::SetupToken,
                                                    u15::new(8),
                                                )
                                                .with_interrupt_on_complete(true),
                                                BufferPtrs::new_contiguous(
                                                    resetting_device.req.buffer.phys_addr,
                                                    8,
                                                ),
                                            );
                                            qtd_0.next_qtd_ptr =
                                                NextQtdPointer::new_valid(get_addr_offset(
                                                    resetting_device.req.qtds[1].phys_addr,
                                                    *ctrl_ds_segment,
                                                ));
                                            qtd_ptrs[0].write(qtd_0);
                                            let qtd_1 = Qtd::new(
                                                TransferToken::new_active(
                                                    PidCode::InToken,
                                                    u15::ZERO,
                                                )
                                                .with_interrupt_on_complete(true)
                                                .with_data_toggle(true),
                                                BufferPtrs::ZERO,
                                            );
                                            qtd_ptrs[1].write(qtd_1);
                                            let mut qh = Qh::new(
                                                EndpointCharacteristics::new_with_raw_value(0)
                                                    .with_device_addr({
                                                        // new devices have address 0
                                                        u7::ZERO
                                                    })
                                                    .with_endpoint_number({
                                                        // control endpoint
                                                        u4::ZERO
                                                    })
                                                    .with_endpoint_speed(EndpointSpeed::High.into())
                                                    .with_max_packet_len(u11::new(8)),
                                                EndpointCapabilities::new_with_raw_value(0),
                                            );
                                            qh.next_qtd_pointer =
                                                NextQtdPointer::new_valid(get_addr_offset(
                                                    resetting_device.req.qtds[0].phys_addr,
                                                    *ctrl_ds_segment,
                                                ));
                                            qh_ptr.qh().write(qh);
                                            qh_manager
                                                .add_qh_to_async_list(resetting_device.req.qh);
                                            resetting_device.step =
                                                ResetDeviceStep::WaitForSetAddr {
                                                    addr: addr_to_set,
                                                };
                                        } else if f.poll_unpin(cx).is_ready() {
                                            self.root_ports_shared[root_port_index]
                                                .res_signal
                                                .signal(Err(Error::DeviceIsFullSpeed));
                                            *resetting_device_option = None;
                                        } else {
                                            break;
                                        }
                                    }
                                    ResetDeviceStep::WaitForSetAddr { addr } => {
                                        let status_qtd = unsafe {
                                            VolatilePtr::new(resetting_device.req.qtds[1].ptr)
                                        };
                                        if !status_qtd.qtd_token().read().active() {
                                            let (tx, rx) = oneshot();
                                            qh_manager.queue_remove_qh(
                                                unsafe {
                                                    VolatileRef::new(
                                                        self.operational_regs
                                                            .as_mut_ptr()
                                                            .usb_cmd()
                                                            .as_raw_ptr(),
                                                    )
                                                },
                                                resetting_device.req.qh,
                                                tx,
                                            );
                                            resetting_device.step =
                                                ResetDeviceStep::WaitForQhRemove {
                                                    addr: *addr,
                                                    qh_manager_rx: rx,
                                                };
                                        } else {
                                            break;
                                        }
                                    }
                                    ResetDeviceStep::WaitForQhRemove {
                                        addr,
                                        qh_manager_rx,
                                    } => {
                                        if qh_manager_rx.poll_unpin(cx).is_ready() {
                                            let shared = Arc::new(DeviceSharedState::default());
                                            devices
                                                .insert(resetting_device.root_port, shared.clone());
                                            self.root_ports_shared[root_port_index]
                                                .res_signal
                                                .signal(Ok(Device::new(
                                                    shared,
                                                    *ctrl_ds_segment,
                                                    *addr,
                                                )));
                                            *resetting_device_option = None;
                                        } else {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        Poll::Pending
                    })
                    .await
                }
            }
        }
    }
}
