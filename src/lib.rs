#![no_std]
mod capability_regs;
mod endpoint_speed;
mod new_ehci;
mod operational_regs;
mod os_owned_ehci;
mod pci;
mod periodic_list;
mod qtd;
mod queue_head;
mod transfer_token;
mod usb_leg_sup;

use core::{
    arch::x86_64::_mm_pause, fmt::Debug, hint::spin_loop, mem::offset_of, num::NonZero,
    ptr::NonNull,
};

use arbitrary_int::{traits::Integer, u4, u7, u11, u12, u15, u20};
use bitbybit::bitfield;
use volatile::{VolatileFieldAccess, VolatilePtr, access::ReadOnly};

pub use crate::new_ehci::{AnyEhci, new_ehci};
pub use crate::os_owned_ehci::OsOwnedEhci;
pub use crate::pci::{PCI_CLASS, PCI_PROG_IF, PCI_SUBCLASS, PciAccess};
pub use crate::periodic_list::PeriodicFrameList;
pub use crate::usb_leg_sup::{BiosOwnedEhci, TakingOwnershipEhci, TryTakeOutput};
use crate::{
    capability_regs::{CapabilityRegs, CapabilityRegsVolatileFieldAccess},
    endpoint_speed::EndpointSpeed,
    operational_regs::{
        AsyncListAddrReg, LineStatus, OperationalRegs, OperationalRegsVolatileFieldAccess,
        PortScReg,
    },
    qtd::{
        NextQtdPointer, QtdBufferPagePointerPage0, QtdBufferPagePointerPage1Plus,
        QueueElementTransferDescriptor,
    },
    queue_head::{
        AlternateQtdLinkPtr, CurrentQtdLinkPtr, EndpointCapabilities, EndpointCharacteristics,
        QhBufferPtrPage0, QhBufferPtrPage1, QhBufferPtrPage2, QhBufferPtrPage3P, QueueHead,
        QueueHeadHorizontalLinkPtr, SelectType,
    },
    transfer_token::{PidCode, TransferToken},
};

#[derive(Debug, Clone, Copy)]
pub struct MappedMem<T> {
    pub phys_addr: u32,
    pub ptr: NonNull<T>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RootPortNumber(u4);

impl TryFrom<u4> for RootPortNumber {
    type Error = OutOfRange;

