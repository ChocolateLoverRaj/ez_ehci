use core::{future, mem::offset_of, task::Poll};

use alloc::sync::Arc;
use arbitrary_int::{traits::Integer, u4, u7, u11, u15};
use embedded_hal_async::delay::DelayNs;
use futures::task::AtomicWaker;
use volatile::{VolatileFieldAccess, VolatilePtr};
use zerocopy::transmute;

use crate::{
    MappedMem, QueueHead,
    buffer_ptrs::BufferPtrs,
    device::Device,
    endpoint_speed::EndpointSpeed,
    irq_handler::UsbIntWakers,
    operational_regs::{LineStatus, PortScReg},
    qh_manager::{QhManager, QhWithMetadata, QhWithMetadataVolatileFieldAccess},
    qtd::{Qtd, QtdVolatileFieldAccess},
    queue_head::{EndpointCapabilities, EndpointCharacteristics},
    setup_packet::SetupPacket,
    transfer_token::{PidCode, TransferToken},
};

pub type PortChangeDetectWaker = Arc<AtomicWaker>;

/// Detects when new USB devices are attached and assigns them an address.
/// New USB devices have address 0, and only 1 device can have address 0 at a time.
/// So the goal is to quickly assign it a new address.
#[derive(Debug)]
pub struct DeviceGreeterWaitingForDevice {
    /// Max 15 root ports, 1 bit per port, 0 means not initialized.
    initialized_ports: u16,
    port_change_detect_waker: PortChangeDetectWaker,
    n_ports: u4,
    port_sc_regs: VolatilePtr<'static, [PortScReg]>,
    next_addr: u7,
    port_wakers: UsbIntWakers,
    qh_manager: Arc<QhManager>,
    buffer: MappedMem<AssignAddrBuffer>,
}

impl DeviceGreeterWaitingForDevice {
    pub(crate) fn new(
        port_change_detect_waker: PortChangeDetectWaker,
        n_ports: u4,
        port_sc_regs: VolatilePtr<'static, [PortScReg]>,
        port_wakers: UsbIntWakers,
        qh_manager: Arc<QhManager>,
        buffer: MappedMem<AssignAddrBuffer>,
    ) -> Self {
        Self {
            initialized_ports: 0,
            next_addr: u7::new(1),
            n_ports,
            port_change_detect_waker,
            port_sc_regs,
            port_wakers,
            qh_manager,
            buffer,
        }
    }

    pub async fn wait_for_device(mut self) -> DeviceGreeterFoundDevice {
        let initialize_ports = self.initialized_ports;
        let port = future::poll_fn(|context| {
            self.port_change_detect_waker.register(context.waker());
            // Check for devices
            for port in 0..self.n_ports.value() {
                if initialize_ports & (1 << port) != 0 {
                    continue;
                }
                let port_sc_reg = self
                    .port_sc_regs
                    .index(usize::try_from(port).unwrap())
                    .read();
                if port_sc_reg.current_connect_status() {
                    self.initialized_ports |= 1 << port;
                    return Poll::Ready(u4::new(port));
                }
            }
            Poll::Pending
        })
        .await;
        DeviceGreeterFoundDevice {
            initialized_ports: self.initialized_ports,
            port_change_detect_waker: self.port_change_detect_waker,
            n_ports: self.n_ports,
            port_sc_regs: self.port_sc_regs,
            port,
            next_addr: self.next_addr,
            port_wakers: self.port_wakers,
            qh_manager: self.qh_manager,
        }
    }

