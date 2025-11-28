use std::{
    iter,
    pin::Pin,
    ptr::NonNull,
    task::{Context, Poll},
};

use futures::Future;

use crate::{Queue, QueueValue, sync::*};

pub struct Receiver {
    buffer: NonNull<Queue>,
    local_tail: usize,
}

impl Receiver {
    pub(crate) fn new(buffer: NonNull<Queue>) -> Self {
        Self {
            buffer,
            local_tail: 0,
        }
    }

    pub fn try_recv(&mut self) -> Option<QueueValue> {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };

        let cur_head = buffer.head.load(Ordering::Relaxed);

        if cur_head == self.local_tail {
            self.local_tail = buffer.tail.load(Ordering::Acquire);

            // retry with updated head value
            // hecking twice is still cheaper than reading atomically twice
            if cur_head == self.local_tail {
                return None;
            }
        }

        #[cfg(not(loomer))]
        let val = unsafe {
            ((*Queue::elem(self.buffer.as_ptr(), cur_head)).get() as *const QueueValue).read()
        };

        #[cfg(loomer)]
        let val = unsafe {
            (*Queue::elem(self.buffer.as_ptr(), cur_head))
                .with(|ptr| (ptr as *const QueueValue).read())
        };

        let mut new_head = cur_head + 1;
        if new_head == buffer.capacity {
            new_head = 0;
        }

        // this is fine as we are the only ones writing to head
        buffer.head.store(new_head, Ordering::Release);

        Some(val)
    }

    pub fn recv(&mut self) -> QueueValue {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };

        let cur_head = buffer.head.load(Ordering::Relaxed);

        if cur_head == self.local_tail {
            self.local_tail = buffer.tail.load(Ordering::Acquire);

            let mut spin_count = 0;
            while cur_head == self.local_tail {
                if spin_count > 100 {
                    thread::yield_now();
                    spin_count = 0;
                } else {
                    hint::spin_loop();
                    spin_count += 1;
                }
                self.local_tail = buffer.tail.load(Ordering::Acquire);
            }
        }

        #[cfg(not(loomer))]
        let val = unsafe {
            ((*Queue::elem(self.buffer.as_ptr(), cur_head)).get() as *const QueueValue).read()
        };

        #[cfg(loomer)]
        let val = unsafe {
            (*Queue::elem(self.buffer.as_ptr(), cur_head))
                .with(|ptr| (ptr as *const QueueValue).read())
        };

        let mut new_head = cur_head + 1;
        if new_head == buffer.capacity {
            new_head = 0;
        }

        // this is fine as we are the only ones writing to head
        buffer.head.store(new_head, Ordering::Release);

        val
    }

    #[inline]
    pub fn recv_async(&mut self) -> RecvFut<'_> {
        RecvFut { receiver: self }
    }

    #[inline]
    pub fn batch_recv_async<'a, I>(
        &'a mut self,
        buf: &'a mut I,
        limit: usize,
    ) -> BatchRecvFut<'a, I> {
        BatchRecvFut {
            receiver: self,
            buf,
            limit,
        }
    }

    pub fn batch_recv(&mut self, buf: &mut impl iter::Extend<QueueValue>, limit: usize) -> usize {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };
        let mut count = 0;

        let mut cur_head = buffer.head.load(Ordering::Relaxed);

        while count < limit {
            if cur_head == self.local_tail {
                self.local_tail = buffer.tail.load(Ordering::Acquire);

                // cretry with updated head value
                // hecking twice is still cheaper than reading atomically twice
                if cur_head == self.local_tail {
                    break;
                }
            }

            #[cfg(not(loomer))]
            let val = unsafe {
                ((*Queue::elem(self.buffer.as_ptr(), cur_head)).get() as *const QueueValue).read()
            };

            #[cfg(loomer)]
            let val = unsafe {
                (*Queue::elem(self.buffer.as_ptr(), cur_head))
                    .with(|ptr| (ptr as *const QueueValue).read())
            };

            // TODO: change this to extend_one when it stabilizes
            buf.extend(iter::once(val));
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

impl Drop for Receiver {
    fn drop(&mut self) {
        // SAFETY: we are the only ones accessing it apart from other end which should not drop the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };
        let rc = buffer.rc.fetch_sub(1, Ordering::SeqCst);

        if rc == 1 {
            // SAFETY: only dropped when buffer.rc == 0, so this is fine as other end shouldn't
            // attempt to drop it.
            unsafe {
                Queue::free(self.buffer);
            }
        }
    }
}

pub struct RecvFut<'receiver> {
    receiver: &'receiver mut Receiver,
}

impl<'receiver> RecvFut<'receiver> {
    #[inline]
    fn project(self: Pin<&mut Self>) -> &mut Receiver {
        // SAFETY: we should NEVER move out any values
        let me = unsafe { self.get_unchecked_mut() };
        me.receiver
    }
}

impl<'receiver> Future for RecvFut<'receiver> {
    type Output = QueueValue;

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

pub struct BatchRecvFut<'a, I> {
    receiver: &'a mut Receiver,
    buf: &'a mut I,
    limit: usize,
}

impl<'a, I> BatchRecvFut<'a, I> {
    #[inline]
    fn project(self: Pin<&mut Self>) -> (&mut Receiver, &mut I, usize) {
        // SAFETY: we should NEVER move out any values
        let me = unsafe { self.get_unchecked_mut() };
        (me.receiver, me.buf, me.limit)
    }
}

impl<'a, I> Future for BatchRecvFut<'a, I>
where
    I: iter::Extend<QueueValue>,
{
    type Output = usize;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let (receiver, buf, limit) = self.project();
        let count = receiver.batch_recv(buf, limit);

        if count == 0 {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(count)
        }
    }
}

// SAFETY: internal queue has atomic pointers to head and tails, and thus is safe to send
unsafe impl Send for Receiver {}
