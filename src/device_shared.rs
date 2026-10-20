use alloc::sync::Arc;

use crate::{
    MappedMem, async_list_manager::QhWithMetaA,
    interrupt_endpoint_shared::InterruptEndpointSharedState, oneshot::OneshotSender,
    signal::Signal,
};

pub struct AddQhReq {
    pub add_qh: MappedMem<QhWithMetaA>,
    pub callback: OneshotSender<()>,
}

pub struct AddEndpointReq {
    pub endpoint_shared: Arc<InterruptEndpointSharedState>,
}

pub enum DeviceReq {
    AddQh(AddQhReq),
    AddEndpoint(AddEndpointReq),
}

#[derive(Default)]
pub struct DeviceSharedState {
    pub req_signal: Signal<DeviceReq>,
    pub usb_int_signal: Signal<()>,
}
