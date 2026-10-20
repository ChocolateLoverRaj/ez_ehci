use core::ptr::NonNull;

use arbitrary_int::{traits::Integer, u4, u11};
use volatile::{VolatileFieldAccess, VolatilePtr, VolatileRef};

use crate::{
    CtrlDsSegment, MappedMem, PeriodicFrameList, Qh,
    periodic_list::{PeriodicFrameListElement, PeriodicFrameListVolatileFieldAccess},
    queue_head::{QhHorizontalLinkPtr, QhVolatileFieldAccess, SelectType},
    util::get_addr_offset,
};

pub struct PeriodicListManager {
    list: VolatileRef<'static, PeriodicFrameList>,
    /// Each index represents an interval power.
    /// A QH with interval of 1ms would be in index 0.
    /// A QH with interval of 1024ms would be in index 10.
    /// If multiple QHs exist with the same interval, the last QH is stored here
    qhs: [Option<MappedMem<QhWithMetaP>>; 11],
    /// If there is at least 1 QH in the list, then this is a pointer to the first one in list index 0
    first_qh: Option<MappedMem<QhWithMetaP>>,
    ctrl_ds_segment: CtrlDsSegment,
}

impl PeriodicListManager {
    pub fn new(
        list: VolatileRef<'static, PeriodicFrameList>,
        ctrl_ds_segment: CtrlDsSegment,
    ) -> Self {
        Self {
            list,
            ctrl_ds_segment,
            qhs: [None; _],
            first_qh: None,
        }
    }

    pub fn add_qh(&mut self, qh: MappedMem<QhWithMetaP>, interval: NormalizedInterval) {
        // We won't balance the list in our implementation
        // We'll align everything to the start of the list to keep it simple
        let s_mask = match interval.micro_frames_power().value() {
            0 => 0b1111_1111,
            1 => 0b0101_0101,
            2 => 0b0001_0001,
            _ => 0b0000_0001,
        };
        let qh_ptr = unsafe { VolatilePtr::new(qh.ptr) };
        qh_ptr
            .qh()
            .endpoint_capabilities()
            .update(|endpoint_capabilities| {
                endpoint_capabilities.with_split_completion_mask(s_mask)
            });
        qh_ptr.interval().write(interval);

        // Check if there is already a QH with the same interval as us
        let qh_index = interval.frames_power().as_usize();
        if let Some(existing_qh_mem) = self.qhs[qh_index] {
            // Super easy, just insert this QH right after the existing one
            let existing_qh_ptr = unsafe { VolatilePtr::new(existing_qh_mem.ptr) };
            // Link our QH from the eHCI's perspective
            let next_phys = existing_qh_ptr.qh().horizontal_link_ptr().read();
            qh_ptr.qh().horizontal_link_ptr().write(next_phys);
            existing_qh_ptr
                .qh()
                .horizontal_link_ptr()
                .write(QhHorizontalLinkPtr::new(
                    SelectType::Qh,
                    get_addr_offset(qh.phys_addr, self.ctrl_ds_segment),
                ));
            // Update our meta-data
            // Update this node
            qh_ptr.next().write(existing_qh_ptr.next().read());
            qh_ptr.prev().write(Some(existing_qh_mem.ptr));
            // Update next node
            if let Some(next_qh_ptr) = qh_ptr.next().read() {
                let next_qh_ptr = unsafe { VolatilePtr::new(next_qh_ptr) };
                next_qh_ptr.prev().write(Some(qh.ptr));
            }
            // Update prev node
            existing_qh_ptr.next().write(Some(qh.ptr));
            // Our QH is now the last one with this interval
            self.qhs[qh_index] = Some(qh);
        } else {
            let prev_qh_power = self
                .qhs
                .iter()
                .copied()
                .enumerate()
                .take(qh_index)
                .find_map(|(index, qh)| Some((index, qh?)));
            let next_qh = if let Some((_, prev_qh)) = prev_qh_power {
                let prev_qh_ptr = unsafe { VolatilePtr::new(prev_qh.ptr) };
                if let Some(ptr) = prev_qh_ptr.qh().horizontal_link_ptr().read().ptr() {
                    Some((ptr, prev_qh_ptr.next().read().unwrap()))
                } else {
                    None
                }
            } else {
                self.first_qh.map(|mem| {
                    (
                        get_addr_offset(mem.phys_addr, self.ctrl_ds_segment),
                        mem.ptr,
                    )
                })
            };

            // If there is a QH after this one, link to it
            if let Some((next_qh_addr_offest, next_qh_non_null)) = next_qh {
                // Set the next QH as after ours
                qh_ptr
                    .qh()
                    .horizontal_link_ptr()
                    .write(QhHorizontalLinkPtr::new(
                        SelectType::Qh,
                        next_qh_addr_offest,
                    ));
                // Metadata
                qh_ptr.next().write(Some(next_qh_non_null));
                let next_qh_ptr = unsafe { VolatilePtr::new(next_qh_non_null) };
                next_qh_ptr.prev().write(Some(qh.ptr));
            } else {
                qh_ptr
                    .qh()
                    .horizontal_link_ptr()
                    .write(QhHorizontalLinkPtr::INVALID);
                qh_ptr.next().write(None);
            }

            let element_to_qh = PeriodicFrameListElement::new(
                SelectType::Qh,
                get_addr_offset(qh.phys_addr, self.ctrl_ds_segment),
            );
            if let Some((prev_qh_power, prev_qh)) = prev_qh_power {
                // Set our QH as after the prev
                let prev_qh_ptr = unsafe { VolatilePtr::new(prev_qh.ptr) };
                prev_qh_ptr
                    .qh()
                    .horizontal_link_ptr()
                    .write(QhHorizontalLinkPtr::new(
                        SelectType::Qh,
                        get_addr_offset(qh.phys_addr, self.ctrl_ds_segment),
                    ));
                // Update meta-data
                prev_qh_ptr.next().write(Some(qh.ptr));
                qh_ptr.prev().write(Some(prev_qh.ptr));
                // Add our QH to the slots
                let mut periodic_list_index = interval.frames().as_usize();
                let prev_qh_bit = 1 << prev_qh_power;
                while periodic_list_index < 1024 {
                    if periodic_list_index & prev_qh_bit == 0 {
                        self.list
                            .as_mut_ptr()
                            .elements()
                            .as_slice()
                            .index(periodic_list_index)
                            .write(element_to_qh);
                    }
                    periodic_list_index += interval.frames().as_usize();
                }
            } else {
                // Update meta-data
                qh_ptr.prev().write(None);
                // Add our QH to the slots
                let mut periodic_list_index = 0;
                while periodic_list_index < 1024 {
                    self.list
                        .as_mut_ptr()
                        .elements()
                        .as_slice()
                        .index(periodic_list_index)
                        .write(element_to_qh);
                    periodic_list_index += interval.frames().as_usize();
                }
                // We are now the QH with the largets interval, which makes us the first QH
                self.first_qh = Some(qh);
            }
            // Add to list of known QHs for interval
            self.qhs[qh_index] = Some(qh);
        }
    }

