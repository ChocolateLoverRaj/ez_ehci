#![no_std]
extern crate alloc;

mod buffer_ptrs;
mod capability_regs;
mod endpoint_speed;
mod initialized_ehci;
mod new_ehci;
mod operational_regs;
mod os_owned_ehci;
mod pci;
mod periodic_list;
mod qtd;
mod queue_head;
mod root_port_number;
mod setup_packet;
mod transfer_token;
mod usb_leg_sup;

use core::{fmt::Debug, mem::offset_of, num::NonZero, ptr::NonNull};

use bitbybit::bitfield;
use volatile::{VolatileFieldAccess, VolatilePtr, access::ReadOnly};

use crate::capability_regs::{CapabilityRegs, CapabilityRegsVolatileFieldAccess};
pub use crate::initialized_ehci::{InitDeviceBuffer, InitializedEhci, NewDeviceEvent};
pub use crate::new_ehci::{AnyEhci, new_ehci};
pub use crate::os_owned_ehci::OsOwnedEhci;
pub use crate::pci::{PCI_CLASS, PCI_PROG_IF, PCI_SUBCLASS, PciAccess};
pub use crate::periodic_list::PeriodicFrameList;
pub use crate::queue_head::QueueHead;
pub use crate::usb_leg_sup::{BiosOwnedEhci, TakingOwnershipEhci, TryTakeOutput};

#[derive(Debug, Clone, Copy)]
pub struct MappedMem<T> {
    pub phys_addr: u32,
    pub ptr: NonNull<T>,
}

#[bitfield(u32, debug)]
struct ExtendedCapabilityPointerReg {
    #[bits(0..=7, r)]
    capability_id: u8,
    #[bits(8..=15, r)]
    next_cap_pointer: u8,
    #[bits(16..=31, rw)]
    capability_specific: u16,
}

struct ExtendedCapabilitiesIterator<P: PciAccess> {
    pci_access: P,
    capability_pointer: Option<NonZero<u8>>,
}

impl<P: PciAccess> ExtendedCapabilitiesIterator<P> {
    fn new(pci_access: P, capability_regs: VolatilePtr<'static, CapabilityRegs, ReadOnly>) -> Self {
        Self {
            pci_access,
            capability_pointer: NonZero::new(capability_regs.hcc_params().read().eecp()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CapabilityInfo {
    /// Represents an offset in bytes in the PCI configuration space.
    offset: NonZero<u8>,
    id: u8,
}

#[repr(u8)]
enum ExtendedCapability {
    UsbLegSup = 0x1,
}

impl<P: PciAccess> Iterator for ExtendedCapabilitiesIterator<P> {
    type Item = CapabilityInfo;

    fn next(&mut self) -> Option<Self::Item> {
        let capability_pointer = self.capability_pointer?;
        let capability_reg = ExtendedCapabilityPointerReg::new_with_raw_value(
            self.pci_access.read_u32(capability_pointer.get()),
        );
        self.capability_pointer = NonZero::new(capability_reg.next_cap_pointer());
        Some(CapabilityInfo {
            offset: capability_pointer,
            id: capability_reg.capability_id(),
        })
    }
}
