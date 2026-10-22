use core::{ptr::NonNull, time::Duration};

use volatile::VolatilePtr;
use zerocopy::transmute;

use crate::{
    Delay, InterruptEndpoint, MappedMem,
    control_endpoint::LoadedControlEndpoint,
    hub_descriptor::HubDescriptor,
    port_status_data::{PortChange, PortStatus, PortStatusData},
    qtd::Qtd,
    setup_packet::SetupPacket,
};

pub struct Hub {
    pub(crate) endpoint_0: LoadedControlEndpoint,
    pub(crate) interrupt_endpoint: InterruptEndpoint,
}

impl Hub {
    async fn get_port_status(
        &mut self,
        port_number: u8,
        qtds_mem: [MappedMem<Qtd>; 3],
        setup_packet_mem: MappedMem<[u8; 8]>,
        buffer_mem: MappedMem<[u8; 4]>,
    ) -> PortStatusData {
        let setup_packet_ptr = unsafe { VolatilePtr::new(setup_packet_mem.ptr) };
        setup_packet_ptr.write(transmute!(SetupPacket::new_get_port_status(port_number)));
        let buffers = [buffer_mem.cast_slice()];
        let len = unsafe {
            self.endpoint_0
                .transfer_in(setup_packet_mem, &buffers, &qtds_mem)
        }
        .await;
        assert_eq!(len, 4);
        let port_status = unsafe { buffer_mem.ptr.cast::<[u8; 4]>().read_volatile() };
        log::info!("port {port_number} status: {port_status:02X?}");
        transmute!(port_status)
    }

    pub async fn get_info(
        mut self,
        qtds_mem: [MappedMem<Qtd>; 3],
        setup_packet_mem: MappedMem<[u8; 8]>,
        buffer_mem: MappedMem<[u8; size_of::<HubDescriptor>()]>,
        delay: &mut dyn Delay,
    ) {
        let setup_packet_ptr = unsafe { VolatilePtr::new(setup_packet_mem.ptr) };
        setup_packet_ptr.write(transmute!(SetupPacket::new_get_hub_descriptor(
            size_of::<HubDescriptor>().try_into().unwrap()
        )));
        let buffers = [buffer_mem.cast_slice()];
        let descriptor_len = unsafe {
            self.endpoint_0
                .transfer_in(setup_packet_mem, &buffers, &qtds_mem)
        }
        .await;
        let descriptor_ptr = NonNull::slice_from_raw_parts(
            buffer_mem.ptr.cast::<u8>(),
            descriptor_len.try_into().unwrap(),
        );
        let descriptor = unsafe { descriptor_ptr.as_ref() };
        log::info!("Hub descriptor bytes: {descriptor:02X?}");
        let hub_descriptor = HubDescriptor::from_bytes(descriptor);
        log::debug!("{hub_descriptor:#?}");

        for i in 0..hub_descriptor.fixed_size_fields.b_nbr_ports {
            let port_number = 1 + i;

            setup_packet_ptr.write(transmute!(SetupPacket::new_set_feature_port_power(
                port_number
            )));
            unsafe {
                self.endpoint_0
                    .transfer_no_data(setup_packet_mem, qtds_mem[0..2].try_into().unwrap())
            }
            .await;

            delay
                .delay_until(
                    delay.now()
                        + Duration::from_millis(2)
                            * hub_descriptor.fixed_size_fields.b_pwr_on_2_pwr_good.into(),
                )
                .await;

            let port_status_data = self
                .get_port_status(
                    port_number,
                    qtds_mem,
                    setup_packet_mem,
                    MappedMem {
                        phys_addr: buffer_mem.phys_addr,
                        ptr: buffer_mem.ptr.cast(),
                    },
                )
                .await;
            let port_status = PortStatus::new_with_raw_value(port_status_data.w_port_status.get());

            if port_status.current_connect_status() {
                setup_packet_ptr.write(transmute!(SetupPacket::new_set_feature_port_reset(
                    port_number
                )));
                unsafe {
                    self.endpoint_0
                        .transfer_no_data(setup_packet_mem, qtds_mem[0..2].try_into().unwrap())
                }
                .await;

                // 10ms is minimum wait time
                // 50ms is just a guess
                // ideally we should use the interrupt endpoint and have a 100-500ms timeout with possible retries
                delay
                    .delay_until(delay.now() + Duration::from_millis(50))
                    .await;
                let port_status_data = self
                    .get_port_status(
                        port_number,
                        qtds_mem,
                        setup_packet_mem,
                        MappedMem {
                            phys_addr: buffer_mem.phys_addr,
                            ptr: buffer_mem.ptr.cast(),
                        },
                    )
                    .await;
                let port_status =
                    PortStatus::new_with_raw_value(port_status_data.w_port_status.get());

                if let Some(speed) = port_status.attached_device_speed() {
                    log::info!("port {port_number} attached with speed {speed:?}");
                    todo!("assign addr");
                } else {
                    log::warn!("attempted to reset {port_number} but no device attached");
                    // Note that maybe if we wait longer or retry we could use this device
                }
            }
        }
    }
}
