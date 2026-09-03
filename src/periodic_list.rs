use arbitrary_int::{u2, u27};
use bitbybit::bitfield;

#[bitfield(u32, debug)]
pub struct PeriodicFrameListElement {
    /// 0: valid.
    /// 1: invalid, will not be used.
    #[bit(0, rw)]
    pub t: bool,
    #[bits(1..=2, rw)]
    pub typ: u2,
    /// Must be set to 0.
    #[bits(3..=4, rw)]
    pub reserved: u2,
    #[bits(5..=31, rw)]
    pub addr_upper_bits: u27,
}

#[repr(C, align(4096))]
#[derive(Debug, Clone, Copy)]
pub struct PeriodicFrameList {
    pub(crate) elements: [PeriodicFrameListElement; 1024],
}
