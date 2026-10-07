use core::{array, future, mem::offset_of, ops::Index, ptr::NonNull, task::Poll};

use alloc::{string::String, sync::Arc, vec::Vec};
use arbitrary_int::{traits::Integer, u2, u4, u7, u11, u15};
use embedded_hal_async::delay::DelayNs;
use volatile::{VolatileFieldAccess, VolatilePtr};
use zerocopy::{FromBytes, Immutable, KnownLayout, transmute};

use crate::{
    DeviceDescriptor, MappedMem, PeriodicFrameList, QueueHead,
    buffer_ptrs::BufferPtrs,
    common_descriptor::CommonDescriptor,
    configuration_descriptor::ConfigurationDescriptor,
    endpoint_descriptor::{
        EndpointAddress, EndpointAttributes, EndpointDescriptor, EndpointMaxPacketSize,
    },
    endpoint_speed::EndpointSpeed,
    hub_descriptor::HubDescriptor,
    interface_descriptor::InterfaceDescriptor,
    irq_handler::UsbIntWakers,
    operational_regs::PortScReg,
    periodic_list::{PeriodicFrameListElement, PeriodicFrameListVolatileFieldAccess},
    qh_manager::{QhManager, QhWithMetadata, QhWithMetadataVolatileFieldAccess},
    qtd::{NextQtdPointer, Qtd, QtdVolatileFieldAccess},
    queue_head::{
        AlternateQtdLinkPtr, CurrentQtdLinkPtr, EndpointCapabilities, EndpointCharacteristics,
        QueueHeadVolatileFieldAccess, SelectType,
    },
    root_port_number::RootPortNumber,
    setup_packet::{DescriptorType, SetupPacket},
    transfer_token::{PidCode, TransferToken},
};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, Immutable, KnownLayout)]
pub struct UsbVersion {
    minor: u8,
    major: u8,
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
    pub configurations: Vec<Configuration>,
}

#[derive(Debug, Clone)]
pub struct Configuration {
    value: u8,
    name: Option<String>,
    attributes: u8,
    max_power: u8,
    interfaces: Vec<Interface>,
}

#[derive(Debug, Clone)]
pub struct Interface {
    interface_number: u8,
    alternate_setting: u8,
    class: u8,
    sub_class: u8,
    protocol: u8,
    name: Option<String>,
    endpoints: Vec<Endpoint>,
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    endpoint_address: EndpointAddress,
    attributes: EndpointAttributes,
    max_packet_size: EndpointMaxPacketSize,
    interval: u8,
}

const MAX_CONFIGURATION_DESCRIPTOR_LEN: usize = {
    // Technically this should be 65_535 as per the spec, but QEMU does not support control transfers with a max len >4096.
    // So we cap this at 4096 since this should support all real hardware anyways
    4096
};

#[repr(C)]
#[derive(Debug, VolatileFieldAccess, Clone, Copy)]
pub struct GetInfoBuffer {
    /// Referenced as &mut [MaybeUninit<u8>] by QTDs 1..5
    /// This is the max size of a configuration descriptor, so that we can transfer a configuration descriptor no matter how big it is.
    buffer: [u8; MAX_CONFIGURATION_DESCRIPTOR_LEN],
    qh: QhWithMetadata,
    /// 0: SETUP
    /// 1: IN
    /// 2: STATUS
    qtds: [Qtd; 3],
    /// Referenced as &[u8] by QTD 0
    setup_packet: [u8; 8],
}

/// Let's you talk to a USB device on a root port.
#[derive(Debug)]
pub struct Device {
    pub(crate) root_port_number: RootPortNumber,
    pub(crate) port_sc_reg: VolatilePtr<'static, PortScReg>,
    pub(crate) addr: u7,
    pub(crate) qh_manager: Arc<QhManager>,
    pub(crate) port_wakers: UsbIntWakers,
    pub(crate) periodic_list: VolatilePtr<'static, PeriodicFrameList>,
}

impl Device {
    async fn wait_for_qtd(&mut self, qtd: VolatilePtr<'_, Qtd>) {
        future::poll_fn(|context| {
            self.port_wakers[u4::from(self.root_port_number).as_usize()].register(context.waker());
            if !qtd.qtd_token().read().active() {
                return Poll::Ready(());
            }
            Poll::Pending
        })
        .await;
    }

