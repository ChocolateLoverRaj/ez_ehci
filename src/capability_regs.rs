use arbitrary_int::u4;
use bitbybit::bitfield;
use volatile::VolatileFieldAccess;

#[bitfield(u16, debug)]
pub struct HciVersion {
    #[bits(0..=7, r)]
    pub minor_revision: u8,
    #[bits(8..=15, r)]
    pub major_revision: u8,
}

#[bitfield(u32, debug)]
pub struct HcsParams {
    #[bits(0..=3, r)]
    pub n_ports: u4,
    #[bit(4, r)]
    pub port_power_control: bool,
    #[bit(7, r)]
    pub port_routing_rules: bool,
    /// Number of ports per companion controller.
    #[bits(8..=11, r)]
    pub n_pcc: u4,
    /// Number of companion controllers.
    #[bits(12..=15, r)]
    pub n_cc: u4,
    #[bit(16, r)]
    pub port_indicators: bool,
    #[bits(20..=23, r)]
    pub debug_port_number: u4,
}

#[bitfield(u32, debug)]
pub struct HccParams {
    #[bit(0, r)]
    _64_bit_addressing_cap: bool,
    #[bit(1, r)]
    programmable_frame_list_flag: bool,
    #[bit(2, r)]
    async_sched_park_cap: bool,
    #[bits(4..=7, r)]
    isochronous_sched_threshold: u4,
    #[bits(8..=15, r)]
    eecp: u8,
}

#[repr(C)]
#[derive(VolatileFieldAccess, Clone, Copy, Debug)]
pub struct CapabilityRegs {
    pub cap_len: u8,
    pub reserved: u8,
    pub hci_version: HciVersion,
    pub hcs_params: HcsParams,
    pub hcc_params: HccParams,
    pub hcsp_port_route: [u32; 2],
}
