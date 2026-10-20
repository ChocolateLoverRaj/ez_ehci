use crate::{
    MappedMem, PeriodicFrameList, async_list_manager::QhWithMetaA, poll_strategy::PollStrategy,
};

#[derive(Debug, Clone, Copy)]
pub struct StartConfig {
    pub stop_strategy: PollStrategy,
    pub reset_strategy: PollStrategy,
    pub ctrl_ds_segment: CtrlDsSegment,
    pub periodic_frame_list_mem: MappedMem<PeriodicFrameList>,
    pub qh: MappedMem<QhWithMetaA>,
}

#[derive(Debug, Clone, Copy)]
pub struct CtrlDsSegment {
    pub(crate) base_addr_upper: u32,
}

impl CtrlDsSegment {
    pub const LOWER_4_G: Self = Self { base_addr_upper: 0 };

    /// Must be a multiple of 4 GiB.
    pub fn new(base_phys_addr: u64) -> Self {
        let upper = (base_phys_addr >> 32) as u32;
        let lower = base_phys_addr as u32;
        if lower != 0 {
            panic!("base addr must be a multiple of 4 GiB");
        }
        Self {
            base_addr_upper: upper,
        }
    }
}