    fn remove_qh(&mut self, qh: MappedMem<QhWithMetaP>) {
        todo!()
    }
}

/// A Queue Head with meta-data for being in the periodic list.
#[repr(C)]
#[derive(VolatileFieldAccess)]
pub struct QhWithMetaP {
    pub(crate) qh: Qh,
    /// None means that this is the first QH in the list.
    prev: Option<NonNull<QhWithMetaP>>,
    /// None means that this is the last QH in the list.
    next: Option<NonNull<QhWithMetaP>>,
    interval: NormalizedInterval,
}

#[derive(Debug, Clone, Copy)]
pub struct NormalizedInterval {
    /// Valid from 0..=13.
    /// 0 means 125 us.
    /// 13 means 1024 ms.
    micro_frames_power: u4,
}

impl NormalizedInterval {
    pub fn from_micro_frames_power(micro_frames_power: u4) -> Self {
        assert!(micro_frames_power < u4::new(13));
        Self { micro_frames_power }
    }
    pub fn from_micro_frames_interval(micro_frames_interval: u16) -> Self {
        let interval_micro_frames_power = u4::new(if micro_frames_interval >= 1024 * 8 {
            13
        } else {
            micro_frames_interval.ilog2().try_into().unwrap()
        });
        Self {
            micro_frames_power: interval_micro_frames_power,
        }
    }

    pub fn micro_frames_power(&self) -> u4 {
        self.micro_frames_power
    }

    /// If the interval is <1 frame it returns 0 (meaning 1 frame).
    pub fn frames_power(&self) -> u4 {
        self.micro_frames_power.saturating_sub(u4::new(3))
    }

    /// Returns a power of 2 between 1..=1024.
    /// If the interval is <1 frame it rounds up to 1 frame.
    pub fn frames(&self) -> u11 {
        u11::new(2_u16.pow(self.frames_power().value().into()))
    }
}
