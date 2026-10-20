use crate::start_config::CtrlDsSegment;

/// Panics if the phys addr is outside of ctrl ds segment
pub fn get_addr_offset(phys_addr: u64, ctrl_ds_segment: CtrlDsSegment) -> u32 {
    let upper = (phys_addr >> 32) as u32;
    let lower = phys_addr as u32;
    if upper != ctrl_ds_segment.base_addr_upper {
        panic!("phys addr is outside of the range of CTRLDSSEGMENT");
    }
    lower
}
