use alloc::{sync::Arc, vec::Vec};
use arbitrary_int::{traits::Integer, u4, u7, u11, u15};
use volatile::VolatilePtr;
use zerocopy::{FromBytes, transmute};

use crate::{
    CtrlDsSegment, DeviceDescriptor, InterruptEndpoint, MappedMem, Qh,
    async_list_manager::{QhWithMetaA, QhWithMetaAVolatileFieldAccess},
    buffer_ptrs::BufferPtrs,
    device_info::{ConfigurationInfo, DeviceInfo},
    device_shared::{AddEndpointReq, AddQhReq, DeviceReq, DeviceSharedState},
    endpoint_descriptor::TransferType,
    endpoint_speed::EndpointSpeed,
    hub::Hub,
    interrupt_endpoint_shared::InterruptEndpointSharedState,
    oneshot::oneshot,
    qtd::{NextQtdPointer, Qtd, QtdVolatileFieldAccess},
    queue_head::{
        AlternateQtdLinkPtr, EndpointCapabilities, EndpointCharacteristics, QhVolatileFieldAccess,
    },
    setup_packet::SetupPacket,
    transfer_token::{PidCode, TransferToken},
    util::get_addr_offset,
};

/// A device was reset, confirmed to be high speed, and assigned an address.
/// Note that during the lifetime of this struct ('static) the device might've been removed,
/// so this struct isn't a guarantee that the device is still connected.
/// The eHCI might've even been stopped since this struct was created.
pub struct Device {
    address: u7,
    shared: Arc<DeviceSharedState>,
    ctrl_ds_segment: CtrlDsSegment,
}

impl Device {
    pub(crate) fn new(
        shared: Arc<DeviceSharedState>,
        ctrl_ds_segment: CtrlDsSegment,
        address: u7,
    ) -> Self {
        Self {
            ctrl_ds_segment,
            address,
            shared,
        }
    }

    pub fn address(&self) -> u7 {
        self.address
    }

    pub async unsafe fn add_ep0(self, qh_mem: MappedMem<QhWithMetaA>) -> DeviceWithEp0 {
        let mut qh = Qh::new(
            EndpointCharacteristics::new_with_raw_value(0)
                .with_head_of_reclamation_list_flag(false)
                .with_device_addr(self.address)
                .with_endpoint_number({
                    // control endpoint
                    u4::ZERO
                })
                .with_endpoint_speed(EndpointSpeed::High.into())
                .with_max_packet_len(u11::new({
                    // We know the device is USB 2.0, so min max packet len is 64
                    // We can use >64 after we read the device descriptor
                    64
                })),
            EndpointCapabilities::ZERO,
        );
        // Make sure the eHCI doesn't try to read any QTDs or do any transfers
        qh.next_qtd_pointer = NextQtdPointer::INVALID;
        qh.transfer_token.set_halted(true);
        let qh_ptr = unsafe { VolatilePtr::new(qh_mem.ptr) };
        qh_ptr.qh().write(qh);

        let (tx, rx) = oneshot();
        self.shared.req_signal.signal(DeviceReq::AddQh(AddQhReq {
            add_qh: qh_mem,
            callback: tx,
        }));
        rx.await.unwrap();

        DeviceWithEp0 {
            address: self.address,
            ctrl_ds_segment: self.ctrl_ds_segment,
            qh: qh_mem,
            shared: self.shared,
        }
    }
}
const MAX_CONFIGURATION_DESCRIPTOR_LEN: usize = {
    // Technically this should be 65_535 as per the spec, but QEMU does not support control transfers with a max len >4096.
    // So we cap this at 4096 since this should support all real hardware anyways
    4096
};

pub struct GetInfoResources {
    pub qtds: [MappedMem<Qtd>; 3],
    pub setup_packet: MappedMem<[u8; 8]>,
    pub buffer: MappedMem<[u8; MAX_CONFIGURATION_DESCRIPTOR_LEN]>,
}

pub struct DeviceWithEp0 {
    address: u7,
    shared: Arc<DeviceSharedState>,
    ctrl_ds_segment: CtrlDsSegment,
    qh: MappedMem<QhWithMetaA>,
}

impl DeviceWithEp0 {
    pub fn address(&self) -> u7 {
        self.address
    }

