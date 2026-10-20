use core::{
    mem,
    sync::atomic::{Ordering, fence},
};

use alloc::vec::Vec;
use volatile::{VolatileFieldAccess, VolatilePtr, VolatileRef};

use crate::{
    CtrlDsSegment, MappedMem, Qh,
    oneshot::OneshotSender,
    operational_regs::UsbCmdReg,
    queue_head::{QhHorizontalLinkPtr, QhVolatileFieldAccess, SelectType},
    util::get_addr_offset,
};

/// Many different functions will want to put transfers on the QH at the same time. This struct handles that.
pub struct AsyncListManager {
    anchor_qh: MappedMem<QhWithMetaA>,
    ctrl_ds_segment: CtrlDsSegment,
    qhs_removed_before_doorbell: Vec<OneshotSender<()>>,
    qhs_queued_to_remove: Vec<OneshotSender<()>>,
}

const NO_ACTIVE_SLOT: usize = 0b11;

impl AsyncListManager {
    pub fn new(anchor_qh: MappedMem<QhWithMetaA>, ctrl_ds_segment: CtrlDsSegment) -> Self {
        Self {
            anchor_qh,
            ctrl_ds_segment,
            qhs_queued_to_remove: Default::default(),
            qhs_removed_before_doorbell: Default::default(),
        }
    }

    pub fn add_qh_to_async_list(&self, qh: MappedMem<QhWithMetaA>) {
        let qh_ptr = unsafe { VolatilePtr::new(qh.ptr) };
        let anchor_qh_ptr = unsafe { VolatilePtr::new(self.anchor_qh.ptr) };
        // Prepare the new QH
        let next_qh_ptr = anchor_qh_ptr.next().read();
        qh_ptr
            .qh()
            .horizontal_link_ptr()
            .write(anchor_qh_ptr.qh().horizontal_link_ptr().read());
        qh_ptr.prev().write(anchor_qh_ptr);
        qh_ptr.next().write(next_qh_ptr);

        // Ensure new QH fields hit RAM before anchor is pointed to it
        fence(Ordering::SeqCst);

        // Add it to the circular linked list
        anchor_qh_ptr
            .qh()
            .horizontal_link_ptr()
            .write(QhHorizontalLinkPtr::new(
                SelectType::Qh,
                get_addr_offset(qh.phys_addr, self.ctrl_ds_segment),
            ));
        // Update prev and next node's links
        next_qh_ptr.prev().write(qh_ptr);
        anchor_qh_ptr.next().write(qh_ptr);
    }

    pub fn queue_remove_qh(
        &mut self,
        mut usb_cmd: VolatileRef<'_, UsbCmdReg>,
        qh: MappedMem<QhWithMetaA>,
        sender: OneshotSender<()>,
    ) {
        // Get the ptr to the next qh
        let qh_ptr = unsafe { VolatilePtr::new(qh.ptr) };
        let next_horizontal_link_ptr = qh_ptr.qh().horizontal_link_ptr().read();
        // Update the previous qh to point to the next qh
        let prev_qh_ptr = qh_ptr.prev().read();
        prev_qh_ptr
            .qh()
            .horizontal_link_ptr()
            .write(next_horizontal_link_ptr);
        // Update the next qh's prev pointer
        let next_qh_ptr = qh_ptr.next().read();
        next_qh_ptr.prev().write(prev_qh_ptr);
        prev_qh_ptr.next().write(next_qh_ptr);
        // Flush unlinking writes to DRAM for DMA Coherence
        fence(core::sync::atomic::Ordering::SeqCst);

        if self.qhs_removed_before_doorbell.is_empty() {
            // Doorbell is not rung
            self.qhs_removed_before_doorbell.push(sender);
            usb_cmd
                .as_mut_ptr()
                .update(|usb_cmd| usb_cmd.with_interrupt_on_async_advance_doorbell(true));
        } else {
            // Doorbell already in progress, add to queue
            self.qhs_queued_to_remove.push(sender);
        }
    }

    /// Call when the async advance bit is set (and interrupt will occur). Clear the bit before calling.
    pub fn handle_async_advance(&mut self, mut usb_cmd: VolatileRef<'_, UsbCmdReg>) {
        let new_qhs_before_doorbell = mem::take(&mut self.qhs_queued_to_remove);
        let removed_qhs = mem::replace(
            &mut self.qhs_removed_before_doorbell,
            new_qhs_before_doorbell,
        );
        for sender in removed_qhs {
            sender.send(());
        }
        if !self.qhs_removed_before_doorbell.is_empty() {
            usb_cmd
                .as_mut_ptr()
                .update(|usb_cmd| usb_cmd.with_interrupt_on_async_advance_doorbell(true));
        }
    }
}

/// A Queue Head with metadata for being in the async list.
#[repr(C)]
#[derive(Debug, Clone, Copy, VolatileFieldAccess)]
pub struct QhWithMetaA {
    pub(crate) qh: Qh,
    pub(crate) prev: VolatilePtr<'static, QhWithMetaA>,
    pub(crate) next: VolatilePtr<'static, QhWithMetaA>,
}
