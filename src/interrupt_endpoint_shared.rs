use crate::{
    MappedMem,
    oneshot::OneshotSender,
    periodic_list_manager::{NormalizedInterval, QhWithMetaP},
    signal::Signal,
};

pub struct AddInterruptQhReq {
    pub qh: MappedMem<QhWithMetaP>,
    pub interval: NormalizedInterval,
    pub callback: OneshotSender<()>,
}

#[derive(Default)]
pub struct InterruptEndpointSharedState {
    pub req_signal: Signal<AddInterruptQhReq>,
    pub int_signal: Signal<()>,
}
