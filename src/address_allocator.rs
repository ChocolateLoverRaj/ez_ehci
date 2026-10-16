/// Allocates addresses from 1..=127.
#[derive(Debug)]
pub struct AddressAllocator {
    /// bit 0 is always marked as used because address 0 is not a valid address to allocate
    allocated_addresses: u128,
}

impl Default for AddressAllocator {
    fn default() -> Self {
        Self {
            allocated_addresses: 1 << 0,
        }
    }
}

impl AddressAllocator {
    pub fn allocate_addr(&mut self) -> Option<u8> {
        let addr = self.allocated_addresses.trailing_ones().try_into().unwrap();
        if addr <= 127 {
            self.allocated_addresses |= 1_u128 << addr;
            Some(addr)
        } else {
            None
        }
    }

    pub fn deallocate_addr(&mut self, addr: u8) {
        self.allocated_addresses &= !(1 << addr);
    }
}
