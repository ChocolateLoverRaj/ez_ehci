use arbitrary_int::{u2, u4, u7, u12, u20, u27};
use bitbybit::bitfield;

use crate::transfer_token::TransferToken;

#[bitfield(u32, debug)]
pub struct QueueHeadHorizontalLinkPtr {
    #[bit(0, rw)]
    terminate: bool,
    #[bits(1..=2, rw)]
    pointer_type: u2,
    #[bits(5..=31, rw)]
    ptr_upper: u27,
}

#[bitfield(u32, debug)]
pub struct EndpointCharacteristics {
    #[bits(0..=6)]
    device_addr: u7,
    #[bit(7, rw)]
    inactive_on_next_transaction: bool,
    #[bits(8..=11, rw)]
    endpoint_number: u4,
    #[bits(12..=13, rw)]
    endpoint_speed: u2,
    #[bit(14, rw)]
    data_toggle_control: bool,
    #[bit(15, rw)]
    head_of_reclamation_list_flag: bool,
    #[bits(16..=26)]
    max_packet_len: u11,
    #[bit(27, rw)]
    endpoint_control_flag: bool,
    #[bits(28..=31, rw)]
    nak_count_reload: u4,
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
    pot_number: u7,
    #[bits(30..=31, rw)]
    high_bandwidth_pipe_multiplier: u2,
}

#[bitfield(u32, debug)]
pub struct CurrentQtdLinkPtr {
    #[bits(5..=31, rw)]
    ptr_upper: u27,
}

#[bitfield(u32, debug)]
pub struct NextQtdLinkPtr {
    #[bit(0, rw)]
    terminate: bool,
    #[bits(5..=31, rw)]
    ptr_upper: u27,
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

#[bitfield(u32, debug)]
pub struct QhBufferPtrPage0 {
    #[bits(0..=11, rw)]
    current_offset: u12,
    #[bits(12..=31, rw)]
    ptr_upper: u20,
}

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
pub struct QhBufferPtrPage3 {
    #[bits(12..=31, rw)]
    ptr_upper: u20,
}

#[bitfield(u32, debug)]
pub struct QhBufferPtrPage4 {
    #[bits(12..=31, rw)]
    ptr_upper: u20,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct QueueHead {
    queue_head_horizontal_link_ptr: QueueHeadHorizontalLinkPtr,
    endpoint_charactersistics: EndpointCharacteristics,
    endpoint_capabilities: EndpointCapabilities,
    current_qtd_pointer: CurrentQtdLinkPtr,
    next_qtd_pointer: NextQtdLinkPtr,
    alternate_qtd_pointer: AlternateQtdLinkPtr,
    transfer_token: TransferToken,
    buffer_ptr_page_0: QhBufferPtrPage0,
    buffer_ptr_page_1: QhBufferPtrPage1,
    buffer_ptr_page_2: QhBufferPtrPage2,
    buffer_ptr_page_3: QhBufferPtrPage3,
    buffer_ptr_page_4: QhBufferPtrPage4,
}
