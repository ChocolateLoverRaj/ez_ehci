use core::{hint, ptr::NonNull, sync::atomic::AtomicBool};

use arbitrary_int::{traits::Integer, u2};
use futures::task::AtomicWaker;
use volatile::{VolatilePtr, access::ReadOnly};

use crate::{
    InitializedEhci, MappedMem, PeriodicFrameList,
    capability_regs::CapabilityRegs,
    operational_regs::{
        AsyncListAddrReg, OperationalRegs, OperationalRegsVolatileFieldAccess, PortScReg, UsbStsReg,
    },
    periodic_list::PeriodicFrameListElement,
    queue_head::{
        EndpointCapabilities, EndpointCharacteristics, QueueHead, QueueHeadHorizontalLinkPtr,
        SelectType,
    },
};

pub struct OsOwnedEhci {
    capability_regs: VolatilePtr<'static, CapabilityRegs, ReadOnly>,
    operational_regs: VolatilePtr<'static, OperationalRegs>,
    port_sc_regs: VolatilePtr<'static, [PortScReg]>,
}

impl OsOwnedEhci {
    /// # Safety
    /// The bar must be the virtual address pointing to the bar. The entire BAR 0 must be mapped with the right caching type.
    pub(crate) unsafe fn new(bar: NonNull<[u8]>) -> Self {
        let capability_regs_ptr =
            unsafe { VolatilePtr::<CapabilityRegs>::new_read_only(bar.cast()) };
        let capability_regs = capability_regs_ptr.read();
        log::debug!("capability regs: {capability_regs:#X?}");
        let operational_regs_ptr = unsafe {
            VolatilePtr::new(
                bar.byte_offset(capability_regs.cap_len.try_into().unwrap())
                    .cast(),
            )
        };
        let operational_regs = operational_regs_ptr.read();
        log::debug!("operational regs: {operational_regs:#X?}");
        let n_root_ports = capability_regs.hcs_params.n_ports();
        let n_ports = n_root_ports.value().try_into().unwrap();
        let port_sc_regs = unsafe {
            VolatilePtr::new(NonNull::slice_from_raw_parts(
                operational_regs_ptr.port_sc().as_raw_ptr().cast(),
                n_ports,
            ))
        };
        for port in 0..n_ports {
            let port_sc = port_sc_regs.index(port).read();
            log::debug!("port_sc[{port}]: {port_sc:#X?}");
        }
        Self {
            capability_regs: capability_regs_ptr,
            operational_regs: operational_regs_ptr,
            port_sc_regs,
        }
    }

    pub fn init(
        self,
        periodic_frame_list_mem: MappedMem<PeriodicFrameList>,
        anchor_qh_mem: MappedMem<QueueHead>,
    ) -> InitializedEhci {
        // Halt
        log::info!("Halting eHCI");
        self.operational_regs
            .usb_cmd()
            .update(|usb_cmd| usb_cmd.with_rs(false));
        // Wait until it's stopped
        while !self.operational_regs.usb_sts().read().hc_halted() {
            hint::spin_loop();
        }
        log::info!("Resetting eHCI");
        // Reset
        self.operational_regs
            .usb_cmd()
            .update(|usb_cmd| usb_cmd.with_hc_reset(true));
        // Wait until it's done resetting
        while self.operational_regs.usb_cmd().read().hc_reset() {
            hint::spin_loop();
        }
        log::info!("Done resetting");

        // Initialize CTRLDSSEGMENT
        // Currently we only support memory in the lower 4 GiB
        // eHCI can optionaly support higher mem as long as it's all within a 4 GiB aligned region.
        // We could add support for this later.
        self.operational_regs.ctrl_ds_segment().write(0);
        log::info!("Initialized CTRLDSSEGMENT");

        // Initialize USBINTR
        self.operational_regs.usb_intr().update(|usb_intr| {
            usb_intr
                .with_usb_interrupt_enable(true)
                .with_usb_error_interrupt_enable(true)
                .with_port_change_interrupt_enable(true)
                .with_host_system_error_interrupt_enable(true)
        });
        // Clear pending interrupts
        self.operational_regs.usb_sts().write(
            UsbStsReg::new_with_raw_value(0)
                .with_usb_int(true)
                .with_frame_list_rollover(true),
        );
        log::info!("Initialized USBINTR");

        // Initialize the periodic frame list
        let periodic_frame_list = unsafe { VolatilePtr::new(periodic_frame_list_mem.ptr) };
        periodic_frame_list.write(PeriodicFrameList {
            elements: [PeriodicFrameListElement::new_with_raw_value(Default::default())
                .with_t(true)
                .with_reserved(u2::ZERO); _],
        });
        log::info!("Initialized periodic frame list");
        // Initialize PERIODICLIST BASE
        self.operational_regs
            .periodic_list_base()
            .write(periodic_frame_list_mem.phys_addr.try_into().unwrap());
        log::info!("Initialized PERIODICLIST BASE");
        // Write to USBCMD to turn the host controller on
        self.operational_regs
            .usb_cmd()
            .update(|usb_cmd| usb_cmd.with_rs(true));
        log::info!("Initialized USBCMD");
        // Initialize CONFIGFLAG
        self.operational_regs
            .config_flag()
            .update(|config_flag| config_flag.with_configure_flag(true));
        log::info!("Initialized CONFIGFLAG");

        // Initialize the async schedule
        let qh_ptr = unsafe { VolatilePtr::new(anchor_qh_mem.ptr) };
        let mut qh = QueueHead::new(
            EndpointCharacteristics::ZERO.with_head_of_reclamation_list_flag(true),
            EndpointCapabilities::ZERO,
        );
        qh.queue_head_horizontal_link_ptr =
            QueueHeadHorizontalLinkPtr::new(SelectType::Qh, anchor_qh_mem.phys_addr);
        qh_ptr.write(qh);
        self.operational_regs
            .async_list_addr()
            .write(AsyncListAddrReg::new(anchor_qh_mem.phys_addr));
        self.operational_regs
            .usb_cmd()
            .update(|usb_cmd| usb_cmd.with_async_schedule_enable(true));

        InitializedEhci {
            capability_regs: self.capability_regs,
            operational_regs: self.operational_regs,
            port_sc_regs: self.port_sc_regs,
            int_occurred: AtomicBool::new(false),
            waker: AtomicWaker::new(),
            async_advance_occurred: AtomicBool::new(false),
            anchor_qh: anchor_qh_mem,
        }
    }
}
