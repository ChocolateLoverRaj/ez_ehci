use arbitrary_int::{traits::Integer, u12, u20};

#[derive(Debug, Clone, Copy)]
pub struct BufferPtrs {
    ptrs: [u64; 5],
    offset: u12,
}

impl BufferPtrs {
    pub fn new_contiguous(ptr: u64, len: u64) -> Self {
        let offset = ptr % 0x1000;
        let page_0_ptr = ptr - offset;
        let end_ptr = page_0_ptr + len;
        if end_ptr > page_0_ptr + 0x1000 * 5 {
            panic!("buffer does not fit in 5 4 KiB aligned pages.")
        }
        Self {
            ptrs: [
                page_0_ptr,
                page_0_ptr + 0x1000,
                page_0_ptr + 0x1000 * 2,
                page_0_ptr + 0x1000 * 3,
                page_0_ptr + 0x1000 * 4,
            ],
            offset: u12::new(offset.try_into().unwrap()),
        }
    }

    pub const EMPTY: Self = Self {
        ptrs: [0; _],
        offset: u12::ZERO,
    };

    pub fn offset(&self) -> u12 {
        self.offset
    }

    pub fn ptr_bits_12_31(&self, index: usize) -> u20 {
        u20::new(self.ptrs[index] as u32 >> 12)
    }

    pub fn ptr_bits_32_63(&self, index: usize) -> u32 {
        (self.ptrs[index] >> 32) as u32
    }
}
