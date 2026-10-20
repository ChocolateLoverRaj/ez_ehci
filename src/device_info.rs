use alloc::{string::String, vec::Vec};
use zerocopy::{FromBytes, Immutable, KnownLayout, transmute};

use crate::{
    DeviceDescriptor,
    common_descriptor::CommonDescriptor,
    configuration_descriptor::ConfigurationDescriptor,
    endpoint_descriptor::{
        EndpointAddress, EndpointAttributes, EndpointDescriptor, EndpointMaxPacketSize,
    },
    interface_descriptor::InterfaceDescriptor,
    setup_packet::DescriptorType,
};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, Immutable, KnownLayout)]
pub struct UsbVersion {
    pub minor: u8,
    pub major: u8,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub usb_version: UsbVersion,
    pub device_class: u8,
    pub device_sub_class: u8,
    pub device_protocol: u8,
    pub max_packet_size: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub bcd_device: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    pub configurations: Vec<ConfigurationInfo>,
}

impl DeviceInfo {
    pub fn from_descriptor(
        device_descriptor: &DeviceDescriptor,
        configurations: Vec<ConfigurationInfo>,
    ) -> Self {
        Self {
            bcd_device: device_descriptor.bcd_device.get(),
            configurations,
            device_class: device_descriptor.b_device_class,
            device_protocol: device_descriptor.b_device_protocol,
            device_sub_class: device_descriptor.b_device_sub_class,

            manufacturer: None,
            max_packet_size: device_descriptor.b_max_packet_size,
            product_id: device_descriptor.id_product.get(),
            vendor_id: device_descriptor.id_vendor.get(),
            product: None,
            serial_number: None,
            usb_version: transmute!(device_descriptor.bcd_usb),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigurationInfo {
    pub number: u8,
    pub name: Option<String>,
    pub attributes: u8,
    pub max_power: u8,
    pub interfaces: Vec<InterfaceInfo>,
}

impl ConfigurationInfo {
    pub fn from_descriptor(full_descriptor: &[u8]) -> Self {
        let configuration_descriptor = ConfigurationDescriptor::ref_from_bytes(
            &full_descriptor[..size_of::<ConfigurationDescriptor>()],
        )
        .unwrap();
        let mut interfaces = Vec::new();
        let mut position = size_of::<ConfigurationDescriptor>();
        let mut interface_index = 0;
        while interface_index < configuration_descriptor.b_num_interfaces {
            let descriptor = CommonDescriptor::ref_from_bytes(
                &full_descriptor[position..position + size_of::<CommonDescriptor>()],
            )
            .unwrap();
            if descriptor.b_descriptor_type == u8::from(DescriptorType::Interface) {
                let interface_descriptor = InterfaceDescriptor::ref_from_bytes(
                    &full_descriptor[position..position + size_of::<InterfaceDescriptor>()],
                )
                .unwrap();
                position += usize::try_from(descriptor.b_length).unwrap();
                let mut endpoints = Vec::new();
                let mut endpoint_index = 0;
                while endpoint_index < interface_descriptor.b_num_endpoints {
                    let descriptor = CommonDescriptor::ref_from_bytes(
                        &full_descriptor[position..position + size_of::<CommonDescriptor>()],
                    )
                    .unwrap();
                    if descriptor.b_descriptor_type == u8::from(DescriptorType::Endpoint) {
                        let endpoint_descriptor = EndpointDescriptor::ref_from_bytes(
                            &full_descriptor[position..position + size_of::<EndpointDescriptor>()],
                        )
                        .unwrap();
                        position += usize::try_from(descriptor.b_length).unwrap();
                        endpoints.push(EndpointInfo {
                            endpoint_address: EndpointAddress::new_with_raw_value(
                                endpoint_descriptor.b_endpoint_address,
                            ),
                            attributes: EndpointAttributes::new_with_raw_value(
                                endpoint_descriptor.bm_attributes,
                            ),
                            max_packet_size: EndpointMaxPacketSize::new_with_raw_value(
                                endpoint_descriptor.w_max_packet_size.get(),
                            ),
                            interval: endpoint_descriptor.b_interval,
                        });
                        endpoint_index += 1;
                    } else {
                        position += usize::try_from(descriptor.b_length).unwrap();
                    }
                }
                interfaces.push(InterfaceInfo {
                    class: interface_descriptor.b_interface_class,
                    sub_class: interface_descriptor.b_interface_sub_class,
                    interface_number: interface_descriptor.b_interface_number,
                    alternate_setting: interface_descriptor.b_alternate_setting,
                    protocol: interface_descriptor.b_interface_protocol,
                    name: None,
                    endpoints,
                });
                interface_index += 1;
            } else {
                position += usize::try_from(descriptor.b_length).unwrap();
            }
        }
        Self {
            number: configuration_descriptor.b_configuration_value,
            attributes: configuration_descriptor.bm_attributes,
            interfaces,
            max_power: configuration_descriptor.b_max_power,
            name: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct InterfaceInfo {
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub class: u8,
    pub sub_class: u8,
    pub protocol: u8,
    pub name: Option<String>,
    pub endpoints: Vec<EndpointInfo>,
}

#[derive(Debug, Clone, Copy)]
pub struct EndpointInfo {
    pub endpoint_address: EndpointAddress,
    pub attributes: EndpointAttributes,
    pub max_packet_size: EndpointMaxPacketSize,
    pub interval: u8,
}
