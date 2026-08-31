#![no_std]

use core::{fmt::Debug, hint::spin_loop, ptr::NonNull};

use arbitrary_int::{traits::Integer, u2, u4, u27};
use bitbybit::bitfield;
use debug_ignore::DebugIgnore;
use volatile::{VolatileFieldAccess, VolatilePtr};

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
    hcsp_port_route: u64,
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

pub struct Driver {
    capability_regs: VolatilePtr<'static, CapabilityRegs>,
    operational_regs: VolatilePtr<'static, OperationalRegs>,
    port_sc_regs: VolatilePtr<'static, [PortScReg]>,
    periodic_frame_list_phys_addr_lower: u32,
    periodic_frame_list: VolatilePtr<'static, [PeriodicFrameListElement; 1024]>,
}

impl Driver {
    /// # Safety
    /// The bar must be the virtual address pointing to the bar. The entire BAR 0 must be mapped with the right caching type.
    /// The periodic frame list must be 0x1000 bytes and mapped as strong uncacheable.
    pub unsafe fn new(
        bar: NonNull<u8>,
        periodic_frame_list_phys_addr: u64,
        periodic_frame_list: NonNull<[u32; 1024]>,
    ) -> Self {
        let capability_regs = unsafe { VolatilePtr::<CapabilityRegs>::new(bar.cast()) };
        let c = capability_regs.read();
        log::debug!("capability regs: {c:#X?}");
        let operational_regs =
            unsafe { VolatilePtr::new(bar.byte_offset(c.cap_len.try_into().unwrap()).cast()) };
        let o = operational_regs.read();
        log::debug!("operational regs: {o:#X?}");
        let n_ports = c.hcs_params.n_ports().value().try_into().unwrap();
        let port_sc_regs = unsafe {
            VolatilePtr::new(NonNull::slice_from_raw_parts(
                operational_regs.port_sc().as_raw_ptr().cast(),
                n_ports,
            ))
        };
        for port in 0..n_ports {
            let port_sc = port_sc_regs.index(port).read();
            log::debug!("port_sc[{port}]: {port_sc:#X?}");
        }
        Self {
            capability_regs,
            operational_regs,
            port_sc_regs,
            periodic_frame_list_phys_addr_lower: if c.hcc_params._64_bit_addressing_cap() {
                periodic_frame_list_phys_addr as u32
            } else {
                periodic_frame_list_phys_addr.try_into().unwrap()
            },
            periodic_frame_list: unsafe { VolatilePtr::new(periodic_frame_list.cast()) },
        }
    }

    pub fn run(&mut self) {
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
        self.operational_regs.ctrl_ds_segment().write(
            (self.periodic_frame_list_phys_addr_lower)
                .try_into()
                .unwrap(),
        );
        // Initialize USBINTR
        self.operational_regs.usb_intr().update(|usb_intr| {
            usb_intr
                .with_usb_interrupt_enable(true)
                .with_usb_error_interrupt_enable(true)
                .with_port_change_interrupt_enable(true)
                .with_host_system_error_interrupt_enable(true)
        });
        // Initialize the periodic frame list
        self.periodic_frame_list.write(
            [PeriodicFrameListElement::new_with_raw_value(Default::default())
                .with_t(true)
                .with_reserved(u2::ZERO); _],
        );
        // Initialize PERIODICLIST BASE
        self.operational_regs
            .periodic_list_base()
            .write(self.periodic_frame_list_phys_addr_lower);
        // Write to USBCMD to turn the host controller on
        self.operational_regs
            .usb_cmd()
            .update(|usb_cmd| usb_cmd.with_rs(true));
        // Initialize CONFIGFLAG
        self.operational_regs
            .config_flag()
            .update(|config_flag| config_flag.with_configure_flag(true));
    }
}
