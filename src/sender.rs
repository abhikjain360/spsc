//! Producer side of the SPSC channel.
//!
//! This module contains the `Sender` type which allows sending values into the queue.
//! It supports both blocking and non-blocking operations, as well as async operations
//! and batch sends.

use std::{
    cmp, iter,
    pin::Pin,
    ptr::NonNull,
    task::{Context, Poll},
};

use futures::Future;

use crate::{Queue, QueueValue, sev, sync::*, wfe};

/// The sending half of the channel.
///
/// Values can be sent into the queue using this type. The sender is `Send` but not `Sync`,
/// meaning it can be moved between threads but cannot be shared.
///
/// # Examples
///
/// Basic usage:
///
/// ```
/// use gil::channel;
///
/// let (mut tx, mut rx) = channel(10);
/// tx.send(42);
/// assert_eq!(rx.recv(), 42);
/// ```
///
/// Async usage:
///
/// ```
/// use gil::channel;
///
/// # async fn example() {
/// let (mut tx, mut rx) = channel(10);
/// tx.send_async(42).await;
/// assert_eq!(rx.recv_async().await, 42);
/// # }
/// ```
pub struct Sender {
    buffer: NonNull<Queue>,
}

impl Sender {
    /// Creates a new sender from a queue pointer.
    ///
    /// This is an internal function used by the `channel` constructor.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Non-null pointer to the shared queue
    pub(crate) fn new(buffer: NonNull<Queue>) -> Self {
        Self { buffer }
    }

    /// Attempts to send a value into the queue without blocking.
    ///
    /// This method returns immediately, either successfully sending the value
    /// or returning it back if the queue is full.
    ///
    /// # Arguments
    ///
    /// * `val` - The value to send
    ///
    /// # Returns
    ///
    /// - `Ok(())` if the value was successfully sent
    /// - `Err(val)` if the queue is full, returning the value back
    ///
    /// # Examples
    ///
    /// ```
    /// use gil::channel;
    ///
    /// let (mut tx, mut rx) = channel(1);
    /// assert!(tx.try_send(1).is_ok());
    /// assert!(tx.try_send(2).is_err()); // Queue is full
    /// ```
    pub fn try_send(&self, val: QueueValue) -> Result<(), QueueValue> {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };

        let cur_tail = buffer.tail.load(Ordering::Relaxed);
        let new_tail = (cur_tail + 1) & buffer.mask_producer;

        let local_head = buffer.local_head.get();
        if local_head == new_tail {
            let new_local_head = buffer.head.load(Ordering::Acquire);
            buffer.local_head.set(new_local_head);

            if new_local_head == new_tail {
                return Err(val);
            }
        }

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

