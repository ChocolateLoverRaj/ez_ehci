use alloc::boxed::Box;

/// Explicitly `dyn` compatible.
pub trait Delay {
    /// Must not allocate, block, or acquire a lock.
    fn delay_ms(&self, delay_ms: u8) -> Box<dyn Future<Output = ()>>;
}
