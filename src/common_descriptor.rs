use zerocopy::{FromBytes, Immutable, KnownLayout};

#[repr(C)]
#[derive(Debug, Clone, Copy, FromBytes, Immutable, KnownLayout)]
pub struct CommonDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
}
