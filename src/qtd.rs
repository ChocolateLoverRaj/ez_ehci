use arbitrary_int::{u2, u3, u12, u20, u27};
use bitbybit::bitfield;

use crate::transfer_token::TransferToken;

#[bitfield(u32, debug)]
pub struct NextQtdPointer {
    /// 0: pointer is valid.
    /// 1: pointer is invalid.
    #[bit(0, rw)]
    terminate: bool,
    #[bits(5..=31, rw)]
    addr_upper: u27,
}

#[bitfield(u32, debug)]
pub struct QtdBufferPagePointer {
    /// Only used for the first buffer page pointer.
    /// Reserved for rest.
    #[bits(0..=11, rw)]
    current_offset: u12,
    #[bits(12..=31, rw)]
    ptr_upper: u20,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct QueueElementTransferDescriptor {
    next_qtd_ptr: NextQtdPointer,
    alternate_next_qtd_ptr: NextQtdPointer,
    qtd_token: TransferToken,
    buffer_pointer_page_0: QtdBufferPagePointer,
    buffer_pointer_page_1: QtdBufferPagePointer,
    buffer_pointer_page_2: QtdBufferPagePointer,
    buffer_pointer_page_3: QtdBufferPagePointer,
    buffer_pointer_page_4: QtdBufferPagePointer,
}
