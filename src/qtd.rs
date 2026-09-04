use arbitrary_int::{u12, u20, u27};
use bitbybit::bitfield;
use volatile::VolatileFieldAccess;

use crate::{queue_head::AlternateQtdLinkPtr, transfer_token::TransferToken};

#[bitfield(u32, debug)]
pub struct NextQtdPointer {
    /// 0: pointer is valid.
    /// 1: pointer is invalid.
    #[bit(0, rw)]
    terminate: bool,
    #[bits(5..=31, rw)]
    addr_upper: u27,
}

impl NextQtdPointer {
    pub const INVALID: Self = Self::new_with_raw_value(0).with_terminate(true);

    pub fn new_valid(addr: u32) -> Self {
        Self::new_with_raw_value(0)
            .with_terminate(false)
            .with_addr_upper(u27::new(addr >> 5))
    }
}

#[bitfield(u32, debug)]
pub struct QtdBufferPagePointerPage0 {
    #[bits(0..=11, rw)]
    pub current_offset: u12,
    #[bits(12..=31, rw)]
    pub ptr_upper: u20,
}

#[bitfield(u32, debug)]
pub struct QtdBufferPagePointerPage1Plus {
    #[bits(12..=31, rw)]
    ptr_upper: u20,
}

#[repr(C, align(32))]
#[derive(Debug, Clone, Copy, VolatileFieldAccess)]
pub struct QueueElementTransferDescriptor {
    pub(crate) next_qtd_ptr: NextQtdPointer,
    pub(crate) alternate_next_qtd_ptr: AlternateQtdLinkPtr,
    pub(crate) qtd_token: TransferToken,
    pub(crate) buffer_pointer_page_0: QtdBufferPagePointerPage0,
    pub(crate) buffer_pointer_page_1: QtdBufferPagePointerPage1Plus,
    pub(crate) buffer_pointer_page_2: QtdBufferPagePointerPage1Plus,
    pub(crate) buffer_pointer_page_3: QtdBufferPagePointerPage1Plus,
    pub(crate) buffer_pointer_page_4: QtdBufferPagePointerPage1Plus,
    // The remaining fields exist if 64-bit capable
    pub(crate) extended_buffer_ptr_page_0: u32,
    pub(crate) extended_buffer_ptr_page_1: u32,
    pub(crate) extended_buffer_ptr_page_2: u32,
    pub(crate) extended_buffer_ptr_page_3: u32,
    pub(crate) extended_buffer_ptr_page_4: u32,
}
