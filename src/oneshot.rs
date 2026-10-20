use core::{
    cell::SyncUnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};

use alloc::sync::Arc;
use futures::task::AtomicWaker;

struct OneshotShared<T> {
    stored: AtomicBool,
    cell: SyncUnsafeCell<MaybeUninit<T>>,
    waker: AtomicWaker,
}

pub struct OneshotSender<T> {
    shared: Arc<OneshotShared<T>>,
}

impl<T> OneshotSender<T> {
    pub fn send(self, value: T) {
        unsafe { self.shared.cell.get().as_mut_unchecked() }.write(value);
        self.shared.stored.store(true, Ordering::Release);
        self.shared.waker.wake();
    }
}

pub struct OneshotReceiver<T> {
    shared: Arc<OneshotShared<T>>,
}

impl<T> Future for OneshotReceiver<T> {
    type Output = Result<T, RecvError>;

    fn poll(self: core::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.shared.waker.register(cx.waker());
        if self.shared.stored.load(Ordering::Acquire) {
            let value = unsafe { self.shared.cell.get().as_mut_unchecked().assume_init_read() };
            Poll::Ready(Ok(value))
        } else if Arc::strong_count(&self.shared) == 1 {
            Poll::Ready(Err(RecvError))
        } else {
            Poll::Pending
        }
    }
}

pub fn oneshot<T>() -> (OneshotSender<T>, OneshotReceiver<T>) {
    let shared = Arc::new(OneshotShared {
        stored: AtomicBool::new(false),
        cell: SyncUnsafeCell::new(MaybeUninit::uninit()),
        waker: AtomicWaker::new(),
    });
    (
        OneshotSender {
            shared: shared.clone(),
        },
        OneshotReceiver { shared },
    )
}

/// This happens when the sender gets dropped without sending anything.
#[derive(Debug, Clone, Copy)]
pub struct RecvError;
