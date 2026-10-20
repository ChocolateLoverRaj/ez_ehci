use core::{
    mem,
    task::{Context, Poll, Waker},
};

use spinning_top::Spinlock;

#[derive(Debug)]
pub struct Signal<T> {
    mutex: Spinlock<State<T>>,
}

impl<T> Default for Signal<T> {
    fn default() -> Self {
        Self {
            mutex: Spinlock::new(State::None),
        }
    }
}

#[derive(Debug)]
enum State<T> {
    None,
    Waiting(Waker),
    Signaled(T),
}

impl<T> Signal<T> {
    pub fn signal(&self, value: T) {
        if let State::Waiting(waker) = mem::replace(&mut *self.mutex.lock(), State::Signaled(value))
        {
            waker.wake();
        }
    }

    pub fn poll_wait(&self, cx: &mut Context<'_>) -> Poll<T> {
        let mut state = self.mutex.lock();
        match mem::replace(&mut *state, State::None) {
            State::None => {
                *state = State::Waiting(cx.waker().clone());
                Poll::Pending
            }
            State::Waiting(w) if w.will_wake(cx.waker()) => {
                *state = State::Waiting(w);
                Poll::Pending
            }
            State::Waiting(w) => {
                *state = State::Waiting(cx.waker().clone());
                w.wake();
                Poll::Pending
            }
            State::Signaled(value) => Poll::Ready(value),
        }
    }

    pub fn wait(&self) -> WaitFuture<'_, T> {
        WaitFuture { signal: self }
    }
}

pub struct WaitFuture<'a, T> {
    signal: &'a Signal<T>,
}

impl<T> Future for WaitFuture<'_, T> {
    type Output = T;

    fn poll(self: core::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.signal.poll_wait(cx)
    }
}
