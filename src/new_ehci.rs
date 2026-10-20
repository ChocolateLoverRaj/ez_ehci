use core::ptr::NonNull;

use alloc::boxed::Box;
use volatile::VolatilePtr;

use crate::{
    BiosOwnedEhci, ExtendedCapabilitiesIterator, ExtendedCapability, PciAccess, Stack,
    capability_regs::CapabilityRegs, delay::Delay,
};

/// # Safety
/// - `mapped_bar` must be the virtual address pointing to the bar.
/// - The entire BAR 0 must be mapped with the right caching type.
pub unsafe fn new_ehci<P: PciAccess>(
    mapped_bar: NonNull<[u8]>,
    mut pci_access: P,
    delay: Box<dyn Delay>,
) -> AnyEhci<P> {
    let capability_regs = unsafe {
        VolatilePtr::<CapabilityRegs>::new_read_only(mapped_bar.cast::<CapabilityRegs>())
    };
    for extended_capability in ExtendedCapabilitiesIterator::new(&mut pci_access, capability_regs) {
        log::info!("extended capability: {extended_capability:#X?}");
    }
    if let Some(usb_leg_sup_offset) =
        ExtendedCapabilitiesIterator::new(&mut pci_access, capability_regs).find_map(|c| {
            if c.id == ExtendedCapability::UsbLegSup as u8 {
                Some(c.offset)
            } else {
                None
            }
        })
    {
        AnyEhci::BiosOwned(BiosOwnedEhci {
            mapped_bar,
            pci_access,
            usb_leg_sup_offset,
            delay,
        })
    } else {
        AnyEhci::OsOwned(unsafe { Stack::new(mapped_bar, delay) })
    }
}

pub enum AnyEhci<P: PciAccess> {
    BiosOwned(BiosOwnedEhci<P>),
    OsOwned(Stack),
}
