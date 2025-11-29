//! # gil - Get In Line
//!
//! A fast single-producer single-consumer (SPSC) queue with both sync and async support.
//!
//! This crate provides a lock-free, bounded queue optimized for scenarios where you have
//! exactly one producer and one consumer. Both the producer and consumer can run on
//! different threads and can be moved between threads, but they cannot be shared.
//!
//! ## Features
//!
//! - **Lock-free**: Uses atomic operations for synchronization
//! - **High performance** (probably): Competitive with Facebook's folly implementation
//! - **Flexible APIs**: Blocking, non-blocking, and async operations
//! - **Batch operations**: Efficiently send/receive multiple items at once
//!
//! ## Example
//!
//! ```rust
//! use std::thread;
//! use gil::channel;
//!
//! let (mut tx, mut rx) = channel(100);
//!
//! thread::spawn(move || {
//!     for i in 0..100 {
//!         tx.send(i);
//!     }
//! });
//!
//! for i in 0..100 {
//!     let value = rx.recv();
//!     assert_eq!(value, i);
//! }
//! ```
//!
//! ## Async Example
//!
//! ```rust
//! use gil::channel;
//!
//! # async fn example() {
//! let (mut tx, mut rx) = channel(100);
//!
//! tokio::spawn(async move {
//!     for i in 0..100 {
//!         tx.send_async(i).await;
//!     }
//! });
//!
//! for i in 0..100 {
//!     let value = rx.recv_async().await;
//!     assert_eq!(value, i);
//! }
//! # }
//! ```

mod queue;
mod receiver;
mod sender;
mod sync;

use queue::Queue;
pub use receiver::Receiver;
pub use sender::Sender;
use sync::*;

/// Wait For Event - puts the core into a low-power state until an event occurs.
/// On non-aarch64, loom, or miri, this is a no-op.
#[inline(always)]
pub(crate) fn wfe() {
    #[cfg(all(target_arch = "aarch64", not(loomer), not(miri)))]
    unsafe {
        core::arch::asm!("wfe", options(nomem, nostack, preserves_flags));
    }
}

/// Send Event - wakes up cores waiting in WFE state.
/// On non-aarch64, loom, or miri, this is a no-op.
#[inline(always)]
pub(crate) fn sev() {
    #[cfg(all(target_arch = "aarch64", not(loomer), not(miri)))]
    unsafe {
        core::arch::asm!("sev", options(nomem, nostack, preserves_flags));
    }
}

#[cfg(target_arch = "x86_64")]
pub type QueueValue = u64;

#[cfg(target_arch = "aarch64")]
pub type QueueValue = u128;

/// Creates a bounded single-producer single-consumer channel with the specified capacity.
///
/// Returns a tuple of `(Sender, Receiver)`. The sender can be used to push values
/// into the queue, and the receiver can be used to pull values from the queue.
///
/// The queue works with `u128` on aarch64 and `u64` on x86_64.
///
/// # Arguments
///
/// * `capacity` - The maximum number of items that can be stored in the queue at once.
///   This capacity is fixed and cannot be changed after creation.
///
/// # Examples
///
/// ```
/// use gil::channel;
///
/// let (mut tx, mut rx) = channel(10);
/// tx.send(42);
/// assert_eq!(rx.recv(), 42);
/// ```
#[inline]
pub fn channel(capacity: usize) -> (Sender, Receiver) {
    let queue_ptr = Queue::with_capacity(capacity);
    unsafe {
        queue_ptr.as_ref().rc.store(2, Ordering::SeqCst);
    }

    (Sender::new(queue_ptr), Receiver::new(queue_ptr))
}

#[cfg(test)]
mod test {
    use std::collections::VecDeque;

    use super::*;

    #[cfg(loomer)]
    #[test]
    fn basic_loomer() {
        loom::model(|| {
            let (mut tx, mut rx) = channel(4);

            thread::spawn(move || {
                for i in 0..4 {
                    tx.send(i as QueueValue);
                }
            });

            for i in 0..4 {
                let r = rx.recv();
                assert_eq!(r, i as QueueValue);
            }
        })
    }

    #[test]
    fn test_valid_sends() {
        const COUNTS: usize = 4096;
        let (mut tx, mut rx) = channel(COUNTS);

        thread::spawn(move || {
            for i in 0..COUNTS << 3 {
                tx.send(i as QueueValue);
            }
        });

        for i in 0..COUNTS << 3 {
            let r = rx.recv();
            assert_eq!(r, i as QueueValue);
        }
    }

    #[test]
    fn test_async_send() {
        futures::executor::block_on(async {
            const COUNTS: usize = 4096;

            let (mut tx, mut rx) = channel(COUNTS);

            thread::spawn(move || {
                for i in 0..COUNTS << 1 {
                    futures::executor::block_on(tx.send_async(i as QueueValue));
                }
                drop(tx);
            });
            for i in 0..COUNTS << 1 {
                assert_eq!(rx.recv_async().await, i as QueueValue);
            }
        });
    }

    #[test]
    fn test_batch_send() {
        const COUNTS: usize = 256;
        let (mut tx, mut rx) = channel(COUNTS);

        thread::spawn(move || {
            let v: Vec<QueueValue> = (0..COUNTS).map(|i| i as QueueValue).collect();
            for _ in 0..COUNTS << 1 {
                tx.batch_send_all(v.iter().copied());
            }
        });

        for _ in 0..COUNTS << 1 {
            for i in 0..COUNTS {
                assert_eq!(rx.recv(), i as QueueValue);
            }
        }
    }

    #[test]
    fn test_batch_recv() {
        const COUNTS: usize = 256;

        let (mut tx, mut rx) = channel(COUNTS);

        thread::spawn(move || {
            for i in 0..(COUNTS << 8) + (COUNTS >> 1) + 1 {
                tx.send(i as QueueValue);
            }
        });

        let mut buf = VecDeque::with_capacity(128);
        let mut i = 0;
        while i < (COUNTS << 8) + (COUNTS >> 1) + 1 {
            rx.batch_recv(&mut buf, 128);
            for val in buf.drain(..) {
                assert_eq!(i as QueueValue, val);
                i += 1;
            }
        }
    }

    #[test]
    fn test_async_batch_send() {
        futures::executor::block_on(async {
            const COUNTS: usize = 256;
            let (mut tx, mut rx) = channel(COUNTS);

            thread::spawn(move || {
                let v: Vec<QueueValue> = (0..200).map(|i| i as QueueValue).collect();
                for _ in 0..COUNTS << 1 {
                    futures::executor::block_on(tx.batch_send_all_async(v.clone().into_iter()));
                }
            });

            for _ in 0..COUNTS << 1 {
                for i in 0..200 {
                    assert_eq!(rx.recv(), i as QueueValue);
                }
            }
        });
    }

    #[test]
    fn test_async_batch_recv() {
        futures::executor::block_on(async {
            const COUNTS: usize = 256;
            let (mut tx, mut rx) = channel(COUNTS);

            thread::spawn(move || {
                for i in 0..(COUNTS << 8) + (COUNTS >> 1) + 1 {
                    tx.send(i as QueueValue);
                }
            });

            let mut buf = VecDeque::with_capacity(128);
            let mut i = 0;
            while i < (COUNTS << 8) + (COUNTS >> 1) + 1 {
                rx.batch_recv_async(&mut buf, 128).await;
                for val in buf.drain(..) {
                    assert_eq!(i as QueueValue, val);
                    i += 1;
                }
            }
        });
    }
}
