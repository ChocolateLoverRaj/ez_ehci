use arbitrary_int::{u12, u20, u27};
use bitbybit::bitfield;
use volatile::VolatileFieldAccess;

use crate::{
    buffer_ptrs::BufferPtrs, queue_head::AlternateQtdLinkPtr, transfer_token::TransferToken,
};

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

impl QueueElementTransferDescriptor {
    pub fn new(qtd_token: TransferToken, buffer_ptrs: BufferPtrs) -> Self {
        Self {
            next_qtd_ptr: NextQtdPointer::INVALID,
            alternate_next_qtd_ptr: AlternateQtdLinkPtr::INVALID,
            qtd_token,
            buffer_pointer_page_0: QtdBufferPagePointerPage0::new_with_raw_value(0)
                .with_current_offset(buffer_ptrs.offset())
                .with_ptr_upper(buffer_ptrs.ptr_bits_12_31(0)),
            buffer_pointer_page_1: QtdBufferPagePointerPage1Plus::new_with_raw_value(0)
                .with_ptr_upper(buffer_ptrs.ptr_bits_12_31(1)),
            buffer_pointer_page_2: QtdBufferPagePointerPage1Plus::new_with_raw_value(0)
                .with_ptr_upper(buffer_ptrs.ptr_bits_12_31(2)),
            buffer_pointer_page_3: QtdBufferPagePointerPage1Plus::new_with_raw_value(0)
                .with_ptr_upper(buffer_ptrs.ptr_bits_12_31(3)),
            buffer_pointer_page_4: QtdBufferPagePointerPage1Plus::new_with_raw_value(0)
                .with_ptr_upper(buffer_ptrs.ptr_bits_12_31(4)),
            extended_buffer_ptr_page_0: buffer_ptrs.ptr_bits_32_63(0),
            extended_buffer_ptr_page_1: buffer_ptrs.ptr_bits_32_63(1),
            extended_buffer_ptr_page_2: buffer_ptrs.ptr_bits_32_63(2),
            extended_buffer_ptr_page_3: buffer_ptrs.ptr_bits_32_63(3),
            extended_buffer_ptr_page_4: buffer_ptrs.ptr_bits_32_63(4),
        }
    }
}
