use core::ptr::NonNull;

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use arbitrary_int::traits::Integer;
use volatile::VolatileRef;

use crate::{
    capability_regs::{CapabilityRegs, CapabilityRegsVolatileFieldAccess},
    delay::Delay,
    ehci_control::EhciControl,
    ehci_control_common::ControlSharedState,
    irq_handler::{IrqHandler, IrqHandlerSharedState},
    operational_regs::{OperationalRegsVolatileFieldAccess, PortScReg},
    root_port::RootPort,
    root_port_shared::RootPortSharedState,
    runner::Runner,
};

pub struct Stack {
    pub irq_handler: IrqHandler,
    pub runner: Runner,
    pub control: EhciControl,
    pub root_ports: Vec<RootPort>,
}

impl Stack {
    /// # Safety
    /// The bar must be the virtual address pointing to the bar. The entire BAR 0 must be mapped with the right caching type.
    pub(crate) unsafe fn new(bar: NonNull<[u8]>, delay: Box<dyn Delay>) -> Self {
        let capability_regs_ptr =
            unsafe { VolatileRef::<CapabilityRegs>::new_read_only(bar.cast()) };
        let mut operational_regs_ptr = unsafe {
            VolatileRef::new(
                bar.byte_offset(
                    capability_regs_ptr
                        .as_ptr()
                        .cap_len()
                        .read()
                        .try_into()
                        .unwrap(),
                )
                .cast(),
            )
        };
        let n_root_ports = capability_regs_ptr.as_ptr().hcs_params().read().n_ports();
        let mut port_sc_regs = unsafe {
            VolatileRef::new(NonNull::slice_from_raw_parts(
                operational_regs_ptr
                    .as_mut_ptr()
                    .port_sc()
                    .as_raw_ptr()
                    .cast::<PortScReg>(),
                n_root_ports.as_usize(),
            ))
        };
        let mut root_ports_shared = Vec::new();
        let mut root_ports = Vec::new();
        for i in 0..n_root_ports.value() {
            let shared = Arc::new(RootPortSharedState::default());
            root_ports_shared.push(shared.clone());
            root_ports.push(RootPort::new(
                unsafe {
                    VolatileRef::new(
                        port_sc_regs
                            .as_mut_ptr()
                            .index(usize::try_from(i).unwrap())
                            .as_raw_ptr(),
                    )
                },
                shared,
            ));
        }

        let irq_handler_shared_state = Arc::new(IrqHandlerSharedState::default());
        let usb_sts_ptr =
            unsafe { VolatileRef::new(operational_regs_ptr.as_ptr().usb_sts().as_raw_ptr()) };
        let control_shared_state = Arc::new(ControlSharedState::default());
        Self {
            irq_handler: IrqHandler::new(usb_sts_ptr, irq_handler_shared_state.clone()),
            runner: Runner::new(
                delay,
                control_shared_state.clone(),
                capability_regs_ptr,
                operational_regs_ptr,
                root_ports_shared,
                irq_handler_shared_state,
                port_sc_regs,
            ),
            control: EhciControl::new(control_shared_state),
            root_ports,
        }
    }
}
