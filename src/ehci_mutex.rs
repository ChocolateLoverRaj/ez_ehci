use core::{
    ops::{Deref, DerefMut},
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll, Waker},
};

use alloc::{boxed::Box, vec::Vec};
use arbitrary_int::u4;
use debug_ignore::DebugIgnore;
use futures::{FutureExt, task::AtomicWaker};
use spinning_top::{Spinlock, guard::SpinlockGuard};
use volatile::{VolatileRef, access::ReadOnly};

use crate::{
    address_allocator::AddressAllocator,
    capability_regs::{CapabilityRegs, CapabilityRegsVolatileFieldAccess},
    delay::Delay,
    operational_regs::{
        LineStatus, OperationalRegs, OperationalRegsVolatileFieldAccess, PortScReg, UsbStsReg,
    },
    root_port_number::RootPortNumber,
};

#[derive(Debug)]
pub enum RootPortData {
    /// At least, to the software, no device is connected.
    NoDeviceConnected,
    /// To us, there is basically a dummy device plugged in. We don't care unless it gets unplugged and a high speed device is plugged in.
    IsLowSpeed,
    IsFullSpeed,
    ResettingPort(DebugIgnore<Pin<Box<dyn Future<Output = ()>>>>),
    WaitingForPortToBeEnabled(DebugIgnore<Pin<Box<dyn Future<Output = ()>>>>),
    QueuedToSetAddr,
    /// Either setting addr or queued to set addr
    SettingAddr,
}

#[derive(Debug)]
pub struct EhciData {
    port_sc_regs: VolatileRef<'static, [PortScReg]>,
    capability_regs: VolatileRef<'static, CapabilityRegs, ReadOnly>,
    operational_regs: VolatileRef<'static, OperationalRegs>,
    addresses: AddressAllocator,
    root_ports: Box<[RootPortData]>,
    delay: DebugIgnore<Box<dyn Delay>>,
    runner_waker: AtomicWaker,
}

impl EhciData {
    fn process_data(&mut self, context: &mut Option<&mut Context<'_>>) {
        let is_runner = context.is_some();
        let wake_runner = || {
            if !is_runner {
                self.runner_waker.wake();
            }
        };
        let noop = Context::from_waker(Waker::noop());
        let context = context.as_mut().map_or(&mut noop, |c| *c);
        let usb_sts = self.operational_regs.as_mut_ptr().usb_sts().read();
        let mut usb_sts_write_value = UsbStsReg::new_with_raw_value(0);
        if usb_sts.host_system_error() {
            todo!("recover from host system errors by resetting and re-initializing the eHCI")
        }
        if usb_sts.port_change_detect() {
            usb_sts_write_value.set_port_change_detect(true);
        }
        for (i, port) in self.root_ports.iter_mut().enumerate() {
            let port_sc_ptr = self.port_sc_regs.as_mut_ptr().index(i);
            let port_sc = port_sc_ptr.read();
            // clear all pending changed bits
            let mut port_sc_write_value = port_sc;
            match port {
                RootPortData::NoDeviceConnected => {
                    if port_sc.current_connect_status() {
                        if LineStatus::from(port_sc.line_status()) == LineStatus::KState {
                            *port = RootPortData::IsLowSpeed;
                        } else {
                            *port = RootPortData::ResettingPort(DebugIgnore(
                                self.delay.delay_ms(50).into(),
                            ));
                        }
                    }
                }
                RootPortData::IsFullSpeed | RootPortData::IsFullSpeed => {
                    if !port_sc.current_connect_status() {
                        *port = RootPortData::NoDeviceConnected;
                    }
                }
                RootPortData::ResettingPort(future) => {
                    wake_runner();
                    if let Poll::Ready(_) = future.poll_unpin(context) {
                        // stop resetting the port
                        port_sc_write_value.set_port_reset(false);
                    }
                }
                RootPortData::WaitingForPortToBeEnabled(future) => {
                    if port_sc.port_enabled() {
                        self.set
                    } else {
                    }
                }
            }
            port_sc_ptr.write(port_sc_write_value);
        }
        // for port in 0..self
        //     .capability_regs
        //     .as_ptr()
        //     .hcs_params()
        //     .read()
        //     .n_ports()
        //     .value()
        // {
        //     match
        // }
    }
}

/// Basically a mutex around USBCMD and key configuration and data structures.
#[derive(Debug)]
pub struct EhciMutex {
    should_process_data: AtomicBool,
    mutex: Spinlock<EhciData>,
}

#[derive(Debug)]
pub struct BorrowedEhciMutex<'a> {
    should_process_data: &'a AtomicBool,
    mutex: &'a Spinlock<EhciData>,
}

impl BorrowedEhciMutex<'_> {
    fn process_if_needed(&self, context: &mut Option<&mut Context<'_>>) {
        while self.should_process_data.load(Ordering::Relaxed) {
            let Some(mut data) = self.mutex.try_lock() else {
                break;
            };
            if self.should_process_data.swap(false, Ordering::Relaxed) {
                data.process_data(context);
            }
        }
    }
}

impl EhciMutex {
    /// Either processes the data or makes sure some other code will process the data.
    pub fn process_or_queue(&self, context: Option<&mut Context<'_>>) {
        self.should_process_data.store(true, Ordering::Relaxed);
        BorrowedEhciMutex {
            should_process_data: &self.should_process_data,
            mutex: &self.mutex,
        }
        .process_if_needed(context);
    }

    /// Use this to add/remove wakers from the list(s).
    /// IRQ handlers are not allowed to call this.
    pub fn lock(&self) -> EhciDataGuard<'_> {
        EhciDataGuard {
            guard: Some(self.mutex.lock()),
            should_process_data: &self.should_process_data,
        }
    }
}

pub struct EhciDataGuard<'a> {
    guard: Option<SpinlockGuard<'a, EhciData>>,
    should_process_data: &'a AtomicBool,
}

impl Deref for EhciDataGuard<'_> {
    type Target = EhciData;

    fn deref(&self) -> &Self::Target {
        self.guard.as_ref().unwrap()
    }
}

impl DerefMut for EhciDataGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard.as_mut().unwrap()
    }
}

impl Drop for EhciDataGuard<'_> {
    fn drop(&mut self) {
        let borrowed_ehci_mutex = BorrowedEhciMutex {
            mutex: SpinlockGuard::mutex(&self.guard.take().unwrap()),
            should_process_data: &self.should_process_data,
        };
        borrowed_ehci_mutex.process_if_needed();
    }
}
