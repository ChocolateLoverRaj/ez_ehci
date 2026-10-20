use core::{ptr::NonNull, time::Duration};

use alloc::sync::Arc;
use arbitrary_int::{traits::Integer, u2, u4, u7, u11, u15};
use volatile::{VolatilePtr, VolatileRef};

use crate::{
    CtrlDsSegment, MappedMem, Qh,
    buffer_ptrs::BufferPtrs,
    device_info::EndpointInfo,
    endpoint_speed::EndpointSpeed,
    interrupt_endpoint_shared::{AddInterruptQhReq, InterruptEndpointSharedState},
    oneshot::oneshot,
    periodic_list_manager::{NormalizedInterval, QhWithMetaP, QhWithMetaPVolatileFieldAccess},
    qtd::{NextQtdPointer, Qtd, QtdVolatileFieldAccess},
    queue_head::{
        AlternateQtdLinkPtr, EndpointCapabilities, EndpointCharacteristics, QhVolatileFieldAccess,
    },
    transfer_token::{PidCode, TransferToken},
    util::get_addr_offset,
};

/// Represents an endpoint to a device. Guaranteed to be the only thing that owns this endpoint.
pub struct InterruptEndpoint {
    device_address: u7,
    shared: Arc<InterruptEndpointSharedState>,
    info: EndpointInfo,
    ctrl_ds_segment: CtrlDsSegment,
}

impl InterruptEndpoint {
    pub(crate) fn new(
        device_address: u7,
        shared: Arc<InterruptEndpointSharedState>,
        info: EndpointInfo,
        ctrl_ds_segment: CtrlDsSegment,
    ) -> Self {
        Self {
            device_address,
            shared,
            info,
            ctrl_ds_segment,
        }
    }

    pub fn endpoint_number(&self) -> u4 {
        self.info.endpoint_address.endpoint_number()
    }

    pub fn interval(&self) -> Duration {
        let microframe = Duration::from_micros(125);
        let n_microframes = 2_u16.pow(u32::from(self.info.interval - 1));
        microframe * u32::from(n_microframes)
    }

    pub fn max_packet_size(&self) -> u11 {
        self.info.max_packet_size.max_packet_size()
    }

    /// Panics if number of QTDs doesn't match the buffer length in packets.
    pub async fn watch(self, resources: WatchInterruptResources) -> WatchedInterruptEndpoint {
        let n_qtds = resources.qtds.ptr.len();
        if n_qtds == 0 {
            panic!("minimum 1 QTD required");
        }
        if resources.buffer.ptr.len() != n_qtds * self.max_packet_size().as_usize() {
            panic!("buffer len doesn't match QTDs len");
        }
        let qh_ptr = unsafe { VolatilePtr::new(resources.qh.ptr) };
        let mut qtds_ptr = unsafe { VolatileRef::new(resources.qtds.ptr) };
        let mut qh = Qh::new(
            EndpointCharacteristics::builder()
                .with_device_addr(self.device_address)
                .with_inactive_on_next_transaction(false)
                .with_endpoint_number(self.endpoint_number())
                .with_endpoint_speed(EndpointSpeed::High.into())
                .with_data_toggle_control(false)
                .with_head_of_reclamation_list_flag(false)
                .with_max_packet_len(self.max_packet_size())
                .with_endpoint_control_flag(false)
                .with_nak_count_reload(u4::ZERO)
                .build(),
            EndpointCapabilities::builder()
                .with_interrupt_schedule_mask(u8::new(1))
                .with_split_completion_mask(0)
                .with_hub_addr(u7::ZERO)
                .with_port_number(u7::ZERO)
                .with_high_bandwidth_pipe_multiplier(u2::new(1))
                .build(),
        );
        // Make sure the eHCI doesn't read the QTD pointer or transfer anything
        qh.transfer_token.set_halted(true);
        qh_ptr.qh().write(qh);
        let (tx, rx) = oneshot();
        self.shared.req_signal.signal(AddInterruptQhReq {
            qh: resources.qh,
            interval: NormalizedInterval::from_micro_frames_interval(
                2_u16.pow((self.info.interval - 1).into()),
            ),
            callback: tx,
        });
        rx.await.unwrap();

        for i in 0..n_qtds {
            let mut qtd = Qtd::new(
                TransferToken::new_active(
                    PidCode::InToken,
                    u15::new(self.max_packet_size().value()),
                )
                .with_interrupt_on_complete(true),
                BufferPtrs::new_contiguous(
                    resources.buffer.phys_addr,
                    self.max_packet_size().into(),
                ),
            );
            // wrap around if this is the last QTD
            let next_qtd_offset = if i + 1 < n_qtds { i + 1 } else { 0 };
            let next_qtd_phys_offset = get_addr_offset(
                resources.qtds.phys_addr
                    + u64::try_from(size_of::<Qtd>() * next_qtd_offset).unwrap(),
                self.ctrl_ds_segment,
            );
            qtd.next_qtd_ptr = NextQtdPointer::new_valid(next_qtd_phys_offset);
            qtd.alternate_next_qtd_ptr = AlternateQtdLinkPtr::new_valid(next_qtd_phys_offset);
            qtds_ptr.as_mut_ptr().index(i).write(qtd);
        }
        // Link the QTDs to the QH
        qh_ptr
            .qh()
            .next_qtd_pointer()
            .write(NextQtdPointer::new_valid(get_addr_offset(
                resources.qtds.phys_addr,
                self.ctrl_ds_segment,
            )));
        qh_ptr
            .qh()
            .transfer_token()
            .write(TransferToken::new_with_raw_value(0));

        WatchedInterruptEndpoint {
            qtds: qtds_ptr,
            completed_packets_index: 0,
            completed_packets: 0,
            buffer: resources.buffer,
            info: self.info,
            shared: self.shared,
        }
    }
}