    pub async fn wait_for_device_2(&mut self, delay: &mut impl DelayNs) -> Device {
        let port = future::poll_fn(|context| {
            self.port_change_detect_waker.register(context.waker());
            // Check for devices
            for port in 0..self.n_ports.value() {
                if self.initialized_ports & (1 << port) != 0 {
                    continue;
                }
                let port_sc_reg = self
                    .port_sc_regs
                    .index(usize::try_from(port).unwrap())
                    .read();
                if port_sc_reg.current_connect_status() {
                    self.initialized_ports |= 1 << port;
                    return Poll::Ready(u4::new(port));
                }
            }
            Poll::Pending
        })
        .await;

        log::warn!("setting address for port {port}");

        let port_sc_reg = self
            .port_sc_regs
            .index(usize::try_from(u4::from(port).value()).unwrap());

        let port_sc = port_sc_reg.read();
        log::debug!("Port SC: {port_sc:#X?}");
        if !port_sc.current_connect_status() {
            todo!()
            // return Err(AssignAddressError::NotConncected);
        }
        if LineStatus::from(port_sc.line_status()) == LineStatus::KState {
            // The port is Low speed
            todo!()
            // return Err(AssignAddressError::IsLowSpeed);
        }
        log::debug!("Resetting port");
        port_sc_reg.update(|port_sc| port_sc.with_port_reset(true).without_write_to_clear_bits());
        delay.delay_ms(50).await;
        port_sc_reg.update(|port_sc| port_sc.with_port_reset(false).without_write_to_clear_bits());
        // Wait until the port is enabled
        log::info!("Waiting for port to be enabled");
        delay.delay_ms(2).await;
        log::info!("delay of 2ms done");
        let port_enabled = port_sc_reg.read().port_enabled();
        if !port_enabled {
            // The port will be disabled if the device isn't high speed
            todo!()
            // return Err(AssignAddressError::IsFullSpeed);
        }
        log::info!("Port is enabled");

        let addr_to_set = self.next_addr;
        self.next_addr += u7::new(1);
        let buffer_ptr = unsafe { VolatilePtr::new(self.buffer.ptr) };
        let qh_phys_addr =
            self.buffer.phys_addr + u32::try_from(offset_of!(AssignAddrBuffer, qh)).unwrap();
        let qtds_addr =
            self.buffer.phys_addr + u32::try_from(offset_of!(AssignAddrBuffer, qtds)).unwrap();
        // First transfer: set address
        let mut qtds = [
            Qtd::new(
                TransferToken::new_active(PidCode::SetupToken, u15::new(8))
                    .with_interrupt_on_complete(true),
                BufferPtrs::new_contiguous(
                    u64::from(self.buffer.phys_addr)
                        + u64::try_from(offset_of!(AssignAddrBuffer, packet)).unwrap(),
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
                .with_max_packet_len(u11::new(8)),
            EndpointCapabilities::new_with_raw_value(0),
        );
        qh.link_contiguous_qtds(&mut qtds, qtds_addr);
        buffer_ptr
            .qtds()
            .as_slice()
            .index(0..2)
            .copy_from_slice(&qtds);
        buffer_ptr.qh().qh().write(qh);
        buffer_ptr
            .packet()
            .write(transmute!(SetupPacket::new_set_address(addr_to_set)));
        let qh_mapped_mem = MappedMem {
            phys_addr: qh_phys_addr,
            ptr: buffer_ptr.qh().as_raw_ptr(),
        };
        self.qh_manager.add_qh_to_async_list(qh_mapped_mem);

        future::poll_fn(|context| {
            self.port_wakers[u4::from(port).as_usize()].register(context.waker());
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
            self.port_wakers[u4::from(port).as_usize()].register(context.waker());
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

        // Remove the QH so we can reuse the buffer
        self.qh_manager.remove_qh(qh_mapped_mem, self.n_ports).await;

        Device {}
    }
}

// Align to 4 KiB and ensure size is <4 KiB to make sure buffers never cross 4 KiB boundary.
#[repr(C, align(0x1000))]
#[derive(Debug, VolatileFieldAccess, Clone, Copy)]
pub struct AssignAddrBuffer {
    qtds: [Qtd; 2],
    qh: QhWithMetadata,
    packet: [u8; 8],
}

#[derive(Debug)]
pub enum AssignAddressError {
    NotConncected,
    /// To use this device you need to route it to a uHCI or oHCI.
    IsLowSpeed,
    /// To use this device you need to route it to a uHCI or oHCI.
    IsFullSpeed,
    HostSystemError,
}

/// Detected a new device.
#[derive(Debug)]
pub struct DeviceGreeterFoundDevice {
    /// Max 15 root ports, 1 bit per port, 0 means not initialized.
    initialized_ports: u16,
    next_addr: u7,
    port_change_detect_waker: PortChangeDetectWaker,
    n_ports: u4,
    port_sc_regs: VolatilePtr<'static, [PortScReg]>,
    port: u4,
    port_wakers: UsbIntWakers,
    qh_manager: Arc<QhManager>,
}

impl DeviceGreeterFoundDevice {
    // pub async fn assign_address(
    //     mut self,
    //     buffer: MappedMem<AssignAddrBuffer>,
    //     delay: &mut impl DelayNs,
    // ) -> (
    //     Result<Device, AssignAddressError>,
    //     DeviceGreeterWaitingForDevice,
    // ) {
    //     let port_sc_reg = self
    //         .port_sc_regs
    //         .index(usize::try_from(u4::from(self.port).value()).unwrap());

    //     let port_sc = port_sc_reg.read();
    //     log::debug!("Port SC: {port_sc:#X?}");
    //     if !port_sc.current_connect_status() {
    //         todo!()
    //         // return Err(AssignAddressError::NotConncected);
    //     }
    //     if LineStatus::from(port_sc.line_status()) == LineStatus::KState {
    //         // The port is Low speed
    //         todo!()
    //         // return Err(AssignAddressError::IsLowSpeed);
    //     }
    //     log::debug!("Resetting port");
    //     port_sc_reg.update(|port_sc| port_sc.with_port_reset(true).without_write_to_clear_bits());
    //     delay.delay_ms(50).await;
    //     port_sc_reg.update(|port_sc| port_sc.with_port_reset(false).without_write_to_clear_bits());
    //     // Wait until the port is enabled
    //     log::info!("Waiting for port to be enabled");
    //     delay.delay_ms(2).await;
    //     log::info!("delay of 2ms done");
    //     let port_enabled = port_sc_reg.read().port_enabled();
    //     if !port_enabled {
    //         // The port will be disabled if the device isn't high speed
    //         todo!()
    //         // return Err(AssignAddressError::IsFullSpeed);
    //     }
    //     log::info!("Port is enabled");

    //     let addr_to_set = self.next_addr;
    //     self.next_addr += u7::new(1);
    //     let buffer_ptr = unsafe { VolatilePtr::new(buffer.ptr) };
    //     let qh_phys_addr =
    //         buffer.phys_addr + u32::try_from(offset_of!(AssignAddrBuffer, qh)).unwrap();
    //     let qtds_addr =
    //         buffer.phys_addr + u32::try_from(offset_of!(AssignAddrBuffer, qtds)).unwrap();
    //     // First transfer: set address
    //     let mut qtds = [
    //         Qtd::new(
    //             TransferToken::new_active(PidCode::SetupToken, u15::new(8))
    //                 .with_interrupt_on_complete(true),
    //             BufferPtrs::new_contiguous(
    //                 u64::from(buffer.phys_addr)
    //                     + u64::try_from(offset_of!(AssignAddrBuffer, packet)).unwrap(),
    //                 8,
    //             ),
    //         ),
    //         Qtd::new(
    //             TransferToken::new_active(PidCode::InToken, u15::ZERO)
    //                 .with_interrupt_on_complete(true)
    //                 .with_data_toggle(true),
    //             BufferPtrs::EMPTY,
    //         ),
    //     ];
    //     let mut qh = QueueHead::new(
    //         EndpointCharacteristics::new_with_raw_value(0)
    //             .with_device_addr({
    //                 // new devices have address 0
    //                 u7::ZERO
    //             })
    //             .with_endpoint_number({
    //                 // control endpoint
    //                 u4::ZERO
    //             })
    //             .with_endpoint_speed(EndpointSpeed::High.into())
    //             .with_max_packet_len(u11::new(8)),
    //         EndpointCapabilities::new_with_raw_value(0),
    //     );
    //     qh.link_contiguous_qtds(&mut qtds, qtds_addr);
    //     buffer_ptr
    //         .qtds()
    //         .as_slice()
    //         .index(0..2)
    //         .copy_from_slice(&qtds);
    //     buffer_ptr.qh().qh().write(qh);
    //     buffer_ptr
    //         .packet()
    //         .write(transmute!(SetupPacket::new_set_address(addr_to_set)));
    //     self.qh_manager.add_qh_to_async_list(MappedMem {
    //         phys_addr: qh_phys_addr,
    //         ptr: buffer_ptr.qh().as_raw_ptr(),
    //     });

    //     future::poll_fn(|context| {
    //         self.port_wakers[u4::from(self.port).as_usize()].register(context.waker());
    //         if !buffer_ptr
    //             .qtds()
    //             .as_slice()
    //             .index(0)
    //             .qtd_token()
    //             .read()
    //             .active()
    //         {
    //             return Poll::Ready(());
    //         }
    //         log::info!("awaiting usb int future");
    //         Poll::Pending
    //     })
    //     .await;
    //     log::info!("QTD 0 complete");
    //     future::poll_fn(|context| {
    //         self.port_wakers[u4::from(self.port).as_usize()].register(context.waker());
    //         if !buffer_ptr
    //             .qtds()
    //             .as_slice()
    //             .index(1)
    //             .qtd_token()
    //             .read()
    //             .active()
    //         {
    //             return Poll::Ready(());
    //         }
    //         log::info!("awaiting usb int future");
    //         Poll::Pending
    //     })
    //     .await;
    //     log::info!("QTD 1 complete");

    //     (
    //         Ok(Device {}),
    //         DeviceGreeterWaitingForDevice {
    //             initialized_ports: self.initialized_ports,
    //             n_ports: self.n_ports,
    //             port_change_detect_waker: self.port_change_detect_waker,
    //             port_sc_regs: self.port_sc_regs,
    //             next_addr: self.next_addr,
    //             port_wakers: self.port_wakers,
    //             qh_manager: self.qh_manager,
    //         },
    //     )
    // }
}
