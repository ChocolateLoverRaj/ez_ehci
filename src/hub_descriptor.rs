use arbitrary_int::u2;
use bitbybit::bitfield;
use zerocopy::{FromBytes, Immutable, KnownLayout, little_endian::U16};

#[repr(C)]
#[derive(Debug, Clone, Copy, FromBytes, Immutable, KnownLayout)]
pub struct HubDescriptorFixedSizeFields {
    pub b_desc_len: u8,
    pub b_descriptor_type: u8,
    pub b_nbr_ports: u8,
    pub w_hub_characteristics: U16,
    /// Time (in 2 ms units) from the time the power-on sequence begins on a port until power is good on that port.
    pub b_pwr_on_2_pwr_good: u8,
    /// Maximum current requirements of the hub controller electronics in mA.
    pub b_hub_contr_current: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct HubDescriptor {
    pub fixed_size_fields: HubDescriptorFixedSizeFields,
    /// Bit 0: reserved. Bit 1: Port 1. Bit n: Port n. Up to 255 ports.
    /// If the bit is set that means that the device is removable.
    pub device_removable: [u8; 32],
}

impl HubDescriptor {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let fixed_size_fields = HubDescriptorFixedSizeFields::read_from_bytes(
            &bytes[..size_of::<HubDescriptorFixedSizeFields>()],
        )
        .unwrap();
        let mut device_removable = [Default::default(); _];
        let bytes_to_copy =
            (usize::try_from(fixed_size_fields.b_nbr_ports).unwrap() + 1).div_ceil(8);
        device_removable[..bytes_to_copy].copy_from_slice(
            &bytes[size_of::<HubDescriptorFixedSizeFields>()
                ..size_of::<HubDescriptorFixedSizeFields>() + bytes_to_copy],
        );
        Self {
            fixed_size_fields,
            device_removable,
        }
    }
}

#[bitfield(u16, debug)]
pub struct HubCharacteristics {
    #[bits(0..=1, rw)]
    pub logical_power_switching_mode: u2,
    #[bit(2, rw)]
    pub is_part_of_a_compound_device: bool,
    #[bits(3..=4, rw)]
    pub over_current_protection_mode: u2,
    #[bits(5..=6, rw)]
    pub tt_think_time: u2,
    #[bit(7, rw)]
    pub port_indicators_supported: bool,
}

pub enum LogicalPowerSwitchingMode {
    GangedPowerSwitching,
    IndividualPortPowerSwitching,
    NoPowerSwitching,
}

impl LogicalPowerSwitchingMode {
    pub fn from_hub_characteristic_bits(bits: u2) -> Self {
        match bits.value() {
            0 => Self::GangedPowerSwitching,
            1 => Self::IndividualPortPowerSwitching,
            2..=3 => Self::NoPowerSwitching,
            _ => unreachable!(),
        }
    }
}
