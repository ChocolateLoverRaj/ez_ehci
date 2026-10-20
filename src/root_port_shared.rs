use crate::{
    MappedMem, async_list_manager::QhWithMetaA, device::Device, error::Error, qtd::Qtd,
    signal::Signal,
};

pub type PortChangedSignal = Signal<()>;
pub type RootPortReqSignal = Signal<ResetDeviceResources>;
pub type RootPortResSignal = Signal<Result<Device, Error>>;

pub struct ResetDeviceResources {
    pub qh: MappedMem<QhWithMetaA>,
    pub qtds: [MappedMem<Qtd>; 2],
    pub buffer: MappedMem<[u8; 8]>,
}

#[derive(Default)]
pub struct RootPortSharedState {
    pub port_changed_signal: PortChangedSignal,
    pub req_signal: RootPortReqSignal,
    pub res_signal: RootPortResSignal,
}
