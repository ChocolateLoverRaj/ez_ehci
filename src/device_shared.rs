use alloc::sync::Arc;

use crate::{interrupt_endpoint_shared::InterruptEndpointSharedState, signal::Signal};

pub struct AddEndpointReq {
    pub endpoint_shared: Arc<InterruptEndpointSharedState>,
}

#[derive(Default)]
pub struct DeviceSharedState {
    pub req_signal: Signal<AddEndpointReq>,
    pub usb_int_signal: Signal<()>,
}
