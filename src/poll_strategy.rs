use core::time::Duration;

/// Some things have no IRQ to indicate that they are done and might not be done instantly.
#[derive(Debug, Clone, Copy)]
pub struct PollStrategy {
    pub check_interval: Duration,
    pub timeout: Duration,
}

impl PollStrategy {
    /// From the spec:
    /// > Controller must halt within 16 micro-frames after software clears the Run bit.
    pub const STOP_RECOMMENDED: Self = Self {
        // tradeoff between latency and CPU usage
        check_interval: Duration::from_millis(1),
        // 16 micro-frames is 2ms
        timeout: Duration::from_millis(2),
    };

    pub const RESET_RECOMMENDED: Self = Self {
        // tradeoff between latency and CPU usage
        check_interval: Duration::from_millis(1),
        // there is no max time specified, but 1s should be a sensible timeout
        timeout: Duration::from_secs(1),
    };
}
