pub const PCI_CLASS: u8 = 0x0C;
pub const PCI_SUBCLASS: u8 = 0x03;
pub const PCI_PROG_IF: u8 = 0x20;

pub trait PciAccess {
    fn read_u32(&mut self, offset: u8) -> u32;
    fn write_u8(&mut self, offset: u8, value: u8);
    fn write_u16(&mut self, offset: u8, value: u16);
    fn write_u32(&mut self, offset: u8, value: u32);
}

impl<P: PciAccess> PciAccess for &mut P {
    fn read_u32(&mut self, offset: u8) -> u32 {
        P::read_u32(self, offset)
    }
    fn write_u8(&mut self, offset: u8, value: u8) {
        P::write_u8(self, offset, value)
    }
    fn write_u16(&mut self, offset: u8, value: u16) {
        P::write_u16(self, offset, value)
    }
    fn write_u32(&mut self, offset: u8, value: u32) {
        P::write_u32(self, offset, value)
    }
}
