#![no_std]
mod capability_regs;
mod endpoint_speed;
mod operational_regs;
mod periodic_list;
mod qtd;
mod queue_head;
mod transfer_token;
mod usb_leg_sup;

use core::{
    arch::x86_64::{_mm_clflush, _mm_pause},
    fmt::Debug,
    hint::spin_loop,
    mem::{MaybeUninit, offset_of},
    num::NonZero,
    ptr::NonNull,
    sync::atomic::AtomicPtr,
};

use arbitrary_int::{traits::Integer, u2, u4, u7, u11, u12, u15, u20};
use bitbybit::bitfield;
use volatile::{VolatileFieldAccess, VolatilePtr, access::ReadOnly};

pub use crate::periodic_list::PeriodicFrameList;
use crate::{
    capability_regs::{CapabilityRegs, CapabilityRegsVolatileFieldAccess},
    endpoint_speed::EndpointSpeed,
    operational_regs::{
        AsyncListAddrReg, LineStatus, OperationalRegs, OperationalRegsVolatileFieldAccess,
        PortScReg,
    },
    periodic_list::PeriodicFrameListElement,
    qtd::{
        NextQtdPointer, QtdBufferPagePointerPage0, QtdBufferPagePointerPage1Plus,
        QueueElementTransferDescriptor, QueueElementTransferDescriptorVolatileFieldAccess,
    },
    queue_head::{
        AlternateQtdLinkPtr, CurrentQtdLinkPtr, EndpointCapabilities, EndpointCharacteristics,
        QhBufferPtrPage0, QhBufferPtrPage1, QhBufferPtrPage2, QhBufferPtrPage3P, QueueHead,
        QueueHeadHorizontalLinkPtr, SelectType,
    },
    transfer_token::{PidCode, TransferToken},
};

pub const PCI_CLASS: u8 = 0x0C;
pub const PCI_SUBCLASS: u8 = 0x03;
pub const PCI_PROG_IF: u8 = 0x20;

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

pub trait PciAccess {
    fn read_u32(&mut self, offset: u8) -> u32;
    fn write_u8(&mut self, offset: u8, value: u8);
    fn write_u16(&mut self, offset: u8, value: u16);
    fn write_u32(&mut self, offset: u8, value: u32);
}

impl<P: PciAccess> PciAccess for &mut P {
    fn read_u32(&mut self, offset: u8) -> u32 {
        P::read_u32(self, offset)
    }
    fn write_u8(&mut self, offset: u8, value: u8) {
        P::write_u8(self, offset, value)
    }
    fn write_u16(&mut self, offset: u8, value: u16) {
        P::write_u16(self, offset, value)
    }
    fn write_u32(&mut self, offset: u8, value: u32) {
        P::write_u32(self, offset, value)
    }
}

/// # Safety
/// - `mapped_bar` must be the virtual address pointing to the bar.
/// - The entire BAR 0 must be mapped with the right caching type.
pub unsafe fn new_ehci<P: PciAccess>(mapped_bar: NonNull<[u8]>, mut pci_access: P) -> AnyEhci<P> {
    let capability_regs = unsafe {
        VolatilePtr::<CapabilityRegs>::new_read_only(mapped_bar.cast::<CapabilityRegs>())
    };
    for extended_capability in ExtendedCapabilitiesIterator::new(&mut pci_access, capability_regs) {
        log::info!("extended capability: {extended_capability:#X?}");
    }
    if let Some(usb_leg_sup_offset) =
        ExtendedCapabilitiesIterator::new(&mut pci_access, capability_regs).find_map(|c| {
            if c.id == ExtendedCapability::UsbLegSup as u8 {
                Some(c.offset)
            } else {
                None
            }
        })
    {
        AnyEhci::BiosOwned(BiosOwnedEhci {
            mapped_bar,
            pci_access,
            usb_leg_sup_offset,
        })
    } else {
        AnyEhci::OsOwned(unsafe { OsOwnedEhci::new(mapped_bar) })
    }
}

pub enum AnyEhci<P: PciAccess> {
    BiosOwned(BiosOwnedEhci<P>),
    OsOwned(OsOwnedEhci),
}

pub struct BiosOwnedEhci<P: PciAccess> {
    mapped_bar: NonNull<[u8]>,
    pci_access: P,
    usb_leg_sup_offset: NonZero<u8>,
}

impl<P: PciAccess> BiosOwnedEhci<P> {
    pub fn take_ownership(mut self) -> TakingOwnershipEhci<P> {
        // Write 1 to the HC OS Owned Semaphore, which is byte 3
        // Other bits are reserved (write 0)
        self.pci_access
            .write_u8(self.usb_leg_sup_offset.get() + 3, 0x1);
        TakingOwnershipEhci {
            mapped_bar: self.mapped_bar,
            pci_access: self.pci_access,
            usb_leg_sup_offset: self.usb_leg_sup_offset,
        }
    }
}

pub struct TakingOwnershipEhci<P: PciAccess> {
    mapped_bar: NonNull<[u8]>,
    pci_access: P,
    usb_leg_sup_offset: NonZero<u8>,
}

pub enum TryTakeOutput<P: PciAccess> {
    NotYet(TakingOwnershipEhci<P>),
    Taken(OsOwnedEhci),
}

impl<P: PciAccess> TakingOwnershipEhci<P> {
    pub fn try_take(mut self) -> TryTakeOutput<P> {
        // Read the HC BIOS Owned Semaphore
        let reg = UsbLegSupReg::new_with_raw_value(
            self.pci_access.read_u32(self.usb_leg_sup_offset.get()),
        );
        if reg.bios_owned_semaphore() {
            TryTakeOutput::NotYet(self)
        } else {
            self.pci_access
                .write_u32(self.usb_leg_sup_offset.get() + 0x4, 0);
            TryTakeOutput::Taken(unsafe { OsOwnedEhci::new(self.mapped_bar) })
        }
    }
}

