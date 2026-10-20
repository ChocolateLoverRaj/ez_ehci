use crate::{MappedMem, async_list_manager::QhWithMetaA, signal::Signal};

#[derive(Default)]
pub struct ControlEndpointSharedState {
    pub req_signal: Signal<MappedMem<QhWithMetaA>>,
    pub int_signal: Signal<()>,
}
