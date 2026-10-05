use alloc::{boxed::Box, sync::Arc};
use arbitrary_int::{u4, u5};
use futures::task::AtomicWaker;
use volatile::VolatilePtr;

use crate::{
    device_greeter::PortChangeDetectWaker,
    operational_regs::{OperationalRegs, OperationalRegsVolatileFieldAccess, PortScReg, UsbStsReg},
    qh_manager::QhManager,
};

/// This can be configured within 0..=31, since max 31 endpoints at a time which we might want to execute 1 future for each endpoint in parallel.
pub const WAKERS_PER_PORT: u5 = u5::new(31);
/// WAKERS_PER_PORT for port 0, WAKERS_PER_PORT for port 1, etc, for each port that exists on the eHCI.
pub type UsbIntWakers = Arc<[AtomicWaker]>;

/// Unlike xHCI, which has advanced interrupt handling with the ability to load balance between CPUs, the eHCI has a single PCI IRQ line (no MSI). This interrupt handler notifies async functions that are waiting for interrupts.
#[derive(Debug)]
pub struct IrqHandler {
    n_ports: u4,
    port_change_detect_waker: PortChangeDetectWaker,
    operational_regs: VolatilePtr<'static, OperationalRegs>,
    port_sc_regs: VolatilePtr<'static, [PortScReg]>,
    port_wakers: UsbIntWakers,
    qh_manager: Arc<QhManager>,
}
unsafe impl Send for IrqHandler {}
unsafe impl Sync for IrqHandler {}

impl IrqHandler {
    pub(crate) fn new(
        n_ports: u4,
        port_change_detect_waker: PortChangeDetectWaker,
        operational_regs: VolatilePtr<'static, OperationalRegs>,
        port_sc_regs: VolatilePtr<'static, [PortScReg]>,
        port_wakers: UsbIntWakers,
        qh_manager: Arc<QhManager>,
    ) -> Self {
        Self {
            n_ports,
            operational_regs,
            port_change_detect_waker,
            port_sc_regs,
            port_wakers,
            qh_manager,
        }
    }

    /// This function will not allocate. This function will try to finish as soon as possible to maintain system responsiveness.
    pub fn handle_irq(&mut self) {
        let status = self.operational_regs.usb_sts().read();
        // TODO: This can make us exit our 2ms wait early.
        let mut clear = UsbStsReg::new_with_raw_value(0).with_frame_list_rollover(true);
        if status.host_system_error() {
            panic!("host system error");
        }
        if status.usb_err_int() {
            log::error!("eHCI error interrupt");
            clear.set_usb_err_int(true);
            for waker in self.port_wakers.iter() {
                waker.wake();
            }
        }
        if status.interrupt_on_async_advance() {
            log::trace!("INTERRUPT ON ASYNC ADVANCE!");
            self.qh_manager.handle_async_advance();
            clear.set_interrupt_on_async_advance(true);
        }
        if status.port_change_detect() {
            log::trace!("PORT CHANGE DETECT INT!");
            clear.set_port_change_detect(true);
            self.port_change_detect_waker.wake();
        }
        if status.usb_int() {
            log::trace!("USB INT!");
            clear.set_usb_int(true);
            for waker in self.port_wakers.iter() {
                waker.wake();
            }
        }
        self.operational_regs.usb_sts().write(clear);

        for port in 0..self.n_ports.value() {
            let port_sc_reg_ptr = self.port_sc_regs.index(usize::try_from(port).unwrap());
            let port_sc_reg = port_sc_reg_ptr.read();
            if port_sc_reg.connect_status_change() {
                log::info!("port {port} connect status changed");
            }
            if port_sc_reg.port_enable_change() {
                log::info!("port {port} port enable changed");
            }
            if port_sc_reg.over_current_change() {
                log::info!("port {port} over current change");
            }
            // Clear interrupts
            port_sc_reg_ptr.write(port_sc_reg);
        }
    }
}
