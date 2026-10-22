use alloc::sync::Arc;
use arbitrary_int::{traits::Integer, u4, u7, u11, u15};
use volatile::{VolatilePtr, VolatileRef};

use crate::{
    CtrlDsSegment, MappedMem, Qh,
    async_list_manager::{QhWithMetaA, QhWithMetaAVolatileFieldAccess},
    buffer_ptrs::BufferPtrs,
    control_endpoint_shared::ControlEndpointSharedState,
    endpoint_speed::EndpointSpeed,
    qtd::{NextQtdPointer, Qtd, QtdVolatileFieldAccess},
    queue_head::{
        AlternateQtdLinkPtr, EndpointCapabilities, EndpointCharacteristics, QhVolatileFieldAccess,
    },
    transfer_token::{PidCode, TransferToken},
    util::get_addr_offset,
};

pub struct ControlEndpoint {
    shared: Arc<ControlEndpointSharedState>,
    device_address: u7,
    endpoint_number: u4,
    ctrl_ds_segment: CtrlDsSegment,
}

impl ControlEndpoint {
    pub(crate) fn new(
        shared: Arc<ControlEndpointSharedState>,
        device_address: u7,
        endpoint_number: u4,
        ctrl_ds_segment: CtrlDsSegment,
    ) -> Self {
        Self {
            shared,
            device_address,
            endpoint_number,
            ctrl_ds_segment,
        }
    }

    pub async unsafe fn load(self, qh_mem: MappedMem<QhWithMetaA>) -> LoadedControlEndpoint {
        let mut qh = Qh::new(
            EndpointCharacteristics::new_with_raw_value(0)
                .with_head_of_reclamation_list_flag(false)
                .with_device_addr(self.device_address)
                .with_endpoint_number(self.endpoint_number)
                .with_endpoint_speed(EndpointSpeed::High.into())
                .with_max_packet_len(u11::new({
                    // We know the device is USB 2.0, so min max packet len is 64
                    // We can use >64 after we read the device descriptor
                    64
                }))
                .with_data_toggle_control(true),
            EndpointCapabilities::ZERO,
        );
        // Make sure the eHCI doesn't try to read any QTDs or do any transfers
        qh.next_qtd_pointer = NextQtdPointer::INVALID;
        qh.transfer_token.set_halted(true);
        let mut qh_ptr = unsafe { VolatileRef::new(qh_mem.ptr) };
        qh_ptr.as_mut_ptr().qh().write(qh);
        self.shared.req_signal.signal(qh_mem);
        LoadedControlEndpoint {
            qh: qh_ptr,
            ctrl_ds_segment: self.ctrl_ds_segment,
            shared: self.shared,
        }
    }
}

pub struct LoadedControlEndpoint {
    shared: Arc<ControlEndpointSharedState>,
    qh: VolatileRef<'static, QhWithMetaA>,
    ctrl_ds_segment: CtrlDsSegment,
}

impl LoadedControlEndpoint {
    unsafe fn write_setup_qtd(
        &self,
        setup_packet: MappedMem<[u8; 8]>,
        setup_qtd_mem: MappedMem<Qtd>,
        next_qtd_mem: MappedMem<Qtd>,
    ) {
        let mut setup_qtd = Qtd::new(
            TransferToken::new_active(PidCode::SetupToken, u15::new(8)),
            BufferPtrs::new_contiguous(setup_packet.phys_addr, 8),
        );
        setup_qtd.next_qtd_ptr = NextQtdPointer::new_valid(get_addr_offset(
            next_qtd_mem.phys_addr,
            self.ctrl_ds_segment,
        ));
        let setup_qtd_ptr = unsafe { VolatilePtr::new(setup_qtd_mem.ptr) };
        setup_qtd_ptr.write(setup_qtd);
    }

    unsafe fn write_status_qtd(&self, status_qtd_mem: MappedMem<Qtd>, pid: PidCode) {
        let status_qtd = Qtd::new(
            TransferToken::new_active(pid, u15::ZERO)
                .with_data_toggle(true)
                .with_interrupt_on_complete(true),
            BufferPtrs::ZERO,
        );
        let mut status_qtd_ptr = unsafe { VolatileRef::new(status_qtd_mem.ptr) };
        status_qtd_ptr.as_mut_ptr().write(status_qtd);
    }

