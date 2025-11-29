use std::{
    cmp, iter,
    mem::MaybeUninit,
    pin::Pin,
    ptr::NonNull,
    task::{Context, Poll},
};

use futures::Future;

use crate::{Queue, QueueValue, sev, sync::*, wfe};

pub struct Sender {
    buffer: NonNull<Queue>,
    local_head: usize,
}

impl Sender {
    pub(crate) fn new(buffer: NonNull<Queue>) -> Self {
        Self {
            buffer,
            local_head: 0,
        }
    }

    pub fn try_send(&mut self, val: QueueValue) -> Result<(), QueueValue> {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };

        let cur_tail = buffer.tail.load(Ordering::Relaxed);
        let mut new_tail = cur_tail + 1;
        if new_tail == buffer.capacity {
            new_tail = 0;
        }

        if self.local_head == new_tail {
            self.local_head = buffer.head.load(Ordering::Acquire);

            if self.local_head == new_tail {
                return Err(val);
            }
        }

        // CONDITIONAL SIGNALING: Check if buffer was empty BEFORE this write
        let was_empty = cur_tail == self.local_head;

        #[cfg(not(loomer))]
        unsafe {
            ((*Queue::elem(self.buffer.as_ptr(), cur_tail)).get() as *mut QueueValue).write(val);
        }

        #[cfg(loomer)]
        unsafe {
            (*Queue::elem(self.buffer.as_ptr(), cur_tail))
                .with_mut(|ptr| (ptr as *mut QueueValue).write(val));
        }

        // store is fine as we are the only ones writing to tail
        buffer.tail.store(new_tail, Ordering::Release);

        // Only wake consumer if buffer was previously empty (consumer might be sleeping)
        if was_empty {
            sev();
        }