    /// Returns the size of the configuration descriptor, which is stored in the buffer.
    async fn get_descriptor(
        &mut self,
        qh_mem: MappedMem<QhWithMetadata>,
        qtds_mem: MappedMem<[Qtd; 3]>,
        setup_packet_mem: MappedMem<[u8; 8]>,
        buffer_mem: MappedMem<[u8]>,
    ) -> usize {
        let qh_ptr = unsafe { VolatilePtr::new(qh_mem.ptr) };
        let qtds_ptr = unsafe { VolatilePtr::new(qtds_mem.ptr) };

        let mut qh = QueueHead::new(
            EndpointCharacteristics::new_with_raw_value(0)
                .with_head_of_reclamation_list_flag(false)
                .with_device_addr(self.addr)
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
        let status_qtd_phys_addr =
            qtds_mem.phys_addr + u32::try_from(size_of::<Qtd>()).unwrap() * 2;
        let in_bytes_to_transfer = u15::new(buffer_mem.ptr.len().try_into().unwrap());
        let mut qtds = [
            // SETUP
            Qtd::new(
                TransferToken::new_active(PidCode::SetupToken, u15::new(8)),
                BufferPtrs::new_contiguous(setup_packet_mem.phys_addr.into(), 8),
            ),
            // IN
            {
                let mut qtd = Qtd::new(
                    TransferToken::new_active(PidCode::InToken, in_bytes_to_transfer)
                        .with_data_toggle(false)
                        .with_interrupt_on_complete(true),
                    BufferPtrs::new_contiguous(
                        u64::from(buffer_mem.phys_addr),
                        buffer_mem.ptr.len().try_into().unwrap(),
                    ),
                );
                qtd.alternate_next_qtd_ptr = AlternateQtdLinkPtr::new(status_qtd_phys_addr);
                qtd
            },
            // Status
            Qtd::new(
                TransferToken::new_active(PidCode::OutToken, u15::ZERO)
                    .with_data_toggle(true)
                    .with_interrupt_on_complete(true),
                BufferPtrs::ZERO,
            ),
        ];
        qh.link_contiguous_qtds(&mut qtds, qtds_mem.phys_addr);
        qh_ptr.qh().write(qh);
        qtds_ptr.write(qtds);

        self.qh_manager.add_qh_to_async_list(qh_mem);
        self.wait_for_qtd(qtds_ptr.as_slice().index(1)).await;
        self.wait_for_qtd(qtds_ptr.as_slice().index(2)).await;
        self.qh_manager
            .remove_qh(qh_mem, self.root_port_number.into())
            .await;

        let descriptor_len = in_bytes_to_transfer
            - qtds_ptr
                .as_slice()
                .index(1)
                .qtd_token()
                .read()
                .total_bytes_to_transfer();
        descriptor_len.as_usize()
    }

    async fn get_device_descriptor(
        &mut self,
        qh_mem: MappedMem<QhWithMetadata>,
        qtds_mem: MappedMem<[Qtd; 3]>,
        setup_packet_mem: MappedMem<[u8; 8]>,
        buffer_mem: MappedMem<[u8; 18]>,
    ) -> DeviceDescriptor {
        let setup_packet_buffer_ptr = unsafe { VolatilePtr::new(setup_packet_mem.ptr) };
        setup_packet_buffer_ptr.write(transmute!(SetupPacket::new_get_device_descriptor(18)));
        let buffer_ptr = unsafe { VolatilePtr::new(buffer_mem.ptr) };
        let bytes_transferred = self
            .get_descriptor(
                qh_mem,
                qtds_mem,
                setup_packet_mem,
                MappedMem {
                    phys_addr: buffer_mem.phys_addr,
                    ptr: NonNull::slice_from_raw_parts(buffer_mem.ptr.cast(), 18),
                },
            )
            .await;
        let descriptor_bytes = unsafe {
            buffer_ptr
                .as_slice()
                .index(..usize::try_from(bytes_transferred).unwrap())
                .as_raw_ptr()
                .as_ref()
        };
        DeviceDescriptor::read_from_bytes(descriptor_bytes).unwrap()
    }

    /// Returns the size of the configuration descriptor, which is stored in the buffer.
    async fn get_configuration_descriptor(
        &mut self,
        descriptor_num: u8,
        qh_mem: MappedMem<QhWithMetadata>,
        qtds_mem: MappedMem<[Qtd; 3]>,
        setup_packet_mem: MappedMem<[u8; 8]>,
        buffer_mem: MappedMem<[u8; MAX_CONFIGURATION_DESCRIPTOR_LEN]>,
    ) -> usize {
        let setup_packet_ptr = unsafe { VolatilePtr::new(setup_packet_mem.ptr) };
        setup_packet_ptr.write(transmute!(SetupPacket::new_get_configuration_descriptor(
            MAX_CONFIGURATION_DESCRIPTOR_LEN.try_into().unwrap(),
            descriptor_num
        )));
        self.get_descriptor(
            qh_mem,
            qtds_mem,
            setup_packet_mem,
            MappedMem {
                phys_addr: buffer_mem.phys_addr,
                ptr: NonNull::slice_from_raw_parts(
                    buffer_mem.ptr.cast(),
                    MAX_CONFIGURATION_DESCRIPTOR_LEN,
                ),
            },
        )
        .await
    }

    pub async fn get_device_info(&mut self, buffer: MappedMem<GetInfoBuffer>) -> DeviceInfo {
        let buffer_ptr = unsafe { VolatilePtr::new(buffer.ptr) };

        let device_descriptor = self
            .get_device_descriptor(
                MappedMem {
                    phys_addr: buffer.phys_addr
                        + u32::try_from(offset_of!(GetInfoBuffer, qh)).unwrap(),
                    ptr: buffer_ptr.qh().as_raw_ptr(),
                },
                MappedMem {
                    phys_addr: buffer.phys_addr
                        + u32::try_from(offset_of!(GetInfoBuffer, qtds)).unwrap(),
                    ptr: buffer_ptr.qtds().as_raw_ptr().cast(),
                },
                MappedMem {
                    phys_addr: buffer.phys_addr
                        + u32::try_from(offset_of!(GetInfoBuffer, setup_packet)).unwrap(),
                    ptr: buffer_ptr.setup_packet().as_raw_ptr(),
                },
                MappedMem {
                    phys_addr: buffer.phys_addr
                        + u32::try_from(offset_of!(GetInfoBuffer, buffer)).unwrap(),
                    ptr: buffer_ptr.buffer().as_raw_ptr().cast(),
                },
            )
            .await;
        log::info!("device descriptor: {device_descriptor:#?}");

        let mut configurations = Vec::new();
        for n in 0..device_descriptor.b_num_configurations {
            let descriptor_len = self
                .get_configuration_descriptor(
                    n,
                    MappedMem {
                        phys_addr: buffer.phys_addr
                            + u32::try_from(offset_of!(GetInfoBuffer, qh)).unwrap(),
                        ptr: buffer_ptr.qh().as_raw_ptr(),
                    },
                    MappedMem {
                        phys_addr: buffer.phys_addr
                            + u32::try_from(offset_of!(GetInfoBuffer, qtds)).unwrap(),
                        ptr: buffer_ptr.qtds().as_raw_ptr().cast(),
                    },
                    MappedMem {
                        phys_addr: buffer.phys_addr
                            + u32::try_from(offset_of!(GetInfoBuffer, setup_packet)).unwrap(),
                        ptr: buffer_ptr.setup_packet().as_raw_ptr(),
                    },
                    MappedMem {
                        phys_addr: buffer.phys_addr
                            + u32::try_from(offset_of!(GetInfoBuffer, buffer)).unwrap(),
                        ptr: buffer_ptr.buffer().as_raw_ptr().cast(),
                    },
                )
                .await;
            let full_descriptor =
                &unsafe { buffer_ptr.buffer().as_slice().as_raw_ptr().as_ref() }[..descriptor_len];
            log::info!("[{n}]: {full_descriptor:02X?}");
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
                                &full_descriptor
                                    [position..position + size_of::<EndpointDescriptor>()],
                            )
                            .unwrap();
                            position += usize::try_from(descriptor.b_length).unwrap();
                            endpoints.push(Endpoint {
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
                    interfaces.push(Interface {
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
            configurations.push(Configuration {
                value: configuration_descriptor.b_configuration_value,
                attributes: configuration_descriptor.bm_attributes,
                interfaces,
                max_power: configuration_descriptor.b_max_power,
                name: None,
            });
        }
        DeviceInfo {
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

    pub async fn do_setup_transfer(
        &mut self,
        qh_mem: MappedMem<QhWithMetadata>,
        qtds_mem: MappedMem<[Qtd; 2]>,
        setup_packet_mem: MappedMem<[u8; 8]>,
    ) {
        let qh_ptr = unsafe { VolatilePtr::new(qh_mem.ptr) };
        let qtds_ptr = unsafe { VolatilePtr::new(qtds_mem.ptr) };
        let mut qh = QueueHead::new(
            EndpointCharacteristics::builder()
                .with_device_addr(self.addr)
                .with_inactive_on_next_transaction(false)
                .with_endpoint_number(u4::ZERO)
                .with_endpoint_speed(EndpointSpeed::High.into())
                .with_data_toggle_control(false)
                .with_head_of_reclamation_list_flag(false)
                .with_max_packet_len(u11::new(8))
                .with_endpoint_control_flag(false)
                .with_nak_count_reload(u4::ZERO)
                .build(),
            EndpointCapabilities::ZERO,
        );
        let mut qtds = [
            Qtd::new(
                TransferToken::new_active(PidCode::SetupToken, u15::new(8)),
                BufferPtrs::new_contiguous(setup_packet_mem.phys_addr.into(), 8),
            ),
            Qtd::new(
                TransferToken::new_active(PidCode::InToken, u15::ZERO)
                    .with_data_toggle(true)
                    .with_interrupt_on_complete(true),
                BufferPtrs::ZERO,
            ),
        ];
        qh.link_contiguous_qtds(&mut qtds, qtds_mem.phys_addr);
        qh_ptr.qh().write(qh);
        qtds_ptr.write(qtds);
        self.qh_manager.add_qh_to_async_list(qh_mem);
        self.wait_for_qtd(qtds_ptr.as_slice().index(1)).await;
        self.qh_manager
            .remove_qh(qh_mem, self.root_port_number.into())
            .await;
    }

    pub async fn set_configuration(
        &mut self,
        configuration_number: u8,
        qh_mem: MappedMem<QhWithMetadata>,
        qtds_mem: MappedMem<[Qtd; 2]>,
        setup_packet_mem: MappedMem<[u8; 8]>,
    ) {
        let setup_packet_ptr = unsafe { VolatilePtr::new(setup_packet_mem.ptr) };
        setup_packet_ptr.write(transmute!(SetupPacket::new_set_configuration(
            configuration_number
        )));
        self.do_setup_transfer(qh_mem, qtds_mem, setup_packet_mem)
            .await;
    }

    pub async fn dev(
        &mut self,
        qh_mem: MappedMem<QueueHead>,
        qtds_mem: MappedMem<[Qtd; 1]>,
        buffer_mem: MappedMem<[u8; 8 * 1]>,
        delay: &mut impl DelayNs,
    ) {
        let qh_ptr = unsafe { VolatilePtr::new(qh_mem.ptr) };
        let qtds_ptr = unsafe { VolatilePtr::new(qtds_mem.ptr) };
        let mut qh = QueueHead::new(
            EndpointCharacteristics::builder()
                .with_device_addr(self.addr)
                .with_inactive_on_next_transaction(false)
                .with_endpoint_number(u4::new(1))
                .with_endpoint_speed(EndpointSpeed::High.into())
                .with_data_toggle_control(false)
                .with_head_of_reclamation_list_flag(false)
                .with_max_packet_len(u11::new(8))
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
        let mut qtds = array::from_fn(|i| {
            Qtd::new(
                TransferToken::new_active(PidCode::InToken, u15::new(8))
                    .with_interrupt_on_complete(true),
                BufferPtrs::new_contiguous(
                    u64::from(buffer_mem.phys_addr) + 8 * u64::try_from(i).unwrap(),
                    8,
                ),
            )
        });
        // qtds[2].qtd_token.set_active(false);
        qh.link_contiguous_qtds(&mut qtds, qtds_mem.phys_addr);
        qtds.last_mut().unwrap().next_qtd_ptr = NextQtdPointer::new_valid(qtds_mem.phys_addr);
        qh_ptr.write(qh);
        qtds_ptr.write(qtds);

        // Assume period of 8ms, so every 8 frames
        for i in 0..1024 / 8 {
            self.periodic_list.elements().as_slice().index(i * 8).write(
                PeriodicFrameListElement::new(SelectType::Qh, qh_mem.phys_addr),
            );
        }

        let mut next_qtd = 0;
        loop {
            if !qtds_ptr
                .as_slice()
                .index(next_qtd)
                .qtd_token()
                .read()
                .active()
            {
                let ptr = NonNull::slice_from_raw_parts(
                    NonNull::new((buffer_mem.ptr.addr().get() + next_qtd * 8) as *mut u8).unwrap(),
                    8,
                );
                let buffer = unsafe { ptr.as_ref() };
                let overlay = qh_ptr.transfer_token().read();
                log::info!("Periodic QTD {next_qtd} done! {buffer:02X?}");
                let qtd_ptr = qtds_ptr.as_slice().index(next_qtd);
                // Restore current offset
                qtd_ptr.buffer_pointer_page_0().update(|r| {
                    r.with_current_offset(
                        BufferPtrs::new_contiguous(
                            u64::from(buffer_mem.phys_addr) + 8 * u64::try_from(next_qtd).unwrap(),
                            8,
                        )
                        .offset(),
                    )
                });
                // Restore token
                qtd_ptr.qtd_token().write(
                    TransferToken::new_active(PidCode::InToken, u15::new(8))
                        .with_interrupt_on_complete(true),
                );
                next_qtd += 1;
                if next_qtd == 1 {
                    next_qtd = 0;
                }
            } else {
                future::poll_fn(|context| {
                    self.port_wakers[u4::from(self.root_port_number).as_usize()]
                        .register(context.waker());
                    if !qtds_ptr
                        .as_slice()
                        .index(next_qtd)
                        .qtd_token()
                        .read()
                        .active()
                    {
                        return Poll::Ready(());
                    }
                    Poll::Pending
                })
                .await;
            }
        }
    }

    pub async fn hub_info(
        &mut self,
        qh_mem: MappedMem<QhWithMetadata>,
        qtds_mem: MappedMem<[Qtd; 3]>,
        setup_packet_mem: MappedMem<[u8; 8]>,
        buffer_mem: MappedMem<[u8; size_of::<HubDescriptor>()]>,
        delay: &mut impl DelayNs,
    ) {
        log::info!("Getting hub info");
        let setup_packet_ptr = unsafe { VolatilePtr::new(setup_packet_mem.ptr) };
        setup_packet_ptr.write(transmute!(SetupPacket::new_get_hub_descriptor(
            size_of::<HubDescriptor>().try_into().unwrap()
        )));
        let descriptor_len = self
            .get_descriptor(
                qh_mem,
                qtds_mem,
                setup_packet_mem,
                MappedMem {
                    phys_addr: buffer_mem.phys_addr,
                    ptr: NonNull::slice_from_raw_parts(
                        buffer_mem.ptr.cast(),
                        size_of::<HubDescriptor>(),
                    ),
                },
            )
            .await;
        let descriptor_ptr =
            NonNull::slice_from_raw_parts(buffer_mem.ptr.cast::<u8>(), descriptor_len);
        let descriptor = unsafe { descriptor_ptr.as_ref() };
        log::info!("Descriptor bytes: {descriptor:02X?}");
        let hub_descriptor = HubDescriptor::from_bytes(descriptor);
        log::info!("{hub_descriptor:#?}");

        self.set_configuration(
            1,
            qh_mem,
            MappedMem {
                phys_addr: qtds_mem.phys_addr,
                ptr: qtds_mem.ptr.cast(),
            },
            setup_packet_mem,
        )
        .await;
        log::info!("set HUB configuration to 1");
        // delay.delay_ms(50).await;

        for port_number in 1..=hub_descriptor.fixed_size_fields.b_nbr_ports {
            // Enable power for all ports
            setup_packet_ptr.write(transmute!(SetupPacket::new_set_feature_port_power(
                port_number
            )));
            self.do_setup_transfer(
                qh_mem,
                MappedMem {
                    phys_addr: qtds_mem.phys_addr,
                    ptr: qtds_mem.ptr.cast(),
                },
                setup_packet_mem,
            )
            .await;
            log::info!("enabled port power for port {port_number}");

            // log::info!("setting HUB configuration to 1 as a test - {port_number}");
            // self.set_configuration(
            //     1,
            //     qh_mem,
            //     MappedMem {
            //         phys_addr: qtds_mem.phys_addr,
            //         ptr: qtds_mem.ptr.cast(),
            //     },
            //     setup_packet_mem,
            // )
            // .await;
            // log::info!("set HUB configuration to 1 as a test - {port_number}");
            // delay.delay_ms(100).await;
        }
    }
}