    fn try_from(value: u4) -> Result<Self, Self::Error> {
        if value <= u4::new(15) {
            Ok(Self(value))
        } else {
            Err(OutOfRange)
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OutOfRange;

#[derive(Debug)]
pub struct NewDeviceEvent {
    pub port: RootPortNumber,
}

#[derive(Debug)]
pub enum RunOutput {
    /// Nothing to do, wait for interrupts
    Idle,
    /// Device detected
    NewDevice(NewDeviceEvent),
}

pub struct InitializedEhci {
    capability_regs: VolatilePtr<'static, CapabilityRegs, ReadOnly>,
    operational_regs: VolatilePtr<'static, OperationalRegs>,
    port_sc_regs: VolatilePtr<'static, [PortScReg]>,
}

// Align to 4 KiB and ensure size is <4 KiB to make sure buffers never cross 4 KiB boundary.
#[repr(C, align(0x1000))]
#[derive(Debug, VolatileFieldAccess, Clone, Copy)]
pub struct InitDeviceBuffer {
    qtds: [QueueElementTransferDescriptor; 3],
    queue_head: QueueHead,
    setup_packet_buffer: [u8; 8],
    payload_buffer: [u8; 64],
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
    pub fn run(&mut self) -> RunOutput {
        // Check for devices
        for port in 0..self.capability_regs.hcs_params().read().n_ports().value() {
            log::info!("checking port {port}");
            let port_sc_reg = self
                .port_sc_regs
                .index(usize::try_from(port).unwrap())
                .read();
            if port_sc_reg.current_connect_status() {
                return RunOutput::NewDevice(NewDeviceEvent {
                    port: u4::new(port).try_into().unwrap(),
                });
            }
        }
        RunOutput::Idle
    }

    pub fn init_device(
        &mut self,
        root_port_number: RootPortNumber,
        buffer: MappedMem<InitDeviceBuffer>,
    ) -> Result<(), InitDeviceError> {
        let port_sc_reg = self
            .port_sc_regs
            .index(usize::try_from(root_port_number.0.value()).unwrap());

        let port_sc = port_sc_reg.read();
        log::debug!("Port SC: {port_sc:#X?}");
        if !port_sc.current_connect_status() {
            return Err(InitDeviceError::NotConncected);
        }
        if LineStatus::from(port_sc.line_status()) == LineStatus::KState {
            // The port is Low speed
            return Err(InitDeviceError::IsLowSpeed);
        }

        /// Busy loop delay for approximately 2ms on a ~2.4 GHz Arrandale CPU (~400,000 iterations)
        #[inline(never)]
        pub fn delay_approx_2ms() {
            // 400,000 iterations * ~12 cycles/iter = ~4,800,000 cycles (~2ms)
            for _ in 0..400_000 {
                _mm_pause();
            }
        }

        /// Busy loop delay for approximately 50ms on a ~2.4 GHz Arrandale CPU (~10,000,000 iterations)
        #[inline(never)]
        pub fn delay_approx_50ms() {
            // 10,000,000 iterations * ~12 cycles/iter = ~120,000,000 cycles (~50ms)
            for _ in 0..10_000_000 {
                _mm_pause();
            }
        }

        log::info!("Resetting port");
        port_sc_reg.update(|port_sc| port_sc.with_port_reset(true).without_write_to_clear_bits());
        delay_approx_50ms();
        // FIXME: Wait 50ms
        port_sc_reg.update(|port_sc| port_sc.with_port_reset(false).without_write_to_clear_bits());
        // Wait until the port is enabled
        log::info!("Waiting for port to be enabled");
        // Wait up to 2 ms for the port to be enabled
        delay_approx_2ms();
        // TODO: Actually wait
        if !port_sc_reg.read().port_enabled() {
            // The port will be disabled if the device isn't high speed
            return Err(InitDeviceError::IsFullSpeed);
        }
        log::info!("Port is enabled");
        let buffer_ptr = unsafe { VolatilePtr::new(buffer.ptr) };
        let queue_head_phys_addr =
            buffer.phys_addr + u32::try_from(offset_of!(InitDeviceBuffer, queue_head)).unwrap();
        let next_qtd_pointer = NextQtdPointer::new_valid(
            buffer.phys_addr + u32::try_from(offset_of!(InitDeviceBuffer, qtds)).unwrap(),
        );
        buffer_ptr.write(InitDeviceBuffer {
            qtds: [
                QueueElementTransferDescriptor {
                    next_qtd_ptr: NextQtdPointer::new_valid(
                        buffer.phys_addr
                            + u32::try_from(
                                offset_of!(InitDeviceBuffer, qtds)
                                    + size_of::<QueueElementTransferDescriptor>() * 1,
                            )
                            .unwrap(),
                    ),
                    // next_qtd_ptr: NextQtdPointer::INVALID,
                    alternate_next_qtd_ptr: AlternateQtdLinkPtr::INVALID,
                    qtd_token: TransferToken::new_active(PidCode::SetupToken, u15::new(8))
                        .with_interrupt_on_complete(true),
                    buffer_pointer_page_0: QtdBufferPagePointerPage0::new_with_raw_value(0)
                        .with_ptr_upper(u20::new(buffer.phys_addr >> 12))
                        .with_current_offset(u12::new(
                            offset_of!(InitDeviceBuffer, setup_packet_buffer)
                                .try_into()
                                .unwrap(),
                        )),
                    buffer_pointer_page_1: QtdBufferPagePointerPage1Plus::new_with_raw_value(0),
                    buffer_pointer_page_2: QtdBufferPagePointerPage1Plus::new_with_raw_value(0),
                    buffer_pointer_page_3: QtdBufferPagePointerPage1Plus::new_with_raw_value(0),
                    buffer_pointer_page_4: QtdBufferPagePointerPage1Plus::new_with_raw_value(0),
                    extended_buffer_ptr_page_0: 0,
                    extended_buffer_ptr_page_1: 0,
                    extended_buffer_ptr_page_2: 0,
                    extended_buffer_ptr_page_3: 0,
                    extended_buffer_ptr_page_4: 0,
                },
                QueueElementTransferDescriptor {
                    next_qtd_ptr: NextQtdPointer::new_valid(
                        buffer.phys_addr
                            + u32::try_from(
                                offset_of!(InitDeviceBuffer, qtds)
                                    + size_of::<QueueElementTransferDescriptor>() * 2,
                            )
                            .unwrap(),
                    ),
                    alternate_next_qtd_ptr: AlternateQtdLinkPtr::INVALID,
                    qtd_token: TransferToken::new_active(PidCode::InToken, u15::new(64))
                        .with_data_toggle(true)
                        .with_interrupt_on_complete(true),
                    buffer_pointer_page_0: QtdBufferPagePointerPage0::new_with_raw_value(0)
                        .with_ptr_upper(u20::new(buffer.phys_addr >> 12))
                        .with_current_offset(u12::new(
                            offset_of!(InitDeviceBuffer, payload_buffer)
                                .try_into()
                                .unwrap(),
                        )),
                    buffer_pointer_page_1: QtdBufferPagePointerPage1Plus::new_with_raw_value(0),
                    buffer_pointer_page_2: QtdBufferPagePointerPage1Plus::new_with_raw_value(0),
                    buffer_pointer_page_3: QtdBufferPagePointerPage1Plus::new_with_raw_value(0),
                    buffer_pointer_page_4: QtdBufferPagePointerPage1Plus::new_with_raw_value(0),
                    extended_buffer_ptr_page_0: 0,
                    extended_buffer_ptr_page_1: 0,
                    extended_buffer_ptr_page_2: 0,
                    extended_buffer_ptr_page_3: 0,
                    extended_buffer_ptr_page_4: 0,
                },
                QueueElementTransferDescriptor {
                    next_qtd_ptr: NextQtdPointer::INVALID,
                    alternate_next_qtd_ptr: AlternateQtdLinkPtr::INVALID,
                    qtd_token: TransferToken::new_active(PidCode::OutToken, u15::ZERO)
                        .with_data_toggle(true)
                        .with_interrupt_on_complete(true),
                    buffer_pointer_page_0: QtdBufferPagePointerPage0::new_with_raw_value(0),
                    buffer_pointer_page_1: QtdBufferPagePointerPage1Plus::new_with_raw_value(0),
                    buffer_pointer_page_2: QtdBufferPagePointerPage1Plus::new_with_raw_value(0),
                    buffer_pointer_page_3: QtdBufferPagePointerPage1Plus::new_with_raw_value(0),
                    buffer_pointer_page_4: QtdBufferPagePointerPage1Plus::new_with_raw_value(0),
                    extended_buffer_ptr_page_0: 0,
                    extended_buffer_ptr_page_1: 0,
                    extended_buffer_ptr_page_2: 0,
                    extended_buffer_ptr_page_3: 0,
                    extended_buffer_ptr_page_4: 0,
                },
            ],
            queue_head: QueueHead {
                queue_head_horizontal_link_ptr: QueueHeadHorizontalLinkPtr::new(
                    SelectType::Qh,
                    queue_head_phys_addr,
                ),
                endpoint_charactersistics: EndpointCharacteristics::new_with_raw_value(0)
                    .with_head_of_reclamation_list_flag({
                        // This is the first item in the circular linked list of queue heads
                        true
                    })
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
                        64
                    })),
                endpoint_capabilities: EndpointCapabilities::new_with_raw_value(0),
                current_qtd_pointer: CurrentQtdLinkPtr::new_with_raw_value(0),
                next_qtd_pointer,
                // next_qtd_pointer: NextQtdPointer::INVALID,
                alternate_qtd_pointer: AlternateQtdLinkPtr::INVALID,
                transfer_token: TransferToken::new_with_raw_value(0),
                buffer_ptr_page_0: QhBufferPtrPage0::new_with_raw_value(0),
                buffer_ptr_page_1: QhBufferPtrPage1::new_with_raw_value(0),
                buffer_ptr_page_2: QhBufferPtrPage2::new_with_raw_value(0),
                buffer_ptr_page_3: QhBufferPtrPage3P::new_with_raw_value(0),
                buffer_ptr_page_4: QhBufferPtrPage3P::new_with_raw_value(0),
                extended_buffer_ptr_page_0: 0,
                extended_buffer_ptr_page_1: 0,
                extended_buffer_ptr_page_2: 0,
                extended_buffer_ptr_page_3: 0,
                extended_buffer_ptr_page_4: 0,
            },
            // Standard GET_DESCRIPTOR (Device) request
            setup_packet_buffer: [
                0x80, // bmRequestType: Device-to-Host, Standard, Device
                0x06, // bRequest: GET_DESCRIPTOR
                0x00, 0x01, // wValue: Descriptor Type (0x01 = Device) & Index (0x00)
                0x00, 0x00, // wIndex: 0
                0x40, 0x00, // wLength: 64 bytes (initial fetch)
            ],
            payload_buffer: [Default::default(); _],
        });
        let value = AsyncListAddrReg::new(queue_head_phys_addr);
        self.operational_regs.async_list_addr().write(value);
        // let usb_sts = self.operational_regs.usb_sts().read();
        // log::info!("USB status: {usb_sts:#X?}");
        self.operational_regs
            .usb_cmd()
            .update(|usb_cmd| usb_cmd.with_async_schedule_enable(true));
        log::info!("Initialized and enabled async schedule.");
        let usb_sts = self.operational_regs.usb_sts().read();
        if usb_sts.host_system_error() {
            log::error!("USB status: {usb_sts:#X?}");
            return Err(InitDeviceError::HostSystemError);
        }

        // TODO: use interrupts
        while buffer_ptr
            .qtds()
            .as_slice()
            .index(0)
            .read()
            .qtd_token
            .active()
        {
            spin_loop();
        }
        log::info!("QTD 0 complete");
        while buffer_ptr
            .qtds()
            .as_slice()
            .index(1)
            .read()
            .qtd_token
            .active()
        {
            spin_loop();
        }
        let buffer = buffer_ptr.payload_buffer().read();
        log::info!("QTD 1 complete. Buffer: {buffer:02X?}");
        while buffer_ptr
            .qtds()
            .as_slice()
            .index(2)
            .read()
            .qtd_token
            .active()
        {
            spin_loop();
        }
        log::info!("QTD 2 complete");
        todo!()
    }
}

#[bitfield(u32, debug)]
struct ExtendedCapabilityPointerReg {
    #[bits(0..=7, r)]
    capability_id: u8,
    #[bits(8..=15, r)]
    next_cap_pointer: u8,
    #[bits(16..=31, rw)]
    capability_specific: u16,
}

struct ExtendedCapabilitiesIterator<P: PciAccess> {
    pci_access: P,
    capability_pointer: Option<NonZero<u8>>,
}

impl<P: PciAccess> ExtendedCapabilitiesIterator<P> {
    fn new(pci_access: P, capability_regs: VolatilePtr<'static, CapabilityRegs, ReadOnly>) -> Self {
        Self {
            pci_access,
            capability_pointer: NonZero::new(capability_regs.hcc_params().read().eecp()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CapabilityInfo {
    /// Represents an offset in bytes in the PCI configuration space.
    offset: NonZero<u8>,
    id: u8,
}

#[repr(u8)]
enum ExtendedCapability {
    UsbLegSup = 0x1,
}

#[bitfield(u32, debug)]
struct UsbLegSupReg {
    #[bit(16, r)]
    bios_owned_semaphore: bool,
    #[bit(24, rw)]
    os_owned_semaphor: bool,
}

impl<P: PciAccess> Iterator for ExtendedCapabilitiesIterator<P> {
    type Item = CapabilityInfo;

    fn next(&mut self) -> Option<Self::Item> {
        let capability_pointer = self.capability_pointer?;
        let capability_reg = ExtendedCapabilityPointerReg::new_with_raw_value(
            self.pci_access.read_u32(capability_pointer.get()),
        );
        self.capability_pointer = NonZero::new(capability_reg.next_cap_pointer());
        Some(CapabilityInfo {
            offset: capability_pointer,
            id: capability_reg.capability_id(),
        })
    }
}
