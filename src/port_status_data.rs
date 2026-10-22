use bitbybit::bitfield;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, little_endian::U16};

use crate::device_speed::DeviceSpeed;

#[repr(C)]
#[derive(Debug, Immutable, FromBytes, IntoBytes, KnownLayout)]
pub struct PortStatusData {
    pub w_port_status: U16,
    pub w_port_change: U16,
}

#[bitfield(u16, debug)]
pub struct PortStatus {
    #[bit(0, rw)]
    pub current_connect_status: bool,
    #[bit(1, rw)]
    pub port_enable: bool,
    #[bit(2, rw)]
    pub suspend: bool,
    #[bit(3, rw)]
    pub over_current: bool,
    #[bit(4, rw)]
    pub reset: bool,
    #[bit(8, rw)]
    pub port_power: bool,
    /// This is relevant only if a device is attached.
    /// 0 = Full-speed or High-speed device attached to this port (determined by bit 10).
    /// 1 = Low-speed device attached to this port.
    #[bit(9, rw)]
    pub low_speed_device_attached: bool,
    /// This is relevant only if a device is attached.
    /// 0 = Full-speed device attached to this port.
    /// 1 = High-speed device attached to this port.
    #[bit(10, rw)]
    pub high_speed_device_attached: bool,
    #[bit(11, rw)]
    pub port_test_mode: bool,
    #[bit(12, rw)]
    pub port_indicator_control: bool,
}

impl PortStatus {
    pub fn attached_device_speed(&self) -> Option<DeviceSpeed> {
        if self.port_enable() {
            Some(
                match (
                    self.low_speed_device_attached(),
                    self.high_speed_device_attached(),
                ) {
                    (true, _) => DeviceSpeed::Low,
                    (false, false) => DeviceSpeed::Full,
                    (false, true) => DeviceSpeed::High,
                },
            )
        } else {
            None
        }
    }
}

#[bitfield(u16, debug)]
pub struct PortChange {
    #[bit(0, rw)]
    pub connect_status_change: bool,
    #[bit(1, rw)]
    pub port_enable_change: bool,
    #[bit(2, rw)]
    pub port_suspend_change: bool,
    #[bit(3, rw)]
    pub over_current_indicator_change: bool,
    #[bit(4, rw)]
    pub port_reset_change: bool,
}
