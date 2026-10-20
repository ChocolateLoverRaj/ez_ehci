use core::{pin::Pin, time::Duration};

use alloc::boxed::Box;

/// Explicitly `dyn` compatible.
/// Must:
/// - not allocate
/// - not block
/// - not acquire a lock
/// - be cancel safe
pub trait Delay {
    fn now(&self) -> Duration;
    fn delay_until(&self, time: Duration) -> Pin<Box<dyn Future<Output = ()>>>;
}
