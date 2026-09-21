use core::future::{self};
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use core::task::Poll;
use core::{fmt::Debug, mem::offset_of};

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::{vec, vec::Vec};
use arbitrary_int::{traits::Integer, u4, u7, u11, u12, u15, u20};
use bitbybit::bitfield;
use embedded_hal_async::delay::DelayNs;
use futures::task::AtomicWaker;
use intrusive_collections::{SinglyLinkedList, SinglyLinkedListAtomicLink, intrusive_adapter};
use volatile::{VolatileFieldAccess, VolatilePtr, access::ReadOnly};
use zerocopy::transmute;

use crate::MappedMem;
use crate::buffer_ptrs::BufferPtrs;
pub use crate::new_ehci::{AnyEhci, new_ehci};
use crate::operational_regs::UsbStsReg;
pub use crate::os_owned_ehci::OsOwnedEhci;
pub use crate::pci::{PCI_CLASS, PCI_PROG_IF, PCI_SUBCLASS, PciAccess};
pub use crate::periodic_list::PeriodicFrameList;
use crate::qtd::QtdVolatileFieldAccess;
use crate::queue_head::QueueHeadVolatileFieldAccess;
use crate::root_port_number::RootPortNumber;
use crate::setup_packet::SetupPacket;
pub use crate::usb_leg_sup::{BiosOwnedEhci, TakingOwnershipEhci, TryTakeOutput};
use crate::{
    capability_regs::{CapabilityRegs, CapabilityRegsVolatileFieldAccess},
    endpoint_speed::EndpointSpeed,
    operational_regs::{
        AsyncListAddrReg, LineStatus, OperationalRegs, OperationalRegsVolatileFieldAccess,
        PortScReg,
    },
    qtd::{NextQtdPointer, Qtd, QtdBufferPagePointerPage0, QtdBufferPagePointerPage1Plus},
    queue_head::{
        AlternateQtdLinkPtr, CurrentQtdLinkPtr, EndpointCapabilities, EndpointCharacteristics,
        QhBufferPtrPage0, QhBufferPtrPage1, QhBufferPtrPage2, QhBufferPtrPage3P, QueueHead,
        QueueHeadHorizontalLinkPtr, SelectType,
    },
    transfer_token::{PidCode, TransferToken},
};

struct QtdWatcher {
    link: SinglyLinkedListAtomicLink,
    waker: AtomicWaker,
    qtd: VolatilePtr<'static, Qtd>,
}

intrusive_adapter!(QtdWatcherAdapter = Arc<QtdWatcher>: QtdWatcher { link => SinglyLinkedListAtomicLink });

pub struct InitializedEhci {
    pub(crate) capability_regs: VolatilePtr<'static, CapabilityRegs, ReadOnly>,
    pub(crate) operational_regs: VolatilePtr<'static, OperationalRegs>,
    pub(crate) port_sc_regs: VolatilePtr<'static, [PortScReg]>,
    pub anchor_qh: MappedMem<QueueHead>,
    /// Each port has one.
    pub wakers: Box<[AtomicWaker]>,
    pub port_change_detect_waker: AtomicWaker,
}

/// Safety: safe to drop in different thread than it was created in.
unsafe impl Send for InitializedEhci {}
/// Safety: safe for multiple threads to have a reference to this.
unsafe impl Sync for InitializedEhci {}

const PAYLOAD_BUFFER_LEN: usize = 64;
// Align to 4 KiB and ensure size is <4 KiB to make sure buffers never cross 4 KiB boundary.
#[repr(C, align(0x1000))]
#[derive(Debug, VolatileFieldAccess, Clone, Copy)]
pub struct InitDeviceBuffer {
    qtds: [Qtd; 5],
    qhs: [QueueHead; 2],
    set_address_setup_packet: [u8; 8],
    get_descriptor_setup_packet: [u8; 8],
    descriptor_buffer: [u8; PAYLOAD_BUFFER_LEN],
}

#[derive(Debug)]
pub enum InitDeviceError {
    NotConncected,
    /// To use this device you need to route it to a uHCI or oHCI.
    IsLowSpeed,
    /// To use this device you need to route it to a uHCI or oHCI.
    IsFullSpeed,
    HostSystemError,
}