pub struct WatchInterruptResources {
    pub qh: MappedMem<QhWithMetaP>,
    /// You choose the number of Qtds (at least 1 is required).
    /// More QTDs allow buffering more packets and can reduce missed packets due to the buffer being full
    /// when there is high CPU usage.
    pub qtds: MappedMem<[Qtd]>,
    /// The length of this must be number of qtds * max packet size, so there is space to store 1 packet per QTD.
    /// No alignemnt requirement.
    pub buffer: MappedMem<[u8]>,
}

pub struct WatchedInterruptEndpoint {
    qtds: VolatileRef<'static, [Qtd]>,
    buffer: MappedMem<[u8]>,
    completed_packets_index: usize,
    completed_packets: usize,
    info: EndpointInfo,
    shared: Arc<InterruptEndpointSharedState>,
}

impl WatchedInterruptEndpoint {
    /// Waits until there is at least 1 packet availalable, and then presents it as an &[u8].
    pub async fn peek(&mut self) -> &[u8] {
        loop {
            if self.completed_packets >= 1 {
                break;
            }
            if !self
                .qtds
                .as_mut_ptr()
                .index(self.completed_packets_index)
                .qtd_token()
                .read()
                .active()
            {
                self.completed_packets += 1;
            } else {
                self.shared.int_signal.wait().await;
            }
        }
        let packet_size = self.info.max_packet_size.max_packet_size().as_usize();
        let buffer_ptr = NonNull::slice_from_raw_parts(
            unsafe {
                self.buffer.ptr.cast::<u8>().offset(
                    (self.completed_packets_index * packet_size)
                        .try_into()
                        .unwrap(),
                )
            },
            packet_size,
        );
        unsafe { buffer_ptr.as_ref() }
    }

    /// Marks the first available QTD as ready to receive a new packet.
    /// Panics if there is no QTD with data.
    pub fn advance(&mut self) {
        let packet_size = self.info.max_packet_size.max_packet_size();
        let packet_size_u64 = packet_size.value().into();
        let qtd = self.qtds.as_mut_ptr().index(self.completed_packets_index);
        // Reset the offset within the page
        qtd.buffer_pointer_page_0().update(|r| {
            r.with_current_offset(
                BufferPtrs::new_contiguous(
                    self.buffer.phys_addr
                        + packet_size_u64 * u64::try_from(self.completed_packets_index).unwrap(),
                    packet_size_u64,
                )
                .offset(),
            )
        });
        // Reset the token and the page index
        qtd.qtd_token().write(
            TransferToken::new_active(PidCode::InToken, u15::new(packet_size.value()))
                .with_interrupt_on_complete(true),
        );
        self.completed_packets_index += 1;
        if self.completed_packets_index == self.qtds.as_mut_ptr().len() {
            self.completed_packets_index = 0;
        }
        self.completed_packets -= 1;
    }
}
