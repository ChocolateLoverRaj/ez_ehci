use alloc::{sync::Arc, vec::Vec};
use arbitrary_int::{traits::Integer, u4, u7, u11, u15};
use volatile::VolatilePtr;
use zerocopy::{FromBytes, transmute};

use crate::{
    CtrlDsSegment, DeviceDescriptor, InterruptEndpoint, MappedMem,
    async_list_manager::QhWithMetaA,
    control_endpoint::{ControlEndpoint, LoadedControlEndpoint},
    control_endpoint_shared::ControlEndpointSharedState,
    device_info::{ConfigurationInfo, DeviceInfo},
    device_shared::{AddEndpointReq, DeviceSharedState},
    endpoint_descriptor::TransferType,
    hub::Hub,
    interrupt_endpoint_shared::InterruptEndpointSharedState,
    qtd::Qtd,
    setup_packet::SetupPacket,
};

/// A device was reset, confirmed to be high speed, and assigned an address.
/// Note that during the lifetime of this struct ('static) the device might've been removed,
/// so this struct isn't a guarantee that the device is still connected.
/// The eHCI might've even been stopped since this struct was created.
pub struct Device {
    address: u7,
    shared: Arc<DeviceSharedState>,
    ctrl_ds_segment: CtrlDsSegment,
    ep0: ControlEndpoint,
}

impl Device {
    pub(crate) fn new(
        shared: Arc<DeviceSharedState>,
        ctrl_ds_segment: CtrlDsSegment,
        address: u7,
        ep0_shared: Arc<ControlEndpointSharedState>,
    ) -> Self {
        Self {
            ctrl_ds_segment,
            address,
            shared,
            ep0: ControlEndpoint::new(ep0_shared, address, u4::ZERO, ctrl_ds_segment),
        }
    }

    pub fn address(&self) -> u7 {
        self.address
    }

    pub async unsafe fn load(self, qh_mem: MappedMem<QhWithMetaA>) -> LoadedDevice {
        let ep0 = unsafe { self.ep0.load(qh_mem) }.await;

        LoadedDevice {
            address: self.address,
            ctrl_ds_segment: self.ctrl_ds_segment,
            shared: self.shared,
            ep0,
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

pub struct LoadedDevice {
    address: u7,
    shared: Arc<DeviceSharedState>,
    ctrl_ds_segment: CtrlDsSegment,
    ep0: LoadedControlEndpoint,
}

impl LoadedDevice {
    pub fn address(&self) -> u7 {
        self.address
    }

    async unsafe fn get_descriptor<'a>(
        &mut self,
        qtds: [MappedMem<Qtd>; 3],
        setup_buffer: MappedMem<[u8; 8]>,
        buffer: MappedMem<[u8]>,
    ) -> &'a [u8] {
        let buffers = [buffer];
        let bytes_transferred =
            unsafe { self.ep0.transfer_in(setup_buffer, &buffers, &qtds) }.await;
        let buffer_ptr = unsafe { VolatilePtr::new(buffer.ptr) };
        unsafe {
            buffer_ptr
                .index(..bytes_transferred.try_into().unwrap())
                .as_raw_ptr()
                .as_ref()
        }
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
        log::debug!("{device_descriptor:#?}");
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
            ep0: self.ep0,
            info: DeviceInfo::from_descriptor(&device_descriptor, configurations),
        }
    }
}

pub struct DeviceWithInfo {
    address: u7,
    shared: Arc<DeviceSharedState>,
    ctrl_ds_segment: CtrlDsSegment,
    ep0: LoadedControlEndpoint,
    info: DeviceInfo,
}

impl DeviceWithInfo {
    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    /// Note that this function takes the *index* of the configuration from the info Vec, not the u8 configuration *number*. This is for efficiency.
    pub async unsafe fn set_configuration(
        mut self,
        configuration_index: usize,
        resources: SetConfigurationResources,
    ) -> ConfiguredDeviceOrHub {
        let configuration = &self.info.configurations[configuration_index];
        let setup_packet_ptr = unsafe { VolatilePtr::new(resources.setup_buffer.ptr) };
        setup_packet_ptr.write(transmute!(SetupPacket::new_set_configuration(
            configuration.number
        )));
        (unsafe {
            self.ep0
                .transfer_no_data(resources.setup_buffer, resources.qtds)
        })
        .await;
        let configuration = &self.info.configurations[configuration_index];
        let configuration = DeviceWithConfigurationSet {
            endpoint_0: self.ep0,
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
                                    self.shared.req_signal.signal(AddEndpointReq {
                                        endpoint_shared: shared.clone(),
                                    });
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
                endpoint_0: configuration.endpoint_0,
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

pub struct DeviceWithConfigurationSet {
    pub endpoint_0: LoadedControlEndpoint,
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
    Device(DeviceWithConfigurationSet),
    Hub(Hub),
}
