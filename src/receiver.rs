//! Consumer side of the SPSC channel.
//!
//! This module contains the `Receiver` type which allows receiving values from the queue.
//! It supports both blocking and non-blocking operations, as well as async operations
//! and batch receives.

use std::{
    cmp, iter,
    pin::Pin,
    ptr::NonNull,
    task::{Context, Poll},
};

use futures::Future;

use crate::{Queue, QueueValue, sev, sync::*};

/// The receiving half of the channel.
///
/// Values can be received from the queue using this type. The receiver is `Send` but not `Sync`,
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
pub struct Receiver {
    buffer: NonNull<Queue>,
}

impl Receiver {
    /// Creates a new receiver from a queue pointer.
    ///
    /// This is an internal function used by the `channel` constructor.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Non-null pointer to the shared queue
    pub(crate) fn new(buffer: NonNull<Queue>) -> Self {
        Self { buffer }
    }

    /// Attempts to receive a value from the queue without blocking.
    ///
    /// This method returns immediately, either returning a value if one is available
    /// or `None` if the queue is empty.
    ///
    /// # Returns
    ///
    /// - `Some(value)` if a value was available
    /// - `None` if the queue is empty
    ///
    /// # Examples
    ///
    /// ```
    /// use gil::channel;
    ///
    /// let (mut tx, mut rx) = channel(10);
    /// assert_eq!(rx.try_recv(), None); // Queue is empty
    /// tx.send(42);
    /// assert_eq!(rx.try_recv(), Some(42));
    /// ```
    pub fn try_recv(&mut self) -> Option<QueueValue> {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };

        let cur_head = buffer.head.load(Ordering::Relaxed);

        let local_tail = buffer.local_tail.get();
        if cur_head == local_tail {
            let new_local_tail = buffer.tail.load(Ordering::Acquire);
            buffer.local_tail.set(new_local_tail);

            // retry with updated head value
            // checking twice is still cheaper than reading atomically twice
            if cur_head == new_local_tail {
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

        let new_head = (cur_head + 1) & buffer.mask_consumer;

        // this is fine as we are the only ones writing to head
        buffer.head.store(new_head, Ordering::Release);

        Some(val)
    }

    /// Receives a value from the queue, blocking if the queue is empty.
    ///
    /// This method will wait until a value is available in the queue. It uses a two-phase
    /// waiting strategy:
    /// 1. Optimistic spinning for ~1000 iterations to catch immediate updates
    /// 2. Power-saving wait (WFE on ARM) if the queue remains empty
    ///
    /// # Returns
    ///
    /// The received value.
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
    pub fn recv(&mut self) -> QueueValue {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };

        let cur_head = buffer.head.load(Ordering::Relaxed);

        let new_head = (cur_head + 1) & buffer.mask_consumer;

        let mut local_tail = buffer.local_tail.get();
        if cur_head == local_tail {
            local_tail = buffer.tail.load(Ordering::Acquire);
            buffer.local_tail.set(local_tail);

            if cur_head == local_tail {
                let mut spin_count = 0_u8;
                while cur_head == local_tail {
                    let new_val = buffer.tail.load(Ordering::Acquire);
                    if new_val != local_tail {
                        local_tail = new_val;
                        buffer.local_tail.set(local_tail);
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
        let val = unsafe {
            ((*Queue::elem(self.buffer.as_ptr(), cur_head)).get() as *const QueueValue).read()
        };

        #[cfg(loomer)]
        let val = unsafe {
            (*Queue::elem(self.buffer.as_ptr(), cur_head))
                .with(|ptr| (ptr as *const QueueValue).read())
        };

        // this is fine as we are the only ones writing to head
        buffer.head.store(new_head, Ordering::Release);

        val
    }

    /// Receives a value asynchronously from the queue.
    ///
    /// Returns a future that resolves when a value is available. The future will
    /// yield if the queue is empty and retry on the next poll.
    ///
    /// # Returns
    ///
    /// A future that completes with the received value.
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
    pub fn recv_async(&mut self) -> RecvFut<'_> {
        RecvFut { receiver: self }
    }

    /// Receives a value asynchronously from the queue into a buffer.
    ///
    /// Returns a future that receives up to `limit` values from the queue.
    /// The future yields if the queue is empty.
    ///
    /// # Arguments
    ///
    /// * `buf` - The buffer to extend with received values
    /// * `limit` - Maximum number of values to receive
    ///
    /// # Returns
    ///
    /// A future that completes with the number of values received.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::VecDeque;
    /// use gil::channel;
    ///
    /// # async fn example() {
    /// let (mut tx, mut rx) = channel(10);
    /// tx.send(1);
    /// tx.send(2);
    ///
    /// let mut buf = VecDeque::new();
    /// let count = rx.batch_recv_async(&mut buf, 10).await;
    /// assert_eq!(count, 2);
    /// # }
    /// ```
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

    /// Receives multiple values from the queue into a buffer, up to a limit.
    ///
    /// This is more efficient than calling `try_recv` in a loop because it amortizes
    /// the cost of atomic operations. It will receive as many values as are available,
    /// up to the specified limit, and then return immediately without blocking.
    ///
    /// # Arguments
    ///
    /// * `buf` - A collection that can be extended with received values
    /// * `limit` - Maximum number of values to receive
    ///
    /// # Returns
    ///
    /// The number of values received (0 if queue was empty).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::VecDeque;
    /// use gil::channel;
    ///
    /// let (mut tx, mut rx) = channel(10);
    /// tx.send(1);
    /// tx.send(2);
    /// tx.send(3);
    ///
    /// let mut buf = VecDeque::new();
    /// let count = rx.batch_recv(&mut buf, 10);
    /// assert_eq!(count, 3);
    /// assert_eq!(buf, vec![1, 2, 3]);
    /// ```
    pub fn batch_recv(&mut self, buf: &mut impl iter::Extend<QueueValue>, limit: usize) -> usize {
        let mut total_count = 0;

        while total_count < limit {
            let slice = self.get_read_slice();
            if slice.is_empty() {
                break;
            }

            let to_recv = cmp::min(slice.len(), limit - total_count);

            // TODO: change this to extend_one when it stabilizes
            buf.extend(slice[..to_recv].iter().copied());

            self.advance(to_recv);
            total_count += to_recv;
        }

        total_count
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

        let mut local_tail = buffer.local_tail.get();
        if head == local_tail {
            local_tail = buffer.tail.load(Ordering::Acquire);
            buffer.local_tail.set(local_tail);
            if head == local_tail {
                return &[]; // Empty
            }
        }

        // Calculate available data
        let available_total = if local_tail >= head {
            local_tail - head
        } else {
            buffer.capacity - (head - local_tail)
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
        let new_head = (head + count) & buffer.mask_consumer;

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

/// Future returned by `recv_async()`.
///
/// This future will attempt to receive a value, yielding if the queue is empty.
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

/// Future returned by `batch_recv_async()`.
///
/// This future will receive multiple values, yielding if the queue is empty.
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