impl InitializedEhci {
    pub(crate) fn new(
        capability_regs: VolatilePtr<'static, CapabilityRegs, ReadOnly>,
        operational_regs: VolatilePtr<'static, OperationalRegs>,
        port_sc_regs: VolatilePtr<'static, [PortScReg]>,
        anchor_qh: MappedMem<QueueHead>,
    ) -> Self {
        Self {
            capability_regs,
            operational_regs,
            port_sc_regs,
            anchor_qh,
            wakers: (0..capability_regs.hcs_params().read().n_ports().as_usize())
                .map(|_| AtomicWaker::new())
                .collect(),
            port_change_detect_waker: AtomicWaker::new(),
        }
    }
    fn add_qh_to_async_list(&self, qh: MappedMem<QueueHead>) {
        let qh_ptr = unsafe { VolatilePtr::new(qh.ptr) };
        let anchor_qh_ptr = unsafe { VolatilePtr::new(self.anchor_qh.ptr) };
        qh_ptr
            .queue_head_horizontal_link_ptr()
            .write(anchor_qh_ptr.queue_head_horizontal_link_ptr().read());
        anchor_qh_ptr
            .queue_head_horizontal_link_ptr()
            .write(QueueHeadHorizontalLinkPtr::new(
                SelectType::Qh,
                qh.phys_addr,
            ));
    }

    pub async fn run(&self) -> NewDeviceEvent {
        future::poll_fn(|context| {
            self.port_change_detect_waker.register(context.waker());
            // Check for devices
            for port in 0..self.capability_regs.hcs_params().read().n_ports().value() {
                let port_sc_reg = self
                    .port_sc_regs
                    .index(usize::try_from(port).unwrap())
                    .read();
                if port_sc_reg.current_connect_status() {
                    return Poll::Ready(NewDeviceEvent {
                        port: u4::new(port).try_into().unwrap(),
                    });
                }
            }
            Poll::Pending
        })
        .await
    }

    pub fn handle_interrupt(&self) {
        let status = self.operational_regs.usb_sts().read();
        log::info!("eHCI interrupt. Status: {status:#X?}");
        // TODO: This can make us exit our 2ms wait early.
        let mut clear = UsbStsReg::new_with_raw_value(0).with_frame_list_rollover(true);
        if status.usb_err_int() {
            panic!("usb err int");
        } else if status.host_system_error() {
            panic!("host system error");
        }
        if status.interrupt_on_async_advance() {
            clear.set_interrupt_on_async_advance(true);
        }
        if status.port_change_detect() {
            clear.set_port_change_detect(true);
            self.port_change_detect_waker.wake();
        }
        if status.usb_int() {
            clear.set_usb_int(true);
            for waker in &self.wakers {
                waker.wake();
            }
        }
        self.operational_regs.usb_sts().write(clear);

        for port in 0..self.capability_regs.hcs_params().read().n_ports().value() {
            let port_sc_reg_ptr = self.port_sc_regs.index(usize::try_from(port).unwrap());
            let port_sc_reg = port_sc_reg_ptr.read();
            if port_sc_reg.connect_status_change() {
                log::info!("port {port} connect status changed");
            }
            if port_sc_reg.port_enable_change() {
                log::info!("port {port} port enable changed");
            }
            if port_sc_reg.over_current_change() {
                log::info!("port {port} over current change");
            }
            // Clear interrupts
            port_sc_reg_ptr.write(port_sc_reg);
        }
    }

