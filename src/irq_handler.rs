use core::sync::atomic::{AtomicBool, Ordering};

use alloc::sync::Arc;
use futures::task::AtomicWaker;
use volatile::{VolatilePtr, VolatileRef};

use crate::operational_regs::UsbStsReg;

#[derive(Debug, Default)]
pub struct IrqHandlerSharedState {
    pub new_usb_int: AtomicBool,
    // this is very similar to the USBSTS reg
    // however we need to immediately clear the USBSTS bits on an interrupt even if we don't immediately process it
    // so we store the changes here
    pub new_usb_err_int: AtomicBool,
    pub new_port_change_detect: AtomicBool,
    // frame list rollover is not needed
    pub new_host_system_error: AtomicBool,
    pub new_async_advance: AtomicBool,
    pub runner_waker: AtomicWaker,
}

/// Unlike xHCI, which has advanced interrupt handling with the ability to load balance between CPUs, the eHCI has a single PCI IRQ line (no MSI). This interrupt handler notifies async functions that are waiting for interrupts.
#[derive(Debug)]
pub struct IrqHandler {
    usb_sts: VolatileRef<'static, UsbStsReg>,
    shared_state: Arc<IrqHandlerSharedState>,
}

impl IrqHandler {
    pub(crate) fn new(
        usb_sts: VolatileRef<'static, UsbStsReg>,
        shared_state: Arc<IrqHandlerSharedState>,
    ) -> Self {
        Self {
            usb_sts,
            shared_state,
        }
    }

    /// This function will not allocate. This function will try to finish as soon as possible to maintain system responsiveness.
    pub fn handle_irq(&mut self) {
        self.shared_state.runner_waker.wake();
        let usb_sts = self.usb_sts.as_ptr().read();
        log::trace!("IRQ: {usb_sts:#?}");
        // Clear everything so an IRQ isn't immediately reaso an IRQ isn't immediately raised again after exiting.
        let mut usb_sts_value_to_write = UsbStsReg::new_with_raw_value(0);
        if usb_sts.usb_int() {
            self.shared_state.new_usb_int.store(true, Ordering::Relaxed);
            usb_sts_value_to_write.set_usb_int(true);
        }
        if usb_sts.usb_err_int() {
            self.shared_state
                .new_usb_err_int
                .store(true, Ordering::Relaxed);
            usb_sts_value_to_write.set_usb_err_int(true);
        }
        if usb_sts.port_change_detect() {
            // The spec specifically says that this change detect is set when a PORTSC reg has a state that *transitions*.
            // This means that we don't need to clear every write to clear bit in every PORTSC reg here.
            // Clearing this bit in USBSTS is enough.
            self.shared_state
                .new_port_change_detect
                .store(true, Ordering::Relaxed);
            usb_sts_value_to_write.set_port_change_detect(true);
        }
        if usb_sts.frame_list_rollover() {
            usb_sts_value_to_write.set_frame_list_rollover(true);
        }
        if usb_sts.host_system_error() {
            self.shared_state
                .new_host_system_error
                .store(true, Ordering::Relaxed);
            usb_sts_value_to_write.set_host_system_error(true);
        }
        if usb_sts.interrupt_on_async_advance() {
            self.shared_state
                .new_async_advance
                .store(true, Ordering::Relaxed);
            usb_sts_value_to_write.set_interrupt_on_async_advance(true);
        }
        // Safety: It's safe to have concurrent writes to usb_sts because it will be an atomic u32 write.
        let usb_sts_ptr = unsafe { VolatilePtr::new(self.usb_sts.as_ptr().as_raw_ptr()) };
        usb_sts_ptr.write(usb_sts_value_to_write);
    }
}
