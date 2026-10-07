use core::{
    cell::UnsafeCell,
    future,
    sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering, fence},
    task::Poll,
};

use alloc::{
    sync::{Arc, Weak},
    task::Wake,
    vec::Vec,
};
use futures::task::AtomicWaker;
use spinning_top::Spinlock;
use volatile::{VolatileFieldAccess, VolatilePtr};

use crate::{
    MappedMem, QueueHead,
    operational_regs::{OperationalRegs, OperationalRegsVolatileFieldAccess},
    queue_head::{QueueHeadHorizontalLinkPtr, QueueHeadVolatileFieldAccess, SelectType},
};

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    /// Free to convert into a queue, empty existing items first.
    Garbage,
    /// Has queued items. More items could be added or this could be converted into active.
    Queued,
    /// Is waiting for an async advance interrupt.
    Active,
}

#[derive(Debug)]
struct Slot {
    state: SlotState,
    wakers: Vec<Weak<OneshotWaker>>,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            state: SlotState::Garbage,
            wakers: Default::default(),
        }
    }
}

#[derive(Debug, Default)]
struct OneshotWaker {
    woken: AtomicBool,
    waker: AtomicWaker,
}

impl Wake for OneshotWaker {
    fn wake(self: alloc::sync::Arc<Self>) {
        self.woken.store(true, Ordering::Relaxed);
        self.waker.wake();
    }
}

/// Many different functions will want to put transfers on the QH at the same time. This struct handles that.
#[derive(Debug)]
pub struct QhManager {
    operational_regs: VolatilePtr<'static, OperationalRegs>,
    anchor_qh: MappedMem<QhWithMetadata>,
    circular_list_lock: Spinlock<()>,
    /// Bit 0: whoever sets this to 1 has &mut slots[0].
    /// Bit 1: whoever sets this to 1 has &mut slots[1].
    active_locks: AtomicU8,
    slots: [UnsafeCell<Slot>; 2],
    /// You're only allowed to modify this when you have both active locks.
    /// A value of 0b11 is used to indicate no active slot
    active_slot: AtomicUsize,
}

const NO_ACTIVE_SLOT: usize = 0b11;

impl QhManager {
    pub fn new(
        anchor_qh: MappedMem<QhWithMetadata>,
        operational_regs: VolatilePtr<'static, OperationalRegs>,
    ) -> Self {
        Self {
            operational_regs,
            anchor_qh,
            circular_list_lock: Spinlock::new(()),
            active_locks: AtomicU8::new(0),
            slots: [Default::default(), Default::default()],
            active_slot: AtomicUsize::new(NO_ACTIVE_SLOT),
        }
    }