    pub async fn init_device(
        &self,
        root_port_number: RootPortNumber,
        buffer: MappedMem<InitDeviceBuffer>,
        delay: &mut impl DelayNs,
    ) -> Result<(), InitDeviceError> {
        for _ in 0..10 {
            delay.delay_ms(2).await;
            log::info!("did test delay");
        }

        let port_sc_reg = self
            .port_sc_regs
            .index(usize::try_from(u4::from(root_port_number).value()).unwrap());

        let port_sc = port_sc_reg.read();
        log::debug!("Port SC: {port_sc:#X?}");
        if !port_sc.current_connect_status() {
            return Err(InitDeviceError::NotConncected);
        }
        if LineStatus::from(port_sc.line_status()) == LineStatus::KState {
            // The port is Low speed
            return Err(InitDeviceError::IsLowSpeed);
        }
        log::debug!("Resetting port");
        port_sc_reg.update(|port_sc| port_sc.with_port_reset(true).without_write_to_clear_bits());
        delay.delay_ms(50).await;
        port_sc_reg.update(|port_sc| port_sc.with_port_reset(false).without_write_to_clear_bits());
        // Wait until the port is enabled
        log::info!("Waiting for port to be enabled");
        delay.delay_ms(2).await;
        log::info!("delay of 2ms done");
        // let timeout = delay.delay_ms(2);
        // let mut timeout_pinned = pin!(timeout);
        // let port_enabled = loop {
        //     let int_future = future::poll_fn(|context| {
        //         self.waker.register(context.waker());
        //         if self.int_occurred.swap(false, Ordering::Relaxed) {
        //             return Poll::Ready(());
        //         }
        //         Poll::Pending
        //     });
        //     match select(timeout_pinned, int_future).await {
        //         Either::Left(_) => {
        //             log::info!("2ms timer over, checking if port enabled");
        //             break port_sc_reg.read().port_enabled();
        //         }
        //         Either::Right((_, r_timeout)) => {
        //             if port_sc_reg.read().port_enabled() {
        //                 log::info!("port enabled, detected from interrupt");
        //                 break true;
        //             }
        //             timeout_pinned = r_timeout;
        //         }
        //     }
        // };
        let port_enabled = port_sc_reg.read().port_enabled();
        if !port_enabled {
            // The port will be disabled if the device isn't high speed
            return Err(InitDeviceError::IsFullSpeed);
        }
        log::info!("Port is enabled");

        let addr_to_set = u7::new(0x67);
        let buffer_ptr = unsafe { VolatilePtr::new(buffer.ptr) };
        let qhs_phys_addr =
            buffer.phys_addr + u32::try_from(offset_of!(InitDeviceBuffer, qhs)).unwrap();
        let qh_size = u32::try_from(size_of::<QueueHead>()).unwrap();
        let qtds_addr =
            buffer.phys_addr + u32::try_from(offset_of!(InitDeviceBuffer, qtds)).unwrap();
        let qtd_size = u32::try_from(size_of::<Qtd>()).unwrap();
        // First transfer: set address
        let mut qtds = [
            Qtd::new(
                TransferToken::new_active(PidCode::SetupToken, u15::new(8))
                    .with_interrupt_on_complete(true),
                BufferPtrs::new_contiguous(
                    u64::from(buffer.phys_addr)
                        + u64::try_from(offset_of!(InitDeviceBuffer, set_address_setup_packet))
                            .unwrap(),
                    8,
                ),
            ),
            Qtd::new(
                TransferToken::new_active(PidCode::InToken, u15::ZERO)
                    .with_interrupt_on_complete(true)
                    .with_data_toggle(true),
                BufferPtrs::EMPTY,
            ),
        ];
        let mut qh = QueueHead::new(
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
                .with_max_packet_len(u11::new({
                    // this is the initial max packet len used until we know the device's max packet len
                    PAYLOAD_BUFFER_LEN.try_into().unwrap()
                })),
            EndpointCapabilities::new_with_raw_value(0),
        );
        qh.link_contiguous_qtds(&mut qtds, qtds_addr);
        buffer_ptr
            .qtds()
            .as_slice()
            .index(0..2)
            .copy_from_slice(&qtds);
        buffer_ptr.qhs().as_slice().index(0).write(qh);
        buffer_ptr
            .set_address_setup_packet()
            .write(transmute!(SetupPacket::new_set_address(addr_to_set)));
        self.add_qh_to_async_list(MappedMem {
            phys_addr: qhs_phys_addr,
            ptr: buffer_ptr.qhs().as_slice().index(0).as_raw_ptr(),
        });

        future::poll_fn(|context| {
            self.wakers[u4::from(root_port_number).as_usize()].register(context.waker());
            if !buffer_ptr
                .qtds()
                .as_slice()
                .index(0)
                .qtd_token()
                .read()
                .active()
            {
                return Poll::Ready(());
            }
            log::info!("awaiting usb int future");
            Poll::Pending
        })
        .await;
        log::info!("QTD 0 complete");
        future::poll_fn(|context| {
            self.wakers[u4::from(root_port_number).as_usize()].register(context.waker());
            if !buffer_ptr
                .qtds()
                .as_slice()
                .index(1)
                .qtd_token()
                .read()
                .active()
            {
                return Poll::Ready(());
            }
            log::info!("awaiting usb int future");
            Poll::Pending
        })
        .await;
        log::info!("QTD 1 complete");

        delay.delay_ms(2).await;

        log::info!("Doing get descriptor");
        let mut qtds = [
            Qtd::new(
                TransferToken::new_active(PidCode::SetupToken, u15::new(8))
                    .with_interrupt_on_complete(true),
                BufferPtrs::new_contiguous(
                    u64::from(buffer.phys_addr)
                        + u64::try_from(offset_of!(InitDeviceBuffer, get_descriptor_setup_packet))
                            .unwrap(),
                    8,
                ),
            ),
            Qtd::new(
                TransferToken::new_active(
                    PidCode::InToken,
                    u15::new(PAYLOAD_BUFFER_LEN.try_into().unwrap()),
                )
                .with_data_toggle(true)
                .with_interrupt_on_complete(true),
                BufferPtrs::new_contiguous(
                    u64::from(buffer.phys_addr)
                        + u64::try_from(offset_of!(InitDeviceBuffer, descriptor_buffer)).unwrap(),
                    PAYLOAD_BUFFER_LEN.try_into().unwrap(),
                ),
            ),
            Qtd::new(
                TransferToken::new_active(PidCode::OutToken, u15::ZERO)
                    .with_data_toggle(true)
                    .with_interrupt_on_complete(true),
                BufferPtrs::EMPTY,
            ),
        ];
        let mut qh = QueueHead::new(
            EndpointCharacteristics::new_with_raw_value(0)
                .with_head_of_reclamation_list_flag(false)
                .with_device_addr(addr_to_set)
                .with_endpoint_number({
                    // control endpoint
                    u4::ZERO
                })
                .with_endpoint_speed(EndpointSpeed::High.into())
                .with_max_packet_len(u11::new({
                    // this is the initial max packet len used until we know the device's max packet len
                    PAYLOAD_BUFFER_LEN.try_into().unwrap()
                })),
            EndpointCapabilities::ZERO,
        );
        qh.link_contiguous_qtds(&mut qtds, qtds_addr + qtd_size * 2);
        buffer_ptr
            .qtds()
            .as_slice()
            .index(2..5)
            .copy_from_slice(&qtds);
        buffer_ptr.qhs().as_slice().index(1).write(qh);
        buffer_ptr.get_descriptor_setup_packet().write(transmute!(
            SetupPacket::new_get_descriptor(PAYLOAD_BUFFER_LEN.try_into().unwrap(),)
        ));
        self.add_qh_to_async_list(MappedMem {
            phys_addr: qhs_phys_addr + qh_size,
            ptr: buffer_ptr.qhs().as_slice().index(1).as_raw_ptr(),
        });
        future::poll_fn(|context| {
            self.wakers[u4::from(root_port_number).as_usize()].register(context.waker());
            if !buffer_ptr
                .qtds()
                .as_slice()
                .index(2)
                .qtd_token()
                .read()
                .active()
            {
                return Poll::Ready(());
            }
            log::info!("awaiting usb int future");
            Poll::Pending
        })
        .await;
        log::info!("QTD 2 complete");
        let qtd_3_ptr = buffer_ptr.qtds().as_slice().index(3);
        future::poll_fn(|context| {
            self.wakers[u4::from(root_port_number).as_usize()].register(context.waker());
            if !buffer_ptr
                .qtds()
                .as_slice()
                .index(3)
                .qtd_token()
                .read()
                .active()
            {
                return Poll::Ready(());
            }
            log::info!("awaiting usb int future");
            Poll::Pending
        })
        .await;
        let bytes_to_transfer = qtd_3_ptr.qtd_token().read().total_bytes_to_transfer();
        let bytes_transferred =
            u15::new(PAYLOAD_BUFFER_LEN.try_into().unwrap()) - bytes_to_transfer;
        let buffer = buffer_ptr.descriptor_buffer().read();
        let bytes = &buffer[..usize::try_from(bytes_transferred.value()).unwrap()];
        log::info!("QTD 3 complete. read: {bytes:02X?}");
        future::poll_fn(|context| {
            self.wakers[u4::from(root_port_number).as_usize()].register(context.waker());
            if !buffer_ptr
                .qtds()
                .as_slice()
                .index(4)
                .qtd_token()
                .read()
                .active()
            {
                return Poll::Ready(());
            }
            log::info!("awaiting usb int future");
            Poll::Pending
        })
        .await;
        log::info!("QTD 4 complete");
        todo!()
    }
}

#[derive(Debug)]
pub struct NewDeviceEvent {
    pub port: RootPortNumber,
}
