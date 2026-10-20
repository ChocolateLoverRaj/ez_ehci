use alloc::sync::Arc;

use crate::{
    ehci_control_common::{ControlReq, ControlSharedState},
    error::Error,
    start_config::StartConfig,
};

pub struct EhciControl {
    shared: Arc<ControlSharedState>,
}

impl EhciControl {
    pub(crate) fn new(shared: Arc<ControlSharedState>) -> Self {
        Self { shared }
    }

    pub async fn start(&mut self, config: StartConfig) -> Result<(), Error> {
        self.shared.req_signal.signal(ControlReq::Start(config));
        log::debug!("signaled start");
        self.shared.res_signal.wait().await
    }
}
