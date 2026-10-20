#[derive(Debug, Clone, Copy)]
pub enum Error {
    StopTimeout,
    ResetTimeout,
    DeviceDisconnected,
    DeviceIsFullSpeed,
}
