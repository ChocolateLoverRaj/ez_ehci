use core::{num::NonZero, ptr::NonNull};

use bitbybit::bitfield;

use crate::{OsOwnedEhci, PciAccess};

#[bitfield(u32, debug)]
pub struct UsbLegSupReg {
    #[bit(16, r)]
    pub bios_owned_semaphore: bool,
    #[bit(24, rw)]
    pub os_owned_semaphor: bool,
}

#[bitfield(u32, debug)]
pub struct UsbLegCtlStsReg {
    #[bit(0, rw)]
    pub usb_smi_enable: bool,
    #[bit(1, rw)]
    pub smi_on_usb_error_enable: bool,
    #[bit(2, rw)]
    pub smi_on_port_change_enable: bool,
    #[bit(3, rw)]
    pub smi_on_frame_list_rollover_enable: bool,
    #[bit(4, rw)]
    pub smi_on_host_system_error_enable: bool,
    #[bit(5, rw)]
    pub smi_on_async_event_enable: bool,
    #[bit(13, rw)]
    pub smi_on_os_ownership_enable: bool,
    #[bit(14, rw)]
    pub smi_on_pci_command_enable: bool,
    #[bit(15, rw)]
    pub smi_on_bar_enable: bool,
    #[bit(16, r)]
    pub smi_on_usb_complete: bool,
    #[bit(17, r)]
    pub smi_on_usb_error: bool,
    #[bit(18, r)]
    pub smi_on_port_change_detect: bool,
    #[bit(19, r)]
    pub smi_on_frame_list_rollover: bool,
    #[bit(20, r)]
    pub smi_on_host_system_error: bool,
    #[bit(21, r)]
    pub smi_on_async_advance: bool,
    #[bit(29, rw)]
    pub smi_on_os_ownership: bool,
    #[bit(30, rw)]
    pub smi_on_pci_command: bool,
    #[bit(31, rw)]
    pub smi_on_bar: bool,
}

pub struct BiosOwnedEhci<P: PciAccess> {
    pub(crate) mapped_bar: NonNull<[u8]>,
    pub(crate) pci_access: P,
    pub(crate) usb_leg_sup_offset: NonZero<u8>,
}

impl<P: PciAccess> BiosOwnedEhci<P> {
    pub fn take_ownership(mut self) -> TakingOwnershipEhci<P> {
        // Write 1 to the HC OS Owned Semaphore, which is byte 3
        // Other bits are reserved (write 0)
        self.pci_access
            .write_u8(self.usb_leg_sup_offset.get() + 3, 0x1);
        TakingOwnershipEhci {
            mapped_bar: self.mapped_bar,
            pci_access: self.pci_access,
            usb_leg_sup_offset: self.usb_leg_sup_offset,
        }
    }
}

pub struct TakingOwnershipEhci<P: PciAccess> {
    mapped_bar: NonNull<[u8]>,
    pci_access: P,
    usb_leg_sup_offset: NonZero<u8>,
}

pub enum TryTakeOutput<P: PciAccess> {
    NotYet(TakingOwnershipEhci<P>),
    Taken(OsOwnedEhci),
}

impl<P: PciAccess> TakingOwnershipEhci<P> {
    pub fn try_take(mut self) -> TryTakeOutput<P> {
        // Read the HC BIOS Owned Semaphore
        let reg = UsbLegSupReg::new_with_raw_value(
            self.pci_access.read_u32(self.usb_leg_sup_offset.get()),
        );
        if reg.bios_owned_semaphore() {
            TryTakeOutput::NotYet(self)
        } else {
            TryTakeOutput::Taken(unsafe { OsOwnedEhci::new(self.mapped_bar) })
        }
    }
}
