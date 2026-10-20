use core::ptr::NonNull;

#[derive(Debug)]
pub struct MappedMem<T: ?Sized> {
    pub phys_addr: u64,
    pub ptr: NonNull<T>,
}

impl<T: ?Sized> Clone for MappedMem<T> {
    fn clone(&self) -> Self {
        Self {
            phys_addr: self.phys_addr,
            ptr: self.ptr.clone(),
        }
    }
}
impl<T: ?Sized> Copy for MappedMem<T> {}

impl<T, const N: usize> MappedMem<[T; N]> {
    pub fn cast_slice(self) -> MappedMem<[T]> {
        MappedMem {
            phys_addr: self.phys_addr,
            ptr: NonNull::slice_from_raw_parts(self.ptr.cast(), N),
        }
    }
}
