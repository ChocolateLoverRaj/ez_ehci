#![no_std]

use core::{fmt::Debug, hint::spin_loop, num::NonZero, ptr::NonNull};

use arbitrary_int::{u2, u4, u27};
use bitbybit::bitfield;
use debug_ignore::DebugIgnore;
use volatile::{VolatileFieldAccess, VolatilePtr, access::ReadOnly};

pub const PCI_CLASS: u8 = 0x0C;
pub const PCI_SUBCLASS: u8 = 0x03;
pub const PCI_PROG_IF: u8 = 0x20;

#[bitfield(u16, debug)]
struct HciVersion {
    #[bits(0..=7, r)]
    pub minor_revision: u8,
    #[bits(8..=15, r)]
    pub major_revision: u8,
}

#[bitfield(u32, debug)]
struct HcsParams {
    #[bits(0..=3, r)]
    n_ports: u4,
    #[bit(4, r)]
    port_power_control: bool,
    #[bit(7, r)]
    port_routing_rules: bool,
    /// Number of ports per companion controller.
    #[bits(8..=11, r)]
    n_pcc: u4,
    /// Number of companion controllers.
    #[bits(12..=15, r)]
    n_cc: u4,
    #[bit(16, r)]
    port_indicators: bool,
    #[bits(20..=23, r)]
    debug_port_number: u4,
}

#[bitfield(u32, debug)]
struct HccParams {
    #[bit(0, r)]
    _64_bit_addressing_cap: bool,
    #[bit(1, r)]
    programmable_frame_list_flag: bool,
    #[bit(2, r)]
    async_sched_park_cap: bool,
    #[bits(4..=7, r)]
    isochronous_sched_threshold: u4,
    #[bits(8..=15, r)]
    eecp: u8,
}

#[repr(C)]
#[derive(VolatileFieldAccess, Clone, Copy, Debug)]
struct CapabilityRegs {
    cap_len: u8,
    reserved: u8,
    hci_version: HciVersion,
    hcs_params: HcsParams,
    hcc_params: HccParams,
    hcsp_port_route: [u32; 2],
}

#[bitfield(u32, debug)]
struct UsbCmdReg {
    /// 0 - stop.
    /// 1 - run.
    #[bit(0, rw)]
    rs: bool,
    /// write 1 to reset, then wait for value to become 0, which indicates it's done resetting.
    #[bit(1, rw)]
    hc_reset: bool,
}

#[bitfield(u32, debug)]
struct UsbStsReg {
    #[bit(0, rw)]
    usb_int: bool,
    #[bit(1, rw)]
    usb_err_int: bool,
    #[bit(2, rw)]
    port_change_detect: bool,
    #[bit(3, rw)]
    frame_list_rollover: bool,
    #[bit(4, rw)]
    host_system_error: bool,
    #[bit(12, r)]
    hc_halted: bool,
    #[bit(14, r)]
    periodic_schedule_status: bool,
    #[bit(15, r)]
    async_schedule_status: bool,
}

#[bitfield(u32, debug)]
struct UsbIntrReg {
    #[bit(0, rw)]
    usb_interrupt_enable: bool,
    #[bit(1, rw)]
    usb_error_interrupt_enable: bool,
    #[bit(2, rw)]
    port_change_interrupt_enable: bool,
    #[bit(3, rw)]
    frame_list_rollover_interrupt_enable: bool,
    #[bit(4, rw)]
    host_system_error_interrupt_enable: bool,
    #[bit(5, rw)]
    interrupt_on_async_advance_enable: bool,
}

#[bitfield(u32, debug)]
struct UsbFrIndexReg {}

#[bitfield(u32, debug)]
struct AsyncListAddrReg {}

#[bitfield(u32, debug)]
struct ConfigFlagReg {
    /// 0: ports routed to oHCI / uHCI controllers by default.
    /// 1: ports routed to this controller by default.
    #[bit(0, rw)]
    configure_flag: bool,
}

