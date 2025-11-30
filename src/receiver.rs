use std::{
    cmp, iter,
    pin::Pin,
    ptr::NonNull,
    task::{Context, Poll},
};

use futures::Future;

use crate::{Queue, QueueValue, sev, sync::*, wfe};

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

            if cur_head == self.local_tail {
                // PHASE 1: Optimistic Spin (Hot Potato)
                // Spin for a short time to catch immediate updates.
                // 100 iterations is usually enough to cover the cross-core latency (~40-80ns)
                // without burning excessive CPU.
                for _ in 0..1000 {
                    let new_val = buffer.tail.load(Ordering::Acquire);
                    if new_val != self.local_tail {
                        self.local_tail = new_val;
                        break;
                    }
                    hint::spin_loop();
                }

                // PHASE 2: Power-Saving Wait (WFE)
                // If we waited 100 spins and nothing happened, the other thread
                // is likely busy or descheduled. Sleep until signalled.
                while cur_head == self.local_tail {
                    let new_val = buffer.tail.load(Ordering::Acquire);
                    if new_val != self.local_tail {
                        self.local_tail = new_val;
                        break;
                    }
                    wfe();
                }
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

        // CONDITIONAL SIGNALING: Check if buffer was full BEFORE this read
        // Calculate occupied slots before the read
        let occupied = if self.local_tail >= cur_head {
            self.local_tail - cur_head
        } else {
            buffer.capacity - (cur_head - self.local_tail)
        };
        let was_full = occupied >= buffer.capacity - 1;

        // this is fine as we are the only ones writing to head
        buffer.head.store(new_head, Ordering::Release);

        // Only wake producer if buffer was previously full (producer might be sleeping)
        if was_full {
            sev();
        }

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

        sev();

        count
    }

    /// Returns a slice of available data to read for zero-copy reads.
    ///
    /// This method provides direct access to the ring buffer's internal memory,
    /// allowing you to read data without intermediate copies. The slice represents
    /// contiguous available data in the buffer.
    ///
    /// After reading from the slice, you must call `advance()` to move the read
    /// position forward.
    ///
    /// Returns an empty slice if the buffer is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use gil::channel;
    /// use std::hint::black_box;
    ///
    /// let (mut tx, mut rx) = channel(128);
    /// tx.send(42);
    ///
    /// // Get readable slice
    /// let slice = rx.get_read_slice();
    /// let len = slice.len();
    /// if !slice.is_empty() {
    ///     // Process data directly from the buffer
    ///     black_box(slice[0]);
    /// }
    /// rx.advance(len);
    /// ```
    #[inline(always)]
    pub fn get_read_slice(&mut self) -> &[QueueValue] {
        let buffer = unsafe { self.buffer.as_ref() };
        let head = buffer.head.load(Ordering::Relaxed);

        if head == self.local_tail {
            self.local_tail = buffer.tail.load(Ordering::Acquire);
            if head == self.local_tail {
                return &[]; // Empty
            }
        }

        // Calculate available data
        let available_total = if self.local_tail >= head {
            self.local_tail - head
        } else {
            buffer.capacity - (head - self.local_tail)
        };

        let contiguous_data = buffer.capacity - head;
        let len = cmp::min(contiguous_data, available_total);

        unsafe {
            let ptr = Queue::elem(self.buffer.as_ptr(), head) as *const QueueValue;
            std::slice::from_raw_parts(ptr, len)
        }
    }

    /// Advances the read position by `count` items.
    ///
    /// This must be called after reading data from the slice obtained from
    /// `get_read_slice()` to make the space available to the sender again.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `count` does not exceed the length of the
    /// slice that was returned by the last `get_read_slice()` call.
    ///
    /// # Examples
    ///
    /// ```
    /// use gil::channel;
    ///
    /// let (mut tx, mut rx) = channel(128);
    /// tx.send(42);
    ///
    /// let slice = rx.get_read_slice();
    /// let count = slice.len();
    /// // Process slice...
    /// rx.advance(count);
    /// ```
    #[inline(always)]
    pub fn advance(&mut self, count: usize) {
        if count == 0 {
            return;
        }

        let buffer = unsafe { self.buffer.as_ref() };
        let head = buffer.head.load(Ordering::Relaxed);
        let mut new_head = head + count;
        if new_head >= buffer.capacity {
            new_head -= buffer.capacity;
        }

        buffer.head.store(new_head, Ordering::Release);
        sev()
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