        Ok(())
    }

    pub fn send(&mut self, val: QueueValue) {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };

        let cur_tail = buffer.tail.load(Ordering::Relaxed);
        let mut new_tail = cur_tail + 1;
        if new_tail == buffer.capacity {
            new_tail = 0;
        }

        if self.local_head == new_tail {
            self.local_head = buffer.head.load(Ordering::Acquire);

            if self.local_head == new_tail {
                // PHASE 1: Optimistic Spin (Hot Potato)
                // Spin for a short time to catch immediate updates.
                // 100 iterations is usually enough to cover the cross-core latency (~40-80ns)
                // without burning excessive CPU.
                for _ in 0..1000 {
                    let new_val = buffer.head.load(Ordering::Acquire);
                    if new_val != self.local_head {
                        self.local_head = new_val;
                        break;
                    }
                    hint::spin_loop();
                }

                // PHASE 2: Power-Saving Wait (WFE)
                // If we waited 100 spins and nothing happened, the other thread
                // is likely busy or descheduled. Sleep until signalled.
                while self.local_head == new_tail {
                    let new_val = buffer.head.load(Ordering::Acquire);
                    if new_val != self.local_head {
                        self.local_head = new_val;
                        break;
                    }
                    wfe();
                }
            }
        }

        // CONDITIONAL SIGNALING: Check if buffer was empty BEFORE this write
        let was_empty = cur_tail == self.local_head;

        #[cfg(not(loomer))]
        unsafe {
            ((*Queue::elem(self.buffer.as_ptr(), cur_tail)).get() as *mut QueueValue).write(val);
        }

        #[cfg(loomer)]
        unsafe {
            (*Queue::elem(self.buffer.as_ptr(), cur_tail))
                .with_mut(|ptr| (ptr as *mut QueueValue).write(val));
        }

        buffer.tail.store(new_tail, Ordering::Release);

        // Only wake consumer if buffer was previously empty (consumer might be sleeping)
        if was_empty {
            sev();
        }
    }

    #[inline]
    pub fn send_async(&mut self, val: QueueValue) -> SendFut<'_> {
        SendFut {
            sender: self,
            to_send: Some(val),
        }
    }

    pub fn batch_send(&mut self, vals: &mut impl Iterator<Item = QueueValue>) -> usize {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };
        let mut count = 0;

        let mut cur_tail = buffer.tail.load(Ordering::Relaxed);
        let mut new_tail = cur_tail + 1;
        if new_tail == buffer.capacity {
            new_tail = 0;
        }

        // Track if buffer was empty at the start
        let was_empty_at_start = cur_tail == self.local_head;

        loop {
            if self.local_head == new_tail {
                // PHASE 1: Optimistic Spin (Hot Potato)
                for _ in 0..100 {
                    let new_val = buffer.head.load(Ordering::Acquire);
                    if new_val != self.local_head {
                        self.local_head = new_val;
                        break;
                    }
                    hint::spin_loop();
                }

                // PHASE 2: Power-Saving Wait (WFE)
                if self.local_head == new_tail {
                    wfe();
                    buffer.tail.store(cur_tail, Ordering::Release);

                    self.local_head = buffer.head.load(Ordering::Acquire);
                    if self.local_head == new_tail {
                        return count;
                    }
                }
            }

            let val = match vals.next() {
                Some(v) => v,
                None => break,
            };

            #[cfg(not(loomer))]
            unsafe {
                ((*Queue::elem(self.buffer.as_ptr(), cur_tail)).get() as *mut QueueValue)
                    .write(val);
            }

            #[cfg(loomer)]
            unsafe {
                (*Queue::elem(self.buffer.as_ptr(), cur_tail))
                    .with_mut(|ptr| (ptr as *mut QueueValue).write(val));
            }

            count += 1;

            cur_tail = new_tail;
            new_tail += 1;
            if new_tail == buffer.capacity {
                new_tail = 0;
            }
        }

        buffer.tail.store(cur_tail, Ordering::Release);

        // Only wake consumer if buffer was empty when we started
        if was_empty_at_start {
            sev();
        }

        count
    }

    pub fn batch_send_all(&mut self, vals: impl Iterator<Item = QueueValue>) {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };

        let mut cur_tail = buffer.tail.load(Ordering::Relaxed);
        let mut new_tail = cur_tail + 1;
        if new_tail == buffer.capacity {
            new_tail = 0;
        }

        // Track if buffer was empty at the start
        let was_empty_at_start = cur_tail == self.local_head;

        for val in vals {
            if self.local_head == new_tail {
                buffer.tail.store(cur_tail, Ordering::Release);

                self.local_head = buffer.head.load(Ordering::Acquire);

                if self.local_head == new_tail {
                    // PHASE 1: Optimistic Spin (Hot Potato)
                    for _ in 0..100 {
                        let new_val = buffer.head.load(Ordering::Acquire);
                        if new_val != self.local_head {
                            self.local_head = new_val;
                            break;
                        }
                        hint::spin_loop();
                    }

                    // PHASE 2: Power-Saving Wait (WFE)
                    while self.local_head == new_tail {
                        let new_val = buffer.head.load(Ordering::Acquire);
                        if new_val != self.local_head {
                            self.local_head = new_val;
                            break;
                        }
                        wfe();
                    }
                }
            }

            #[cfg(not(loomer))]
            unsafe {
                ((*Queue::elem(self.buffer.as_ptr(), cur_tail)).get() as *mut QueueValue)
                    .write(val);
            }

            #[cfg(loomer)]
            unsafe {
                (*Queue::elem(self.buffer.as_ptr(), cur_tail))
                    .with_mut(|ptr| (ptr as *mut QueueValue).write(val));
            }

            cur_tail = new_tail;
            new_tail += 1;
            if new_tail == buffer.capacity {
                new_tail = 0;
            }
        }

        buffer.tail.store(cur_tail, Ordering::Release);

        // Only wake consumer if buffer was empty when we started
        if was_empty_at_start {
            sev();
        }
    }

    #[inline]
    pub fn batch_send_all_async<I: Iterator<Item = QueueValue>>(
        &mut self,
        vals: I,
    ) -> BatchSendAllFut<'_, I> {
        BatchSendAllFut {
            sender: self,
            iter: vals.peekable(),
            total_count: 0,
        }
    }

    /// Returns a mutable slice of available space in the buffer for zero-copy writes.
    ///
    /// This method provides direct access to the ring buffer's internal memory,
    /// allowing you to write data without intermediate copies. The slice represents
    /// contiguous available space in the buffer.
    ///
    /// After writing to the slice, you must call `commit()` to make the data visible
    /// to the receiver.
    ///
    /// Returns an empty slice if the buffer is full.
    ///
    /// # Examples
    ///
    /// ```
    /// use gil::channel;
    /// use std::ptr;
    ///
    /// let (mut tx, mut rx) = channel(128);
    ///
    /// // Get writable slice
    /// let slice = tx.get_write_slice();
    /// let data = [1u128, 2, 3, 4];
    /// let count = data.len().min(slice.len());
    /// if !slice.is_empty() {
    ///     // Write data directly
    ///     unsafe {
    ///         ptr::copy_nonoverlapping(
    ///             data.as_ptr() as *const std::mem::MaybeUninit<_>,
    ///             slice.as_mut_ptr(),
    ///             count
    ///         );
    ///     }
    /// }
    /// tx.commit(count);
    /// ```
    #[inline(always)]
    pub fn get_write_slice(&mut self) -> &mut [MaybeUninit<QueueValue>] {
        let buffer = unsafe { self.buffer.as_ref() };
        let tail = buffer.tail.load(Ordering::Relaxed);

        // Check if we need to refresh local_head
        let occupied = if tail >= self.local_head {
            tail - self.local_head
        } else {
            buffer.capacity - (self.local_head - tail)
        };

        if occupied >= buffer.capacity - 1 {
            // Refresh head to see if consumer moved
            self.local_head = buffer.head.load(Ordering::Acquire);

            let occupied = if tail >= self.local_head {
                tail - self.local_head
            } else {
                buffer.capacity - (self.local_head - tail)
            };

            if occupied >= buffer.capacity - 1 {
                return &mut []; // Full
            }
        }

        // Calculate contiguous available space
        let available_total = buffer.capacity - 1 - occupied;
        let contiguous_space = buffer.capacity - tail;

        // We can only give the contiguous part
        let len = cmp::min(contiguous_space, available_total);

        unsafe {
            let ptr = Queue::elem(self.buffer.as_ptr(), tail) as *mut MaybeUninit<QueueValue>;
            std::slice::from_raw_parts_mut(ptr as *mut MaybeUninit<QueueValue>, len)
        }
    }

    /// Commits `count` items that were written to the slice obtained from `get_write_slice()`.
    ///
    /// This makes the written data visible to the receiver. You must call this after
    /// writing to the slice, otherwise the receiver won't see your data.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `count` items have been properly initialized in the
    /// slice obtained from `get_write_slice()`. The count must not exceed the length
    /// of the slice that was returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use gil::channel;
    ///
    /// let (mut tx, mut rx) = channel(128);
    /// let slice = tx.get_write_slice();
    /// let count = slice.len().min(10);
    /// for i in 0..count {
    ///     slice[i].write(i as u128);
    /// }
    /// tx.commit(count);
    /// ```
    #[inline(always)]
    pub fn commit(&mut self, count: usize) {
        if count == 0 {
            return;
        }

        let buffer = unsafe { self.buffer.as_ref() };
        let tail = buffer.tail.load(Ordering::Relaxed);
        let mut new_tail = tail + count;
        if new_tail >= buffer.capacity {
            new_tail -= buffer.capacity;
        }

        buffer.tail.store(new_tail, Ordering::Release);

        sev();
    }
}

