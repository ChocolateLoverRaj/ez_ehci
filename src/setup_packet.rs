use arbitrary_int::{u2, u5, u7};
use bitbybit::bitfield;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use zerocopy::IntoBytes;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, IntoBytes)]
pub struct SetupPacket {
    bm_request_type: u8,
    b_request: u8,
    w_value: [u8; 2],
    w_index: [u8; 2],
    w_length: [u8; 2],
}

impl SetupPacket {
    pub fn new_get_descriptor(w_length: u16) -> Self {
        Self {
            bm_request_type: BmRequestType::new_with_raw_value(0)
                .with_recipient(Recipient::Device.into())
                .with_request_type(RequestType::Standard.into())
                .with_direction(Direction::DeviceToHost.into())
                .raw_value(),
            b_request: BRequest::GetDescriptor.into(),
            w_value: [0x00, DescriptorType::Device.into()],
            w_index: 0_u16.to_le_bytes(),
            w_length: w_length.to_le_bytes(),
        }
    }

    pub fn new_set_address(new_address: u7) -> Self {
        Self {
            bm_request_type: BmRequestType::new_with_raw_value(0)
                .with_recipient(Recipient::Device.into())
                .with_request_type(RequestType::Standard.into())
                .with_direction(Direction::HostToDeviceOrNoDataTransfer.into())
                .raw_value(),
            b_request: BRequest::SetAddress.into(),
            w_value: u16::from(new_address).to_le_bytes(),
            w_index: 0_u16.to_le_bytes(),
            w_length: 0_u16.to_le_bytes(),
        }
    }
}

#[bitfield(u8, debug)]
#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub struct BmRequestType {
    #[bits(0..=4, rw)]
    recipient: u5,
    #[bits(5..=6, rw)]
    request_type: u2,
    #[bit(7, rw)]
    direction: bool,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, IntoPrimitive, TryFromPrimitive)]
pub enum Recipient {
    Device,
    Interface,
    Endpoint,
    Other,
}
impl From<Recipient> for u5 {
    fn from(value: Recipient) -> Self {
        Self::new(value.into())
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, IntoPrimitive, TryFromPrimitive)]
pub enum RequestType {
    Standard,
    Class,
    Vendor,
}
impl From<RequestType> for u2 {
    fn from(value: RequestType) -> Self {
        Self::new(value.into())
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, IntoPrimitive, TryFromPrimitive)]
pub enum Direction {
    HostToDeviceOrNoDataTransfer,
    DeviceToHost,
}
impl From<Direction> for bool {
    fn from(value: Direction) -> Self {
        matches!(value, Direction::DeviceToHost)
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, IntoPrimitive, TryFromPrimitive)]
pub enum BRequest {
    GetStatus = 0,
    ClearFeature = 1,
    SetFeature = 3,
    SetAddress = 5,
    GetDescriptor = 6,
    SetDescriptor = 7,
    GetConfiguration = 8,
    SetConfiguration = 9,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, IntoPrimitive, TryFromPrimitive)]
pub enum DescriptorType {
    Device = 1,
}
