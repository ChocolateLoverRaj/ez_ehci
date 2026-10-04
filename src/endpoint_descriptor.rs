use arbitrary_int::{u2, u4, u11};
use bitbybit::bitfield;
use zerocopy::{FromBytes, Immutable, KnownLayout, little_endian::U16};

#[repr(C)]
#[derive(Debug, Clone, Copy, FromBytes, Immutable, KnownLayout)]
pub struct EndpointDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_endpoint_address: u8,
    pub bm_attributes: u8,
    pub w_max_packet_size: U16,
    /// For high speed endpoints, the unit is in microframes.
    /// For low and full speed endpoints, the unit is in frames.
    ///
    /// For isochronous endpoints, this number is within 1..=16 and the actual interval is 2^(n-1).
    ///
    /// For low and full speed interrupt endpoints, this number is within 1..=255 and the actual interval is n.
    ///
    /// For high speed interrupt endpoints, this number is within 1..=16 and the actual interval is 2^(n-1).
    ///
    /// For high-speed bulk/control OUT endpoints, the
    /// bInterval must specify the maximum NAK rate of the
    /// endpoint. A value of 0 indicates the endpoint never
    /// NAKs. Other values indicate at most 1 NAK each
    /// bInterval number of microframes. This value must be
    /// in the range from 0 to 255.
    pub b_interval: u8,
}

#[bitfield(u8, debug)]
pub struct EndpointAddress {
    #[bits(0..=3, rw)]
    pub endpoint_number: u4,
    #[bit(7, rw)]
    pub direction: bool,
}

#[bitfield(u8, debug)]
pub struct EndpointAttributes {
    #[bits(0..=1, rw)]
    pub transfer_type: u2,
    /// Only valid for isochronous endpoints.
    #[bits(2..=3, rw)]
    pub synchronization_type: u2,
    /// Only valid for isochronous endpoints.
    #[bits(4..=5, rw)]
    pub usage_type: u2,
}

#[bitfield(u16, debug)]
pub struct EndpointMaxPacketSize {
    #[bits(0..=10, rw)]
    pub max_packet_size: u11,
    /// Only applies to isochronous endpoints, range is 0..=2. 3 is reserved.
    #[bits(11..=12, rw)]
    pub additional_transaction_opportunities_per_microframe: u2,
}