impl Drop for Sender {
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

pub struct SendFut<'sender> {
    sender: &'sender mut Sender,
    to_send: Option<QueueValue>,
}

impl<'sender> SendFut<'sender> {
    #[inline]
    fn project(self: Pin<&mut Self>) -> (&mut Sender, &mut Option<QueueValue>) {
        // SAFETY: we should NEVER move out any values
        let me = unsafe { self.get_unchecked_mut() };
        (me.sender, &mut me.to_send)
    }
}

impl<'sender> Future for SendFut<'sender> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let (sender, to_send) = self.project();
        match sender.try_send(to_send.take().unwrap()) {
            Ok(_) => Poll::Ready(()),
            Err(val) => {
                *to_send = Some(val);
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
}

pub struct BatchSendAllFut<'sender, I: Iterator<Item = QueueValue>> {
    sender: &'sender mut Sender,
    iter: iter::Peekable<I>,
    total_count: usize,
}

impl<'sender, I: Iterator<Item = QueueValue>> BatchSendAllFut<'sender, I> {
    fn project(self: Pin<&mut Self>) -> (&mut Sender, &mut iter::Peekable<I>, &mut usize) {
        // SAFETY: we should NEVER move out any values
        let me = unsafe { self.get_unchecked_mut() };
        (me.sender, &mut me.iter, &mut me.total_count)
    }
}

impl<'sender, I: Iterator<Item = QueueValue>> Future for BatchSendAllFut<'sender, I> {
    type Output = usize;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let (sender, vals, total_count) = self.project();
        *total_count += sender.batch_send(vals);
        match vals.peek() {
            Some(_) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            None => Poll::Ready(*total_count),
        }
    }
}

// SAFETY: internal queue has atomic pointers to head and tails, and thus is safe to send
unsafe impl Send for Sender {}