    pub fn add_qh_to_async_list(&self, qh: MappedMem<QhWithMetadata>) {
        let qh_ptr = unsafe { VolatilePtr::new(qh.ptr) };
        let anchor_qh_ptr = unsafe { VolatilePtr::new(self.anchor_qh.ptr) };
        let lock = self.circular_list_lock.lock();
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

    pub async fn remove_qh(&self, qh: MappedMem<QhWithMetadata>) {
        log::info!("removing QH");
        let lock = self.circular_list_lock.lock();
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

        let waker = Arc::new(OneshotWaker::default());

        enum BitsToAcquire {
            Both,
            One(usize),
        }
        impl BitsToAcquire {
            pub fn bitmask(&self) -> u8 {
                match self {
                    BitsToAcquire::Both => 0b11,
                    BitsToAcquire::One(index) => 1 << *index,
                }
            }

            pub fn bitmask_other(&self) -> u8 {
                0b11 & !self.bitmask()
            }
        }

        let mut bits_to_acquire = BitsToAcquire::Both;
        loop {
            match self.active_locks.compare_exchange(
                bits_to_acquire.bitmask_other(),
                0b11,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    match bits_to_acquire {
                        BitsToAcquire::Both => {
                            // We have the doorbell lock
                            let slot_to_queue = {
                                let slots = self
                                    .slots
                                    .each_ref()
                                    .map(|slot| unsafe { slot.get().as_mut_unchecked() });
                                if slots[0].state == SlotState::Garbage {
                                    slots[0].wakers.clear();
                                }
                                if slots[1].state == SlotState::Garbage {
                                    slots[1].wakers.clear();
                                }
                                let slot_to_queue = match (slots[0].state, slots[1].state) {
                                    (SlotState::Garbage, SlotState::Garbage) => 0,
                                    (SlotState::Queued, SlotState::Garbage) => 0,
                                    (SlotState::Garbage, SlotState::Queued) => 1,
                                    _ => unreachable!(),
                                };
                                slots[slot_to_queue].state = SlotState::Active;
                                slots[slot_to_queue].wakers.push(Arc::downgrade(&waker));
                                slot_to_queue
                            };
                            self.active_slot.store(slot_to_queue, Ordering::Release);
                            // Release the other slot
                            self.active_locks
                                .store(1 << slot_to_queue, Ordering::Release);
                            self.ring_doorbell();
                            break;
                        }
                        BitsToAcquire::One(index) => {
                            // We have the lock to the queue
                            let slot = unsafe { self.slots[index].get().as_mut_unchecked() };
                            if slot.state == SlotState::Garbage {
                                slot.state = SlotState::Queued;
                                slot.wakers.clear()
                            }
                            assert_eq!(slot.state, SlotState::Queued);
                            slot.wakers.push(Arc::downgrade(&waker));
                            // Release the lock, but only if the other slot is locked
                            loop {
                                if self
                                    .active_locks
                                    .compare_exchange(
                                        0b11,
                                        bits_to_acquire.bitmask_other(),
                                        Ordering::Release,
                                        Ordering::Relaxed,
                                    )
                                    .is_ok()
                                {
                                    break;
                                }
                                // This means that the other is not locked anymore, and it's our responsibility to ring the doorbell
                                if self
                                    .active_locks
                                    .compare_exchange(
                                        bits_to_acquire.bitmask(),
                                        0b11,
                                        Ordering::Acquire,
                                        Ordering::Relaxed,
                                    )
                                    .is_ok()
                                {
                                    slot.state = SlotState::Active;
                                    self.active_slot.store(index, Ordering::Release);
                                    // Release the other slot
                                    self.active_locks
                                        .store(bits_to_acquire.bitmask(), Ordering::Release);
                                    self.ring_doorbell();
                                    break;
                                }
                                // This means that the other one is locked again!
                                core::hint::spin_loop();
                            }
                        }
                    }
                }
                Err(active_locks) => match active_locks {
                    0b01 => {
                        bits_to_acquire = BitsToAcquire::One(1);
                    }
                    0b10 => bits_to_acquire = BitsToAcquire::One(0),
                    _ => {
                        bits_to_acquire = BitsToAcquire::Both;
                    }
                },
            }
            core::hint::spin_loop();
        }

        // Wait for EHCI interrupt to acknowledge removal
        future::poll_fn(|cx| {
            waker.waker.register(cx.waker());
            if waker.woken.load(Ordering::Relaxed) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }

    /// Call when the async advance bit is set (and interrupt will occur). Clear the bit before calling.
    pub(crate) fn handle_async_advance(&self) {
        let active_slot_index = self.active_slot.load(Ordering::Acquire);
        if active_slot_index == NO_ACTIVE_SLOT {
            log::warn!("spurrious async advance interrupt");
            return;
        }
        let active_slot = unsafe { self.slots[active_slot_index].get().as_mut_unchecked() };
        for waker in &active_slot.wakers {
            if let Some(waker) = waker.upgrade() {
                waker.woken.store(true, Ordering::Relaxed);
                waker.waker.wake();
            }
        }
        // Don't deallocate, just mark as garbage
        active_slot.state = SlotState::Garbage;
        // Release
        self.active_slot.store(NO_ACTIVE_SLOT, Ordering::Relaxed);
        let prev = self
            .active_locks
            .fetch_and(0b11 & !(1 << active_slot_index), Ordering::Release);
        // If the other slot was unlocked, it's possible it's queued
        if prev & !(1 << active_slot_index) == 0 {
            if self
                .active_locks
                .compare_exchange(0, 0b11, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                let slots = self
                    .slots
                    .each_ref()
                    .map(|slot| unsafe { slot.get().as_mut_unchecked() });
                let queued_slot_index = slots
                    .iter()
                    .position(|slot| slot.state == SlotState::Queued);
                if let Some(queued_slot_index) = queued_slot_index {
                    slots[queued_slot_index].state = SlotState::Active;
                    self.active_slot.store(queued_slot_index, Ordering::Release);
                    // Release the other slot
                    self.active_locks
                        .store(1 << queued_slot_index, Ordering::Release);
                    self.ring_doorbell();
                } else {
                    self.active_locks.store(0, Ordering::Release);
                }
            }
        }
    }

    fn ring_doorbell(&self) {
        // Ring the EHCI Async Advance Doorbell
        fence(Ordering::SeqCst);
        self.operational_regs
            .usb_cmd()
            .update(|usb_cmd| usb_cmd.with_interrupt_on_async_advance_doorbell(true));
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, VolatileFieldAccess)]
pub struct QhWithMetadata {
    pub(crate) qh: QueueHead,
    pub(crate) prev: VolatilePtr<'static, QhWithMetadata>,
    pub(crate) next: VolatilePtr<'static, QhWithMetadata>,
}
