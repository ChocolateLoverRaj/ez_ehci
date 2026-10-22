#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeviceSpeed {
    /// 1.5 Mbps
    Low,
    /// 12 Mbps
    Full,
    /// 480 Mbps
    High,
}