    unsafe fn attach_qtd(&mut self, qtd: MappedMem<Qtd>) {
        self.qh
            .as_mut_ptr()
            .qh()
            .next_qtd_pointer()
            .write(NextQtdPointer::new_valid(get_addr_offset(
                qtd.phys_addr,
                self.ctrl_ds_segment,
            )));
        self.qh
            .as_mut_ptr()
            .qh()
            .transfer_token()
            .write(TransferToken::new_with_raw_value(0));
    }

    async fn wait_for_qtd_complete(&mut self, mut qtd: VolatileRef<'_, Qtd>) {
        loop {
            if !qtd.as_mut_ptr().qtd_token().read().active() {
                break;
            }
            self.shared.int_signal.wait().await;
        }
    }

    pub async unsafe fn transfer_no_data(
        &mut self,
        setup_packet: MappedMem<[u8; 8]>,
        qtds: [MappedMem<Qtd>; 2],
    ) {
        let setup_qtd_mem = qtds[0];
        let status_qtd_mem = qtds[1];
        unsafe {
            self.write_setup_qtd(setup_packet, setup_qtd_mem, status_qtd_mem);
        }
        unsafe {
            self.write_status_qtd(status_qtd_mem, PidCode::InToken);
        }
        unsafe { self.attach_qtd(setup_qtd_mem) };
        let status_qtd_ptr = unsafe { VolatileRef::new(qtds[1].ptr) };
        self.wait_for_qtd_complete(status_qtd_ptr).await;
    }

    /// Returns bytes read.
    pub async unsafe fn transfer_in(
        &mut self,
        setup_packet: MappedMem<[u8; 8]>,
        buffers: &[MappedMem<[u8]>],
        qtds: &[MappedMem<Qtd>],
    ) -> u16 {
        assert!(!buffers.is_empty());
        assert_eq!(qtds.len(), 1 + buffers.len() + 1);
        let setup_qtd_mem = qtds[0];
        let status_qtd_mem = *qtds.last().unwrap();
        unsafe {
            self.write_setup_qtd(setup_packet, setup_qtd_mem, qtds[1]);
        }
        let status_addr_offset = get_addr_offset(status_qtd_mem.phys_addr, self.ctrl_ds_segment);
        let alternate_qtd_ptr = AlternateQtdLinkPtr::new_valid(status_addr_offset);
        for i in 0..buffers.len() {
            let buffer = buffers[i];
            let in_qtd_mem = qtds[i + 1];
            let mut in_qtd = Qtd::new(
                TransferToken::new_active(
                    PidCode::InToken,
                    u15::new(buffer.ptr.len().try_into().unwrap()),
                )
                .with_data_toggle(true)
                .with_interrupt_on_complete(true),
                BufferPtrs::new_contiguous(buffer.phys_addr, buffer.ptr.len().try_into().unwrap()),
            );
            in_qtd.next_qtd_ptr = NextQtdPointer::new_valid(get_addr_offset(
                qtds[1 + i + 1].phys_addr,
                self.ctrl_ds_segment,
            ));
            in_qtd.alternate_next_qtd_ptr = alternate_qtd_ptr;
            let in_qtd_ptr = unsafe { VolatilePtr::new(in_qtd_mem.ptr) };
            in_qtd_ptr.write(in_qtd);
        }
        unsafe {
            self.write_status_qtd(status_qtd_mem, PidCode::OutToken);
        }
        unsafe { self.attach_qtd(qtds[0]) };
        let status_qtd_ptr = unsafe { VolatileRef::new(status_qtd_mem.ptr) };
        self.wait_for_qtd_complete(status_qtd_ptr).await;

        let mut bytes_transferred = 0;
        for i in 0..buffers.len() {
            let buffer_mem = buffers[i];
            let in_qtd_mem = qtds[i + 1];
            let in_qtd_ptr = unsafe { VolatilePtr::new(in_qtd_mem.ptr) };
            let max_transfer_size = u16::try_from(buffer_mem.ptr.len()).unwrap();
            bytes_transferred += max_transfer_size
                - in_qtd_ptr
                    .qtd_token()
                    .read()
                    .total_bytes_to_transfer()
                    .value();
        }
        bytes_transferred
    }

    pub async fn transfer_out(
        setup_token: MappedMem<[u8; 8]>,
        buffers: &mut [MappedMem<[u8]>],
        qtds: &mut [MappedMem<Qtd>],
    ) {
        todo!()
    }
}