        Ok(())
    }

    /// Sends a value into the queue, blocking if the queue is full.
    ///
    /// This method will wait until space is available in the queue. It uses a two-phase
    /// waiting strategy:
    /// 1. Optimistic spinning for ~1000 iterations to catch immediate updates
    /// 2. Power-saving wait (WFE on ARM) if the queue remains full
    ///
    /// # Arguments
    ///
    /// * `val` - The value to send
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use gil::channel;
    ///
    /// let (mut tx, mut rx) = channel(10);
    ///
    /// thread::spawn(move || {
    ///     for i in 0..100 {
    ///         tx.send(i);
    ///     }
    /// });
    ///
    /// for i in 0..100 {
    ///     assert_eq!(rx.recv(), i);
    /// }
    /// ```
    pub fn send(&self, val: QueueValue) {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };

        let cur_tail = buffer.tail.load(Ordering::Relaxed);
        let new_tail = (cur_tail + 1) & buffer.mask_producer;

        let mut local_head = buffer.local_head.get();
        if local_head == new_tail {
            local_head = buffer.head.load(Ordering::Acquire);
            buffer.local_head.set(local_head);

            if local_head == new_tail {
                let mut spin_count = 0_u8;
                while local_head == new_tail {
                    let new_val = buffer.head.load(Ordering::Acquire);
                    if new_val != local_head {
                        local_head = new_val;
                        buffer.local_head.set(local_head);
                        break;
                    }

                    if spin_count < 100 {
                        spin_count += 1;
                        hint::spin_loop();
                    } else {
                        spin_count = 0;
                        thread::yield_now();
                    }
                }
            }
        }

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
    }

    /// Sends a value asynchronously into the queue.
    ///
    /// Returns a future that resolves when the value has been sent. The future will
    /// yield if the queue is full and retry on the next poll.
    ///
    /// # Arguments
    ///
    /// * `val` - The value to send
    ///
    /// # Returns
    ///
    /// A future that completes when the value has been sent.
    ///
    /// # Examples
    ///
    /// ```
    /// use gil::channel;
    ///
    /// # async fn example() {
    /// let (mut tx, mut rx) = channel(10);
    ///
    /// tx.send_async(42).await;
    /// assert_eq!(rx.recv_async().await, 42);
    /// # }
    /// ```
    #[inline]
    pub fn send_async(&mut self, val: QueueValue) -> SendFut<'_> {
        SendFut {
            sender: self,
            to_send: Some(val),
        }
    }

    /// Sends multiple values from an iterator, stopping when the queue is full.
    ///
    /// This is more efficient than calling `try_send` in a loop because it amortizes
    /// the cost of atomic operations.
    ///
    /// # Arguments
    ///
    /// * `vals` - An iterator of values to send
    ///
    /// # Returns
    ///
    /// The number of values successfully sent before the queue became full or the
    /// iterator was exhausted.
    ///
    /// # Examples
    ///
    /// ```
    /// use gil::channel;
    ///
    /// let (mut tx, mut rx) = channel(10);
    /// let values = vec![1, 2, 3, 4, 5];
    /// let sent = tx.batch_send(&mut values.into_iter());
    /// assert_eq!(sent, 5);
    /// ```
    pub fn batch_send(&self, vals: &mut impl Iterator<Item = QueueValue>) -> usize {
        let mut total_count = 0;

        loop {
            let count = {
                let slice = self.get_write_slice();
                if slice.is_empty() {
                    return total_count;
                }

                let mut count = 0;
                for slot in slice.iter_mut() {
                    match vals.next() {
                        Some(val) => {
                            // Write directly to the uninitialized memory
                            unsafe {
                                (slot as *mut QueueValue).write(val);
                            }
                            count += 1;
                        }
                        None => break,
                    }
                }

                count
            }; // slice is dropped here

            if count == 0 {
                break;
            }

            self.commit(count);
            total_count += count;
        }

        total_count
    }

    /// Sends all values from an iterator, blocking until all are sent.
    ///
    /// This method will block as necessary to send all values from the iterator.
    /// It's more efficient than calling `send` in a loop because it amortizes
    /// the cost of atomic operations.
    ///
    /// # Arguments
    ///
    /// * `vals` - An iterator of values to send
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use gil::channel;
    ///
    /// let (mut tx, mut rx) = channel(10);
    ///
    /// thread::spawn(move || {
    ///     let values: Vec<u128> = (0..100).collect();
    ///     tx.batch_send_all(values.into_iter());
    /// });
    ///
    /// for i in 0..100 {
    ///     assert_eq!(rx.recv(), i);
    /// }
    /// ```
    pub fn batch_send_all(&self, vals: impl Iterator<Item = QueueValue>) {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };

        let mut cur_tail = buffer.tail.load(Ordering::Relaxed);
        let mut new_tail = (cur_tail + 1) & buffer.mask_producer;

        for val in vals {
            let mut local_head = buffer.local_head.get();
            if local_head == new_tail {
                buffer.tail.store(cur_tail, Ordering::Release);

                local_head = buffer.head.load(Ordering::Acquire);
                buffer.local_head.set(local_head);

                if local_head == new_tail {
                    while local_head == new_tail {
                        let new_val = buffer.head.load(Ordering::Acquire);
                        if new_val != local_head {
                            local_head = new_val;
                            buffer.local_head.set(local_head);
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
            new_tail = (new_tail + 1) & buffer.mask_producer;
        }

        buffer.tail.store(cur_tail, Ordering::Release);

        sev();
    }

    /// Sends all values from an iterator asynchronously.
    ///
    /// Returns a future that sends all values from the iterator, yielding when
    /// the queue is full.
    ///
    /// # Arguments
    ///
    /// * `vals` - An iterator of values to send
    ///
    /// # Returns
    ///
    /// A future that resolves to the total number of values sent.
    ///
    /// # Examples
    ///
    /// ```
    /// use gil::channel;
    ///
    /// # async fn example() {
    /// let (mut tx, mut rx) = channel(10);
    /// let values: Vec<u128> = (0..100).collect();
    /// let sent = tx.batch_send_all_async(values.into_iter()).await;
    /// assert_eq!(sent, 100);
    /// # }
    /// ```
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
    ///             data.as_ptr(),
    ///             slice.as_mut_ptr(),
    ///             count
    ///         );
    ///     }
    /// }
    /// tx.commit(count);
    /// ```
    #[expect(clippy::mut_from_ref)]
    pub fn get_write_slice(&self) -> &mut [QueueValue] {
        let buffer = unsafe { self.buffer.as_ref() };
        let tail = buffer.tail.load(Ordering::Relaxed);

        let mut local_head = buffer.local_head.get();

        // Check if we need to refresh local_head
        let occupied = if tail >= local_head {
            tail - local_head
        } else {
            buffer.capacity - (local_head - tail)
        };

        if occupied >= buffer.capacity - 1 {
            // Refresh head to see if consumer moved
            local_head = buffer.head.load(Ordering::Acquire);
            buffer.local_head.set(local_head);

            let occupied = if tail >= local_head {
                tail - local_head
            } else {
                buffer.capacity - (local_head - tail)
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
            let ptr = Queue::elem(self.buffer.as_ptr(), tail) as *mut QueueValue;
            std::slice::from_raw_parts_mut(ptr, len)
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
    ///     slice[i] = i as u128;
    /// }
    /// tx.commit(count);
    /// ```
    #[inline(always)]
    pub fn commit(&self, count: usize) {
        if count == 0 {
            return;
        }

        let buffer = unsafe { self.buffer.as_ref() };
        let tail = buffer.tail.load(Ordering::Relaxed);
        let new_tail = (tail + count) & buffer.mask_producer;

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

/// Future returned by `send_async()`.
///
/// This future will attempt to send a value, yielding if the queue is full.
pub struct SendFut<'sender> {
    sender: &'sender mut Sender,
    to_send: Option<QueueValue>,
}

impl<'sender> SendFut<'sender> {
    #[inline]
    fn project(self: Pin<&mut Self>) -> (&mut Sender, &mut Option<QueueValue>) {
        // SAFETY: we should NEVER move out any values
        let me = unsafe { self.get_unchecked_mut() };
        (&mut *me.sender, &mut me.to_send)
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

/// Future returned by `batch_send_all_async()`.
///
/// This future will send all values from an iterator, yielding when the queue is full.
pub struct BatchSendAllFut<'sender, I: Iterator<Item = QueueValue>> {
    sender: &'sender mut Sender,
    iter: iter::Peekable<I>,
    total_count: usize,
}

impl<'sender, I: Iterator<Item = QueueValue>> BatchSendAllFut<'sender, I> {
    fn project(self: Pin<&mut Self>) -> (&mut Sender, &mut iter::Peekable<I>, &mut usize) {
        // SAFETY: we should NEVER move out any values
        let me = unsafe { self.get_unchecked_mut() };
        (&mut *me.sender, &mut me.iter, &mut me.total_count)
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

// SAFETY: The Sender owns a NonNull<Queue> which points to a heap allocation that outlives
// the Sender. The queue uses atomic operations for cross-thread synchronization.
// Sender is Send (can be moved between threads) but NOT Sync (cannot be shared via &Sender)
// because methods use Cell for local caching which is not thread-safe.
unsafe impl Send for Sender {}