    /// This function does not write to the setup buffer. It's the caller's responsibility to do that before calling this fn.
    unsafe fn attach_get_descriptor_request(
        &self,
        qtds: [MappedMem<Qtd>; 3],
        setup_buffer: MappedMem<[u8; 8]>,
        buffer: MappedMem<[u8]>,
    ) {
        // SETUP
        let mut setup_qtd = Qtd::new(
            TransferToken::new_active(PidCode::SetupToken, u15::new(8)),
            BufferPtrs::new_contiguous(setup_buffer.phys_addr, 8),
        );
        setup_qtd.next_qtd_ptr =
            NextQtdPointer::new_valid(get_addr_offset(qtds[1].phys_addr, self.ctrl_ds_segment));
        let setup_qtd_ptr = unsafe { VolatilePtr::new(qtds[0].ptr) };
        setup_qtd_ptr.write(setup_qtd);
        // IN
        let mut in_qtd = Qtd::new(
            TransferToken::new_active(
                PidCode::InToken,
                u15::new(buffer.ptr.len().try_into().unwrap()),
            )
            .with_data_toggle(false)
            .with_interrupt_on_complete(true),
            BufferPtrs::new_contiguous(buffer.phys_addr, buffer.ptr.len().try_into().unwrap()),
        );
        let status_addr_offset = get_addr_offset(qtds[2].phys_addr, self.ctrl_ds_segment);
        in_qtd.next_qtd_ptr = NextQtdPointer::new_valid(status_addr_offset);
        in_qtd.alternate_next_qtd_ptr = AlternateQtdLinkPtr::new_valid(status_addr_offset);
        let in_qtd_ptr = unsafe { VolatilePtr::new(qtds[1].ptr) };
        in_qtd_ptr.write(in_qtd);
        // Status
        let status_qtd = Qtd::new(
            TransferToken::new_active(PidCode::OutToken, u15::ZERO)
                .with_data_toggle(true)
                .with_interrupt_on_complete(true),
            BufferPtrs::ZERO,
        );
        let status_qtd_ptr = unsafe { VolatilePtr::new(qtds[2].ptr) };
        status_qtd_ptr.write(status_qtd);

        // Actually add the QTD chain to the QH
        let qh_ptr = unsafe { VolatilePtr::new(self.qh.ptr) };
        qh_ptr
            .qh()
            .next_qtd_pointer()
            .write(NextQtdPointer::new_valid(get_addr_offset(
                qtds[0].phys_addr,
                self.ctrl_ds_segment,
            )));
        qh_ptr
            .qh()
            .transfer_token()
            .write(TransferToken::new_with_raw_value(0));
    }

    async unsafe fn get_descriptor<'a>(
        &mut self,
        qtds: [MappedMem<Qtd>; 3],
        setup_buffer: MappedMem<[u8; 8]>,
        buffer: MappedMem<[u8]>,
    ) -> &'a [u8] {
        unsafe { self.attach_get_descriptor_request(qtds, setup_buffer, buffer) };
        let in_qtd_ptr = unsafe { VolatilePtr::new(qtds[1].ptr) };
        let status_qtd_ptr = unsafe { VolatilePtr::new(qtds[2].ptr) };
        loop {
            if !status_qtd_ptr.qtd_token().read().active() {
                break;
            }
            self.shared.usb_int_signal.wait().await;
        }

        let buffer_ptr = unsafe { VolatilePtr::new(buffer.ptr) };
        let bytes_transferred = MAX_CONFIGURATION_DESCRIPTOR_LEN
            - in_qtd_ptr
                .qtd_token()
                .read()
                .total_bytes_to_transfer()
                .value()
                .as_usize();
        unsafe { buffer_ptr.index(..bytes_transferred).as_raw_ptr().as_ref() }
    }

    async unsafe fn get_configuration_descriptor<'a>(
        &mut self,
        descriptor_num: u8,
        qtds: [MappedMem<Qtd>; 3],
        setup_buffer: MappedMem<[u8; 8]>,
        buffer: MappedMem<[u8; MAX_CONFIGURATION_DESCRIPTOR_LEN]>,
    ) -> &'a [u8] {
        let setup_packet_ptr = unsafe { VolatilePtr::new(setup_buffer.ptr) };
        setup_packet_ptr.write(transmute!(SetupPacket::new_get_configuration_descriptor(
            MAX_CONFIGURATION_DESCRIPTOR_LEN.try_into().unwrap(),
            descriptor_num
        )));
        (unsafe { self.get_descriptor(qtds, setup_buffer, buffer.cast_slice()) }).await
    }

    pub async unsafe fn get_info(mut self, resources: GetInfoResources) -> DeviceWithInfo {
        let setup_packet_buffer_ptr = unsafe { VolatilePtr::new(resources.setup_packet.ptr) };
        setup_packet_buffer_ptr.write(transmute!(SetupPacket::new_get_device_descriptor(18)));
        let device_descriptor = unsafe {
            self.get_descriptor(
                resources.qtds,
                resources.setup_packet,
                resources.buffer.cast_slice(),
            )
        }
        .await;
        log::info!("device descriptor: {device_descriptor:02X?}");
        let device_descriptor = DeviceDescriptor::read_from_bytes(device_descriptor).unwrap();
        log::info!("{device_descriptor:#?}");
        let mut configurations = Vec::new();
        for n in 0..device_descriptor.b_num_configurations {
            let full_descriptor = unsafe {
                self.get_configuration_descriptor(
                    n,
                    resources.qtds,
                    resources.setup_packet,
                    resources.buffer,
                )
            }
            .await;
            log::info!("full descriptor: {full_descriptor:02X?}");
            configurations.push(ConfigurationInfo::from_descriptor(full_descriptor));
        }
        DeviceWithInfo {
            address: self.address,
            shared: self.shared,
            ctrl_ds_segment: self.ctrl_ds_segment,
            qh: self.qh,
            info: DeviceInfo::from_descriptor(&device_descriptor, configurations),
        }
    }
}

