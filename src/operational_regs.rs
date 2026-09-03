use arbitrary_int::{u2, u4};
use bitbybit::bitfield;
use debug_ignore::DebugIgnore;
use volatile::VolatileFieldAccess;

#[bitfield(u32, debug)]
pub struct UsbCmdReg {
    /// 0 - stop.
    /// 1 - run.
    #[bit(0, rw)]
    pub rs: bool,
    /// write 1 to reset, then wait for value to become 0, which indicates it's done resetting.
    #[bit(1, rw)]
    pub hc_reset: bool,
}

#[bitfield(u32, debug)]
pub struct UsbStsReg {
    #[bit(0, rw)]
    pub usb_int: bool,
    #[bit(1, rw)]
    pub usb_err_int: bool,
    #[bit(2, rw)]
    pub port_change_detect: bool,
    #[bit(3, rw)]
    pub frame_list_rollover: bool,
    #[bit(4, rw)]
    pub host_system_error: bool,
    #[bit(12, r)]
    pub hc_halted: bool,
    #[bit(14, r)]
    pub periodic_schedule_status: bool,
    #[bit(15, r)]
    pub async_schedule_status: bool,
}

#[bitfield(u32, debug)]
pub struct UsbIntrReg {
    #[bit(0, rw)]
    pub usb_interrupt_enable: bool,
    #[bit(1, rw)]
    pub usb_error_interrupt_enable: bool,
    #[bit(2, rw)]
    pub port_change_interrupt_enable: bool,
    #[bit(3, rw)]
    pub frame_list_rollover_interrupt_enable: bool,
    #[bit(4, rw)]
    pub host_system_error_interrupt_enable: bool,
    #[bit(5, rw)]
    pub interrupt_on_async_advance_enable: bool,
}

#[bitfield(u32, debug)]
pub struct UsbFrIndexReg {}

#[bitfield(u32, debug)]
pub struct AsyncListAddrReg {}

#[bitfield(u32, debug)]
pub struct ConfigFlagReg {
    /// 0: ports routed to oHCI / uHCI controllers by default.
    /// 1: ports routed to this controller by default.
    #[bit(0, rw)]
    pub configure_flag: bool,
}

#[bitfield(u32, debug)]
pub struct PortScReg {
    #[bit(0, r)]
    pub current_connect_status: bool,
    #[bit(1, rw)]
    pub connect_status_change: bool,
    #[bit(2, rw)]
    pub port_enabled: bool,
    #[bit(3, rw)]
    pub port_enable_change: bool,
    #[bit(4, r)]
    pub over_current_active: bool,
    #[bit(5, rw)]
    pub over_current_change: bool,
    #[bit(6, rw)]
    pub force_port_resume: bool,
    #[bit(7, rw)]
    pub suspend: bool,
    #[bit(8, rw)]
    pub port_reset: bool,
    #[bits(10..=11, r)]
    pub line_status: u2,
    #[bit(12, rw)]
    pub port_power: bool,
    #[bit(13, rw)]
    pub port_owner: bool,
    #[bits(14..=15, rw)]
    pub port_indicator_control: u2,
    #[bits(16..=19, rw)]
    pub port_test_control: u4,
    #[bit(20, rw)]
    pub wake_on_connect_enable: bool,
    #[bit(21, rw)]
    pub wake_on_disconnect_enable: bool,
    #[bit(22, rw)]
    pub wake_on_over_current_enable: bool,
}

#[repr(C)]
#[derive(VolatileFieldAccess, Clone, Copy, Debug)]
pub struct OperationalRegs {
    pub usb_cmd: UsbCmdReg,
    pub usb_sts: UsbStsReg,
    pub usb_intr: UsbIntrReg,
    pub fr_index: UsbFrIndexReg,
    pub ctrl_ds_segment: u32,
    pub periodic_list_base: u32,
    pub async_list_addr: AsyncListAddrReg,
    pub reserved: DebugIgnore<[u8; 36]>,
    pub config_flag: ConfigFlagReg,
    /// The size of this depends on the number of ports at runtime.
    pub port_sc: DebugIgnore<[PortScReg; 0]>,
}
