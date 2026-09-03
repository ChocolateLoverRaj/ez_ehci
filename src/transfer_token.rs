use arbitrary_int::{u2, u3, u15};
use bitbybit::bitfield;

#[bitfield(u32, debug)]
pub struct TransferToken {
    #[bit(0, rw)]
    ping_state: bool,
    #[bit(1, rw)]
    split_transaction_state: bool,
    #[bit(2, rw)]
    missed_micro_frames: bool,
    #[bit(3, rw)]
    transaction_error: bool,
    #[bit(4, rw)]
    babble_detected: bool,
    #[bit(5, rw)]
    data_buffer_error: bool,
    #[bit(6, rw)]
    halted: bool,
    #[bit(7, rw)]
    active: bool,
    #[bits(8..=9, rw)]
    pid_code: u2,
    #[bits(10..=11, rw)]
    error_counter: u2,
    #[bits(12..=14, rw)]
    current_page: u3,
    #[bit(15, rw)]
    interrupt_on_complete: bool,
    #[bits(16..=30, rw)]
    total_bytes_to_transfer: u15,
    #[bit(31, rw)]
    data_toggle: bool,
}

impl TransferToken {
    // pub fn new_active(pid_code: PidCode) -> Self {
    //     Self::new_with_raw_value(0)
    //         .with_active(true)
    //         .with_pid_code(pid_code.into())
    // }

    pub fn new_active(pid_code: PidCode, total_bytes_to_transfer: u15) -> Self {
        Self::new_with_raw_value(0)
            .with_active(true)
            .with_pid_code(pid_code.into())
            .with_total_bytes_to_transfer(total_bytes_to_transfer)
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PidCode {
    OutToken,
    InToken,
    SetupToken,
}

impl From<PidCode> for u2 {
    fn from(value: PidCode) -> Self {
        Self::new(value as u8)
    }
}