pub struct DeviceWithInfo {
    address: u7,
    shared: Arc<DeviceSharedState>,
    ctrl_ds_segment: CtrlDsSegment,
    qh: MappedMem<QhWithMetaA>,
    info: DeviceInfo,
}

impl DeviceWithInfo {
    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    async unsafe fn do_setup_transfer(
        &mut self,
        qtds: [MappedMem<Qtd>; 2],
        setup_buffer: MappedMem<[u8; 8]>,
    ) {
        let qh_ptr = unsafe { VolatilePtr::new(self.qh.ptr) };
        let setup_qtd_ptr = unsafe { VolatilePtr::new(qtds[0].ptr) };
        let status_qtd_ptr = unsafe { VolatilePtr::new(qtds[1].ptr) };
        let mut setup_qtd = Qtd::new(
            TransferToken::new_active(PidCode::SetupToken, u15::new(8)),
            BufferPtrs::new_contiguous(setup_buffer.phys_addr.into(), 8),
        );
        setup_qtd.next_qtd_ptr =
            NextQtdPointer::new_valid(get_addr_offset(qtds[1].phys_addr, self.ctrl_ds_segment));
        setup_qtd_ptr.write(setup_qtd);
        let status_qtd = Qtd::new(
            TransferToken::new_active(PidCode::InToken, u15::ZERO)
                .with_data_toggle(true)
                .with_interrupt_on_complete(true),
            BufferPtrs::ZERO,
        );
        status_qtd_ptr.write(status_qtd);
        qh_ptr
            .qh()
            .next_qtd_pointer()
            .write(NextQtdPointer::new_valid(get_addr_offset(
                qtds[0].phys_addr,
                self.ctrl_ds_segment,
            )));
        qh_ptr
            .qh()
            .transfer_token()
            .write(TransferToken::new_with_raw_value(0));
        loop {
            if !status_qtd_ptr.qtd_token().read().active() {
                break;
            }
            self.shared.usb_int_signal.wait().await;
        }
    }

    /// Note that this function takes the *index* of the configuration from the info Vec, not the u8 configuration *number*. This is for efficiency.
    pub async unsafe fn set_configuration(
        &mut self,
        configuration_index: usize,
        resources: SetConfigurationResources,
    ) -> ConfiguredDeviceOrHub {
        let configuration = &self.info.configurations[configuration_index];
        let setup_packet_ptr = unsafe { VolatilePtr::new(resources.setup_buffer.ptr) };
        setup_packet_ptr.write(transmute!(SetupPacket::new_set_configuration(
            configuration.number
        )));
        (unsafe { self.do_setup_transfer(resources.qtds, resources.setup_buffer) }).await;
        let configuration = &self.info.configurations[configuration_index];
        let configuration = Configuration {
            interfaces: configuration
                .interfaces
                .iter()
                .map(|interface| Interface {
                    endpoints: interface
                        .endpoints
                        .iter()
                        .map(|endpoint| {
                            match TransferType::from(endpoint.attributes.transfer_type()) {
                                TransferType::Control => EndpointEnum::Control,
                                TransferType::Isochronous => EndpointEnum::Isochronous,
                                TransferType::Interrupt => {
                                    let shared = Arc::new(InterruptEndpointSharedState::default());
                                    self.shared.req_signal.signal(DeviceReq::AddEndpoint(
                                        AddEndpointReq {
                                            endpoint_shared: shared.clone(),
                                        },
                                    ));
                                    EndpointEnum::Interrupt(InterruptEndpoint::new(
                                        self.address,
                                        shared,
                                        *endpoint,
                                        self.ctrl_ds_segment,
                                    ))
                                }
                                TransferType::Bulk => EndpointEnum::Bulk,
                            }
                        })
                        .collect(),
                })
                .collect(),
        };
        if self.info.device_class == 0x9 {
            let EndpointEnum::Interrupt(ep) = configuration
                .interfaces
                .into_iter()
                .next()
                .unwrap()
                .endpoints
                .into_iter()
                .next()
                .unwrap()
            else {
                panic!("unexpected hub endpoint type");
            };
            ConfiguredDeviceOrHub::Hub(Hub {
                interrupt_endpoint: ep,
            })
        } else {
            ConfiguredDeviceOrHub::Device(configuration)
        }
    }
}

pub struct SetConfigurationResources {
    pub qtds: [MappedMem<Qtd>; 2],
    pub setup_buffer: MappedMem<[u8; 8]>,
}

pub struct Configuration {
    pub interfaces: Vec<Interface>,
}

pub struct Interface {
    pub endpoints: Vec<EndpointEnum>,
}

pub enum EndpointEnum {
    Control,
    Bulk,
    Interrupt(InterruptEndpoint),
    Isochronous,
}

pub enum ConfiguredDeviceOrHub {
    Device(Configuration),
    Hub(Hub),
}
