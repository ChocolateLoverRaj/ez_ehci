use arbitrary_int::{u2, u4, u7, u11, u12, u20, u27};
use bitbybit::bitfield;
use volatile::VolatileFieldAccess;

use crate::{qtd::NextQtdPointer, transfer_token::TransferToken};

#[bitfield(u32, debug)]
pub struct QueueHeadHorizontalLinkPtr {
    #[bit(0, rw)]
    terminate: bool,
    #[bits(1..=2, rw)]
    pointer_type: u2,
    #[bits(5..=31, rw)]
    ptr_upper: u27,
}

impl QueueHeadHorizontalLinkPtr {
    pub const INVALID: Self = Self::new_with_raw_value(0).with_terminate(true);

    pub fn new(select_type: SelectType, ptr: u32) -> Self {
        Self::new_with_raw_value(ptr)
            .with_terminate(false)
            .with_pointer_type(select_type.into())
            .with_ptr_upper(u27::new(ptr >> 5))
    }
}

#[repr(u8)]
pub enum SelectType {
    Itd,
    Qh,
    SplitItd,
    Fstn,
}

impl From<SelectType> for u2 {
    fn from(value: SelectType) -> Self {
        Self::new(value as u8)
    }
}

#[bitfield(u32, debug)]
pub struct EndpointCharacteristics {
    #[bits(0..=6, rw)]
    pub device_addr: u7,
    #[bit(7, rw)]
    pub inactive_on_next_transaction: bool,
    #[bits(8..=11, rw)]
    pub endpoint_number: u4,
    #[bits(12..=13, rw)]
    pub endpoint_speed: u2,
    #[bit(14, rw)]
    pub data_toggle_control: bool,
    #[bit(15, rw)]
    pub head_of_reclamation_list_flag: bool,
    #[bits(16..=26, rw)]
    pub max_packet_len: u11,
    #[bit(27, rw)]
    pub endpoint_control_flag: bool,
    #[bits(28..=31, rw)]
    pub nak_count_reload: u4,
}

#[bitfield(u32, debug)]
pub struct EndpointCapabilities {
    #[bits(0..=7, rw)]
    interrupt_schedule_mask: u8,
    #[bits(8..=15, rw)]
    split_completion_mask: u8,
    #[bits(16..=22, rw)]
    hub_addr: u7,
    #[bits(23..=29, rw)]
    port_number: u7,
    #[bits(30..=31, rw)]
    high_bandwidth_pipe_multiplier: u2,
}

#[bitfield(u32, debug)]
pub struct CurrentQtdLinkPtr {
    #[bits(5..=31, rw)]
    ptr_upper: u27,
}

impl CurrentQtdLinkPtr {
    pub fn new(ptr: u32) -> Self {
        Self::new_with_raw_value(0).with_ptr_upper(u27::new(ptr >> 5))
    }
}

#[bitfield(u32, debug)]
pub struct AlternateQtdLinkPtr {
    #[bit(0, rw)]
    terminate: bool,
    #[bits(1..=4, rw)]
    nak_count: u4,
    #[bits(5..=31, rw)]
    ptr_upper: u27,
}

impl AlternateQtdLinkPtr {
    pub const INVALID: Self = Self::new_with_raw_value(0).with_terminate(true);
}

#[bitfield(u32, debug)]
pub struct QhBufferPtrPage0 {
    #[bits(0..=11, rw)]
    current_offset: u12,
    #[bits(12..=31, rw)]
    ptr_upper: u20,
}

impl QhBufferPtrPage0 {}

#[bitfield(u32, debug)]
pub struct QhBufferPtrPage1 {
    #[bits(12..=31, rw)]
    ptr_upper: u20,
}

#[bitfield(u32, debug)]
pub struct QhBufferPtrPage2 {
    #[bits(12..=31, rw)]
    ptr_upper: u20,
}

#[bitfield(u32, debug)]
pub struct QhBufferPtrPage3P {
    #[bits(12..=31, rw)]
    ptr_upper: u20,
}

#[repr(C, align(32))]
#[derive(Debug, Clone, Copy, VolatileFieldAccess)]
pub struct QueueHead {
    pub(crate) queue_head_horizontal_link_ptr: QueueHeadHorizontalLinkPtr,
    pub(crate) endpoint_charactersistics: EndpointCharacteristics,
    pub(crate) endpoint_capabilities: EndpointCapabilities,
    pub(crate) current_qtd_pointer: CurrentQtdLinkPtr,
    pub(crate) next_qtd_pointer: NextQtdPointer,
    pub(crate) alternate_qtd_pointer: AlternateQtdLinkPtr,
    pub(crate) transfer_token: TransferToken,
    pub(crate) buffer_ptr_page_0: QhBufferPtrPage0,
    pub(crate) buffer_ptr_page_1: QhBufferPtrPage1,
    pub(crate) buffer_ptr_page_2: QhBufferPtrPage2,
    pub(crate) buffer_ptr_page_3: QhBufferPtrPage3P,
    pub(crate) buffer_ptr_page_4: QhBufferPtrPage3P,
    // Rest of fields exist if 64-bit capable
    pub(crate) extended_buffer_ptr_page_0: u32,
    pub(crate) extended_buffer_ptr_page_1: u32,
    pub(crate) extended_buffer_ptr_page_2: u32,
    pub(crate) extended_buffer_ptr_page_3: u32,
    pub(crate) extended_buffer_ptr_page_4: u32,
}
