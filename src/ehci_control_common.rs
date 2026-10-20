use crate::{error::Error, signal::Signal, start_config::StartConfig};

#[derive(Debug)]
pub enum ControlReq {
    Start(StartConfig),
}
pub type ControlReqSignal = Signal<ControlReq>;
pub type ControlResSignal = Signal<Result<(), Error>>;

#[derive(Default)]
pub struct ControlSharedState {
    pub req_signal: ControlReqSignal,
    pub res_signal: ControlResSignal,
}