pub struct OsOwnedEhci {
    capability_regs: VolatilePtr<'static, CapabilityRegs, ReadOnly>,
    operational_regs: VolatilePtr<'static, OperationalRegs>,
    port_sc_regs: VolatilePtr<'static, [PortScReg]>,
}

impl OsOwnedEhci {
    /// # Safety
    /// The bar must be the virtual address pointing to the bar. The entire BAR 0 must be mapped with the right caching type.
    unsafe fn new(bar: NonNull<[u8]>) -> Self {
        let capability_regs_ptr =
            unsafe { VolatilePtr::<CapabilityRegs>::new_read_only(bar.cast()) };
        let capability_regs = capability_regs_ptr.read();
        log::debug!("capability regs: {capability_regs:#X?}");
        let operational_regs_ptr = unsafe {
            VolatilePtr::new(
                bar.byte_offset(capability_regs.cap_len.try_into().unwrap())
                    .cast(),
            )
        };
        let operational_regs = operational_regs_ptr.read();
        log::debug!("operational regs: {operational_regs:#X?}");
        let n_root_ports = capability_regs.hcs_params.n_ports();
        let n_ports = n_root_ports.value().try_into().unwrap();
        let port_sc_regs = unsafe {
            VolatilePtr::new(NonNull::slice_from_raw_parts(
                operational_regs_ptr.port_sc().as_raw_ptr().cast(),
                n_ports,
            ))
        };
        for port in 0..n_ports {
            let port_sc = port_sc_regs.index(port).read();
            log::debug!("port_sc[{port}]: {port_sc:#X?}");
        }
        Self {
            capability_regs: capability_regs_ptr,
            operational_regs: operational_regs_ptr,
            port_sc_regs,
        }
    }

    pub fn init(
        self,
        mut periodic_frame_list_mem: MappedMem<MaybeUninit<PeriodicFrameList>>,
    ) -> InitializedEhci {
        // Halt
        log::info!("Halting eHCI");
        self.operational_regs
            .usb_cmd()
            .update(|usb_cmd| usb_cmd.with_rs(false));
        // Wait until it's stopped
        while !self.operational_regs.usb_sts().read().hc_halted() {
            spin_loop();
        }
        log::info!("Resetting eHCI");
        // Reset
        self.operational_regs
            .usb_cmd()
            .update(|usb_cmd| usb_cmd.with_hc_reset(true));
        // Wait until it's done resetting
        while self.operational_regs.usb_cmd().read().hc_reset() {
            spin_loop();
        }
        log::info!("Done resetting");

        // Initialize CTRLDSSEGMENT
        // Currently we only support memory in the lower 4 GiB
        // eHCI can optionaly support higher mem as long as it's all within a 4 GiB aligned region.
        // We could add support for this later.
        self.operational_regs.ctrl_ds_segment().write(0);
        log::info!("Initialized CTRLDSSEGMENT");

        // Initialize USBINTR
        self.operational_regs.usb_intr().update(|usb_intr| {
            usb_intr
                .with_usb_interrupt_enable(true)
                .with_usb_error_interrupt_enable(true)
                .with_port_change_interrupt_enable(true)
                .with_host_system_error_interrupt_enable(true)
        });
        log::info!("Initialized USBINTR");

        // Initialize the periodic frame list
        let periodic_frame_list = unsafe { periodic_frame_list_mem.ptr.as_mut() };
        periodic_frame_list.write(PeriodicFrameList {
            elements: [PeriodicFrameListElement::new_with_raw_value(Default::default())
                .with_t(true)
                .with_reserved(u2::ZERO); _],
        });
        log::info!("Initialized periodic frame list");
        // Initialize PERIODICLIST BASE
        self.operational_regs
            .periodic_list_base()
            .write(periodic_frame_list_mem.phys_addr.try_into().unwrap());
        log::info!("Initialized PERIODICLIST BASE");
        // Write to USBCMD to turn the host controller on
        self.operational_regs
            .usb_cmd()
            .update(|usb_cmd| usb_cmd.with_rs(true));
        log::info!("Initialized USBCMD");
        // Initialize CONFIGFLAG
        self.operational_regs
            .config_flag()
            .update(|config_flag| config_flag.with_configure_flag(true));
        log::info!("Initialized CONFIGFLAG");

        InitializedEhci {
            capability_regs: self.capability_regs,
            operational_regs: self.operational_regs,
            port_sc_regs: self.port_sc_regs,
        }
    }
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
    payload_buffer: [u8; 8],
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
                unsafe {
                    _mm_pause();
                }
            }
        }

        /// Busy loop delay for approximately 50ms on a ~2.4 GHz Arrandale CPU (~10,000,000 iterations)
        #[inline(never)]
        pub fn delay_approx_50ms() {
            // 10,000,000 iterations * ~12 cycles/iter = ~120,000,000 cycles (~50ms)
            for _ in 0..10_000_000 {
                unsafe {
                    _mm_pause();
                }
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
        delay_approx_50ms();
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
                    qtd_token: TransferToken::new_active(PidCode::InToken, u15::new(8))
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
                0x08, 0x00, // wLength: 8 bytes (initial fetch)
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
        // // Poll USB_STS for transfer complete interrupt
        // while !self.operational_regs.usb_sts().read().usb_int() {
        //     spin_loop();
        // }
        // Check first transfer
        while buffer_ptr
            .qtds()
            .as_slice()
            .index(0)
            .read()
            .qtd_token
            .active()
        {
            let usb_sts = self.operational_regs.usb_sts().read();
            log::debug!("USB status: {usb_sts:#X?}");
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
