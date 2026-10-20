use core::task::Poll;

use alloc::sync::Arc;
use volatile::VolatileRef;

use crate::{
    MappedMem,
    async_list_manager::QhWithMetaA,
    device::Device,
    error::Error,
    operational_regs::PortScReg,
    qtd::Qtd,
    root_port_shared::{ResetDeviceResources, RootPortSharedState},
};

pub struct RootPort {
    port_sc_ptr: VolatileRef<'static, PortScReg>,
    shared: Arc<RootPortSharedState>,
}

impl RootPort {
    pub(crate) fn new(
        port_sc_ptr: VolatileRef<'static, PortScReg>,
        shared: Arc<RootPortSharedState>,
    ) -> Self {
        Self {
            port_sc_ptr,
            shared,
        }
    }

    pub fn is_device_connected(&self) -> bool {
        self.port_sc_ptr.as_ptr().read().current_connect_status()
    }

    pub async fn wait_until_connected(&mut self) {
        loop {
            if self.is_device_connected() {
                break;
            }
            self.shared.port_changed_signal.wait().await;
        }
    }

    /// If you cancel this future, it will leak memory.
    pub async fn reset_device(&mut self, resources: ResetDeviceResources) -> Result<Device, Error> {
        // self.port_sc_ptr
        //     .as_mut_ptr()
        //     .update(|p| p.with_port_reset(true));
        // self.port_sc_ptr
        //     .as_mut_ptr()
        //     .update(|p| p.with_port_reset(false));
        // let p = self.port_sc_ptr.as_mut_ptr().read();
        // log::info!("p: {p:#X?}");
        self.shared.req_signal.signal(resources);
        ResetDeviceFuture { root_port: self }.await
    }
}

pub struct ResetDeviceOutput {}

struct ResetDeviceFuture<'a> {
    root_port: &'a mut RootPort,
}

impl Future for ResetDeviceFuture<'_> {
    type Output = Result<Device, Error>;

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        self.root_port.shared.res_signal.poll_wait(cx)
    }
}

impl Drop for ResetDeviceFuture<'_> {
    fn drop(&mut self) {}
}
