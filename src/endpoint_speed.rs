use arbitrary_int::u2;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EndpointSpeed {
    Full,
    Low,
    High,
}

impl From<EndpointSpeed> for u2 {
    fn from(value: EndpointSpeed) -> Self {
        Self::new(value as u8)
    }
}
