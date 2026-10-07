use core::{
    future,
    sync::atomic::{AtomicU16, Ordering, fence},
    task::Poll,
};

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

/// Many different functions will want to put transfers on the QH at the same time. This struct handles that.
#[derive(Debug)]
pub struct QhManager {
    operational_regs: VolatilePtr<'static, OperationalRegs>,
    anchor_qh: MappedMem<QhWithMetadata>,
    lock: Spinlock<()>,
    /// Each bit represents a port, with the highest bit for the device greeter.
    qh_removed_wakers: UsbIntWakers,

    /// Ports that are waiting an async advance int so that we can ring the doorbell again and wait for the next async advance int.
    queued_for_removal: AtomicU16,
    /// Ports that were removed before the lastest doorbell ring.
    /// Their removal will be synchronized with the HC on the next async advance int.
    notified_hc_of_removal: AtomicU16,
    /// Bits whose removal has been acknowledged by the async advance interrupt.
    completed_removal: AtomicU16,
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
            queued_for_removal: AtomicU16::new(0),
            notified_hc_of_removal: AtomicU16::new(0),
            completed_removal: AtomicU16::new(0),
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

        // Ensure new QH fields hit RAM before anchor is pointed to it
        fence(Ordering::SeqCst);

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
        prev_qh_ptr.next().write(next_qh_ptr);
        // Flush unlinking writes to DRAM for DMA Coherence
        fence(core::sync::atomic::Ordering::SeqCst);
        drop(lock);

        let our_bit = 1 << index.value();
        // Add our bit to pending
        self.queued_for_removal.fetch_or(our_bit, Ordering::SeqCst);

        // Try to trigger the doorbell if no doorbell is currently in-flight
        self.try_ring_doorbell();

        // Wait for EHCI interrupt to acknowledge removal
        future::poll_fn(|cx| {
            self.qh_removed_wakers[index.as_usize()].register(cx.waker());

            // Check if our bit has been acknowledged by EHCI
            if (self.completed_removal.load(Ordering::SeqCst) & our_bit) != 0 {
                // Clear our bit from completed
                self.completed_removal.fetch_and(!our_bit, Ordering::SeqCst);
                Poll::Ready(())
            } else {
                // Re-check doorbell in case of race
                self.try_ring_doorbell();
                Poll::Pending
            }
        })
        .await;
    }

    fn try_ring_doorbell(&self) {
        // Only ring if in_flight is 0
        if self
            .notified_hc_of_removal
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            // Swap pending into in_flight_removal
            let to_process = self.queued_for_removal.swap(0, Ordering::SeqCst);
            if to_process == 0 {
                // Nothing pending, release in_flight lock
                self.notified_hc_of_removal.store(0, Ordering::SeqCst);
                return;
            }

            // Save active mask
            self.notified_hc_of_removal
                .store(to_process, Ordering::SeqCst);

            // Ring the EHCI Async Advance Doorbell
            fence(Ordering::SeqCst);
            self.operational_regs
                .usb_cmd()
                .update(|usb_cmd| usb_cmd.with_interrupt_on_async_advance_doorbell(true));
        }
    }

    /// Call when the async advance bit is set (and interrupt will occur). Does not clear async advance. Clear the bit after calling.
    pub(crate) fn handle_async_advance(&self) {
        // Collect bits that were flushed during this doorbell cycle
        let finished_bits = self.notified_hc_of_removal.swap(0, Ordering::SeqCst);

        if finished_bits != 0 {
            // Mark these QHs as safely completed
            self.completed_removal
                .fetch_or(finished_bits, Ordering::SeqCst);

            // Wake up waiting tasks
            for waker in self.qh_removed_wakers.iter() {
                waker.wake();
            }
        }

        // If new QHs were unlinked while the doorbell was in-flight, trigger next doorbell cycle
        if self.queued_for_removal.load(Ordering::SeqCst) != 0 {
            self.try_ring_doorbell();
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
