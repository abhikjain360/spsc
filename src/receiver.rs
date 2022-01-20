use std::{
    iter,
    pin::Pin,
    ptr::{self, NonNull},
    task::{Context, Poll},
};

use futures::Future;

use crate::{sync::*, Queue};

pub struct Receiver<T> {
    buffer: NonNull<Queue<T>>,
    local_tail: usize,
}

impl<T> Receiver<T> {
    pub(crate) fn new(buffer: NonNull<Queue<T>>) -> Self {
        Self {
            buffer,
            local_tail: 0,
        }
    }

    pub fn try_recv(&mut self) -> Option<T> {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_mut() };

        let cur_head = buffer.head.load(Ordering::Relaxed);

        if cur_head == self.local_tail {
            self.local_tail = buffer.tail.load(Ordering::Relaxed);

            // cretry with updated head value
            // hecking twice is still cheaper than reading atomically twice
            if cur_head == self.local_tail {
                return None;
            }
        }

        let val = unsafe { ptr::read(buffer.buffer[cur_head].get() as *const _) };

        let mut new_head = cur_head + 1;
        if new_head == buffer.capacity {
            new_head = 0;
        }

        // this is fine as we are the only ones writing to head
        buffer.head.store(new_head, Ordering::Relaxed);

        Some(val)
    }

    pub fn recv(&mut self) -> T {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_mut() };

        let cur_head = buffer.head.load(Ordering::Relaxed);

        if cur_head == self.local_tail {
            self.local_tail = buffer.tail.load(Ordering::Acquire);

            while cur_head == self.local_tail {
                hint::spin_loop();
                self.local_tail = buffer.tail.load(Ordering::Acquire);
            }
        }

        let val = unsafe { ptr::read(buffer.buffer[cur_head].get() as *const _) };

        let mut new_head = cur_head + 1;
        if new_head == buffer.capacity {
            new_head = 0;
        }

        // this is fine as we are the only ones writing to head
        buffer.head.store(new_head, Ordering::Release);

        val
    }

    #[inline]
    pub fn recv_async(&mut self) -> RecvFut<'_, T> {
        RecvFut { receiver: self }
    }

    pub fn batch_recv(&mut self, buf: &mut impl iter::Extend<T>, limit: usize) -> usize {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_mut() };
        let mut count = 0;

        let mut cur_head = buffer.head.load(Ordering::Relaxed);

        while count < limit {
            if cur_head == self.local_tail {
                self.local_tail = buffer.tail.load(Ordering::Relaxed);

                // cretry with updated head value
                // hecking twice is still cheaper than reading atomically twice
                if cur_head == self.local_tail {
                    break;
                }
            }

            // TODO: change this to extend_one when it stabilizes
            buf.extend(iter::once(unsafe {
                ptr::read(buffer.buffer[cur_head].get() as *const _)
            }));
            count += 1;

            cur_head += 1;
            if cur_head == buffer.capacity {
                cur_head = 0;
            }
        }

        buffer.head.store(cur_head, Ordering::Release);
        count
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        // SAFETY: we are the only ones accessing it apart from other end which should not drop the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };
        let rc = buffer.rc.fetch_sub(1, Ordering::SeqCst);

        if rc == 1 {
            // SAFETY: only dropped when buffer.rc == 0, so this is fine as other end shouldn't
            // attempt to drop it.
            unsafe {
                ptr::drop_in_place(self.buffer.as_ptr());
            }
        }
    }
}

pub struct RecvFut<'receiver, T> {
    receiver: &'receiver mut Receiver<T>,
}

impl<'receiver, T> RecvFut<'receiver, T> {
    #[inline]
    fn project(self: Pin<&mut Self>) -> &mut Receiver<T> {
        // SAFETY: we should NEVER move out any values
        let me = unsafe { self.get_unchecked_mut() };
        me.receiver
    }
}

impl<'receiver, T> Future for RecvFut<'receiver, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let receiver = self.project();
        match receiver.try_recv() {
            None => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Some(val) => Poll::Ready(val),
        }
    }
}

// SAFETY: internal queue has atomic pointers to head and tails, and thus is safe to send
unsafe impl<T: Send> Send for Receiver<T> {}
