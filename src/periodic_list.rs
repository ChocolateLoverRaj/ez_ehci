use arbitrary_int::{traits::Integer, u2, u27};
use bitbybit::bitfield;
use volatile::VolatileFieldAccess;

use crate::queue_head::SelectType;

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

impl PeriodicFrameListElement {
    pub fn new(select_type: SelectType, addr: u32) -> Self {
        Self::builder()
            .with_t(false)
            .with_typ(select_type.into())
            .with_reserved(u2::ZERO)
            .with_addr_upper_bits(u27::new(addr >> 5))
            .build()
    }
}

#[repr(C, align(4096))]
#[derive(Debug, Clone, Copy, VolatileFieldAccess)]
pub struct PeriodicFrameList {
    pub(crate) elements: [PeriodicFrameListElement; 1024],
}
