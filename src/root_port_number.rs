use arbitrary_int::u4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RootPortNumber(u4);

impl From<RootPortNumber> for u4 {
    fn from(value: RootPortNumber) -> Self {
        value.0
    }
}

impl TryFrom<u4> for RootPortNumber {
    type Error = OutOfRange;

    fn try_from(value: u4) -> Result<Self, Self::Error> {
        if value <= u4::new(15) {
            Ok(Self(value))
        } else {
            Err(OutOfRange)
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OutOfRange;
