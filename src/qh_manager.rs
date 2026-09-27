use core::{
    future,
    sync::atomic::{AtomicBool, AtomicU16, Ordering},
    task::Poll,
};

use alloc::sync::Arc;
use arbitrary_int::{traits::Integer, u4};
use futures::task::AtomicWaker;
use spinning_top::Spinlock;
use volatile::{VolatileFieldAccess, VolatilePtr};

use crate::{
    MappedMem, QueueHead,
    irq_handler::UsbIntWakers,
    operational_regs::{OperationalRegs, OperationalRegsVolatileFieldAccess},
    queue_head::{QueueHeadHorizontalLinkPtr, QueueHeadVolatileFieldAccess, SelectType},
};

/// Len: number of ports + 1 for device greeter
pub type QhRemovedWakers = Arc<[AtomicWaker]>;

/// Many different functions will want to put transfers on the QH at the same time. This struct handles that.
#[derive(Debug)]
pub struct QhManager {
    operational_regs: VolatilePtr<'static, OperationalRegs>,
    anchor_qh: MappedMem<QhWithMetadata>,
    lock: Spinlock<()>,
    /// Each bit represents a port, with the highest bit for the device greeter.
    qh_removed_wakers: UsbIntWakers,
    /// If there is already an int that we are waiting for, add your bit to the queued.
    queued: AtomicU16,
    /// Waiting for async advance int.
    /// BIts from queued move here.
    waiting: AtomicU16,
    /// bits from waiting move here
    removed_qhs: AtomicU16,
}

impl QhManager {
    pub fn new(
        anchor_qh: MappedMem<QhWithMetadata>,
        n_ports: u4,
        operational_regs: VolatilePtr<'static, OperationalRegs>,
    ) -> Self {
        Self {
            operational_regs,
            anchor_qh,
            lock: Spinlock::new(()),
            qh_removed_wakers: (0..n_ports.as_usize() + 1)
                .map(|_| AtomicWaker::new())
                .collect(),
            queued: AtomicU16::new(0),
            waiting: AtomicU16::new(0),
            removed_qhs: AtomicU16::new(0),
        }
    }

    pub fn add_qh_to_async_list(&self, qh: MappedMem<QhWithMetadata>) {
        let qh_ptr = unsafe { VolatilePtr::new(qh.ptr) };
        let anchor_qh_ptr = unsafe { VolatilePtr::new(self.anchor_qh.ptr) };
        let lock = self.lock.lock();
        // Prepare the new QH
        let next_qh_ptr = anchor_qh_ptr.next().read();
        qh_ptr
            .qh()
            .queue_head_horizontal_link_ptr()
            .write(anchor_qh_ptr.qh().queue_head_horizontal_link_ptr().read());
        qh_ptr.prev().write(anchor_qh_ptr);
        qh_ptr.next().write(next_qh_ptr);
        // Add it to the circular linked list
        anchor_qh_ptr
            .qh()
            .queue_head_horizontal_link_ptr()
            .write(QueueHeadHorizontalLinkPtr::new(
                SelectType::Qh,
                qh.phys_addr,
            ));
        // Update prev and next node's links
        next_qh_ptr.prev().write(qh_ptr);
        anchor_qh_ptr.next().write(qh_ptr);
        drop(lock)
    }

    pub async fn remove_qh(&self, qh: MappedMem<QhWithMetadata>, index: u4) {
        log::info!("removing QH");
        let lock = self.lock.lock();
        // Get the ptr to the next qh
        let qh_ptr = unsafe { VolatilePtr::new(qh.ptr) };
        let next_horizontal_link_ptr = qh_ptr.qh().queue_head_horizontal_link_ptr().read();
        // Update the previous qh to point to the next qh
        let prev_qh_ptr = qh_ptr.prev().read();
        prev_qh_ptr
            .qh()
            .queue_head_horizontal_link_ptr()
            .write(next_horizontal_link_ptr);
        // Update the next qh's prev pointer
        let next_qh_ptr = qh_ptr.next().read();
        next_qh_ptr.prev().write(prev_qh_ptr);
        drop(lock);

        let mut added_to_queue = false;
        future::poll_fn(|cx| {
            // Register waker
            self.qh_removed_wakers[index.as_usize()].register(cx.waker());
            let our_bit = 1 << index.value();
            if !added_to_queue {
                // Add self to queue
                let queued_bits = self.queued.fetch_or(our_bit, Ordering::Relaxed) | our_bit;
                // Check if already waiting
                match self.waiting.compare_exchange(
                    0,
                    queued_bits,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // Not already waiting, start the wait
                        log::info!("remove_qh is ringing async advance doorbell");
                        // Remove from queued, picking up any new queued ones that may have just been added
                        let new_queued_bits = self.queued.swap(0, Ordering::Relaxed);
                        if new_queued_bits != queued_bits {
                            self.waiting.store(new_queued_bits, Ordering::Relaxed);
                        }
                        self.operational_regs.usb_cmd().update(|usb_cmd| {
                            usb_cmd.with_interrupt_on_async_advance_doorbell(true)
                        });
                    }
                    Err(_) => {
                        // Already waiting, not our job to do anything, we'll get interrupted eventually
                    }
                }
                added_to_queue = true;
            }
            // Check if it was removed, clearing the bit
            if self.removed_qhs.fetch_and(!our_bit, Ordering::Relaxed) & our_bit != 0 {
                // Our qh was removed
                Poll::Ready(())
            } else {
                log::info!("waiting for IRQ for removed QHs");
                Poll::Pending
            }
        })
        .await;
    }

    /// Call when the async advance bit is set (and interrupt will occur). Does not clear async advance. Clear the bit after calling.
    pub(crate) fn handle_async_advance(&self) {
        // Update removed qhs
        let bit_mask = self.waiting.swap(0, Ordering::Relaxed);
        self.removed_qhs.store(bit_mask, Ordering::Relaxed);
        // Notify
        for waker in self.qh_removed_wakers.iter() {
            waker.wake();
        }
        // Advance the queue
        let queued_bits = self.queued.load(Ordering::Relaxed);
        if queued_bits != 0 {
            match self.waiting.compare_exchange(
                0,
                queued_bits,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    // Again tell the eHCI to interrupt on next advance
                    log::info!("QH manager irq is ringing async advance doorbell");
                    // Remove from queued, picking up any new queued ones that may have just been added
                    let new_queued_bits = self.queued.swap(0, Ordering::Relaxed);
                    if new_queued_bits != queued_bits {
                        self.waiting.store(new_queued_bits, Ordering::Relaxed);
                    }
                    self.operational_regs
                        .usb_cmd()
                        .update(|usb_cmd| usb_cmd.with_interrupt_on_async_advance_doorbell(true));
                }
                Err(_) => {
                    // This means someone else already started it, our work is done
                }
            }
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, VolatileFieldAccess)]
pub struct QhWithMetadata {
    pub(crate) qh: QueueHead,
    pub(crate) prev: VolatilePtr<'static, QhWithMetadata>,
    pub(crate) next: VolatilePtr<'static, QhWithMetadata>,
}