#[bitfield(u32, debug)]
struct PortScReg {
    #[bit(0, r)]
    current_connect_status: bool,
    #[bit(1, rw)]
    connect_status_change: bool,
    #[bit(2, rw)]
    port_enabled: bool,
    #[bit(3, rw)]
    port_enable_change: bool,
    #[bit(4, r)]
    over_current_active: bool,
    #[bit(5, rw)]
    over_current_change: bool,
    #[bit(6, rw)]
    force_port_resume: bool,
    #[bit(7, rw)]
    suspend: bool,
    #[bit(8, rw)]
    port_reset: bool,
    #[bits(10..=11, r)]
    line_status: u2,
    #[bit(12, rw)]
    port_power: bool,
    #[bit(13, rw)]
    port_owner: bool,
    #[bits(14..=15, rw)]
    port_indicator_control: u2,
    #[bits(16..=19, rw)]
    port_test_control: u4,
    #[bit(20, rw)]
    wake_on_connect_enable: bool,
    #[bit(21, rw)]
    wake_on_disconnect_enable: bool,
    #[bit(22, rw)]
    wake_on_over_current_enable: bool,
}

#[repr(C)]
#[derive(VolatileFieldAccess, Clone, Copy, Debug)]
struct OperationalRegs {
    usb_cmd: UsbCmdReg,
    usb_sts: UsbStsReg,
    usb_intr: UsbIntrReg,
    fr_index: UsbFrIndexReg,
    ctrl_ds_segment: u32,
    periodic_list_base: u32,
    async_list_addr: AsyncListAddrReg,
    reserved: DebugIgnore<[u8; 36]>,
    config_flag: ConfigFlagReg,
    /// The size of this depends on the number of ports at runtime.
    port_sc: DebugIgnore<[PortScReg; 0]>,
}

#[bitfield(u32, debug)]
struct PeriodicFrameListElement {
    /// 0: valid.
    /// 1: invalid, will not be used.
    #[bit(0, rw)]
    t: bool,
    #[bits(1..=2, rw)]
    typ: u2,
    /// Must be set to 0.
    #[bits(3..=4, rw)]
    reserved: u2,
    #[bits(5..=31, rw)]
    addr_upper_bits: u27,
}

#[derive(Debug, Clone, Copy)]
pub struct MappedMem {
    pub phys_addr: u64,
    pub virt: NonNull<u8>,
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
}

impl<P: PciAccess> PciAccess for &mut P {
    fn read_u32(&mut self, offset: u8) -> u32 {
        P::read_u32(self, offset)
    }
    fn write_u8(&mut self, offset: u8, value: u8) {
        P::write_u8(self, offset, value)
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

    pub fn init(self) -> InitializedEhci {
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

        // Later when we want isochronous transfers
        // Initialize CTRLDSSEGMENT
        // self.operational_regs.ctrl_ds_segment().write(
        //     (self.periodic_frame_list_phys_addr_lower)
        //         .try_into()
        //         .unwrap(),
        // );
        // log::info!("Initialized CTRLDSSEGMENT");

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
        // self.periodic_frame_list.write(
        //     [PeriodicFrameListElement::new_with_raw_value(Default::default())
        //         .with_t(true)
        //         .with_reserved(u2::ZERO); _],
        // );
        // log::info!("Initialized periodic frame list");
        // // Initialize PERIODICLIST BASE
        // self.operational_regs
        //     .periodic_list_base()
        //     .write(self.periodic_frame_list_phys_addr_lower);
        // log::info!("Initialized PERIODICLIST BASE");
        // Write to USBCMD to turn the host controller on

        self.operational_regs
            .usb_cmd()
            .update(|usb_cmd| usb_cmd.with_rs(true));
        log::info!("Initialized USBCMD");
        // Initialize CONFIGFLAG
        self.operational_regs.config_flag().update(|config_flag| {
            log::info!("Read CONNFIGFLAG");
            config_flag.with_configure_flag(true)
        });
        // MMIO Read-Back Flush: Reads from the register to force the PCI bridge
        // to complete the posted write before downstream execution continues.
        let _ = self.operational_regs.config_flag().read();
        log::info!("Initialized CONFIGFLAG");

        InitializedEhci {
            capability_regs: self.capability_regs,
            operational_regs: self.operational_regs,
            port_sc_regs: self.port_sc_regs,
        }
    }

    // pub fn setup_device(&mut self, device: RootPortNumber) {}
}

pub struct InitializedEhci {
    capability_regs: VolatilePtr<'static, CapabilityRegs, ReadOnly>,
    operational_regs: VolatilePtr<'static, OperationalRegs>,
    port_sc_regs: VolatilePtr<'static, [PortScReg]>,
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
    /// Represents an offset in bytes from the end of the capability regs.
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
