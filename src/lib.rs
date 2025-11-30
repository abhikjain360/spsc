#![doc = include_str!("../README.md")]

mod queue;
mod receiver;
mod sender;
mod sync;

use queue::Queue;
pub use receiver::Receiver;
pub use sender::Sender;
use sync::*;

/// Wait For Event - puts the core into a low-power state until an event occurs.
///
/// On ARM64 architectures, this uses the `wfe` instruction for power-efficient waiting.
/// On other architectures, or when testing with loom/miri, this is a no-op.
///
/// This is used internally by blocking operations when the queue is empty or full.
#[inline(always)]
pub(crate) fn wfe() {
    #[cfg(all(target_arch = "aarch64", not(loomer), not(miri)))]
    unsafe {
        core::arch::asm!("wfe", options(nomem, nostack, preserves_flags));
    }
}

/// Send Event - wakes up cores waiting in WFE state.
///
/// On ARM64 architectures, this uses the `sev` instruction to wake waiting cores.
/// On other architectures, or when testing with loom/miri, this is a no-op.
///
/// This is used internally after writing to the queue to potentially wake the other side.
#[inline(always)]
pub(crate) fn sev() {
    #[cfg(all(target_arch = "aarch64", not(loomer), not(miri)))]
    unsafe {
        core::arch::asm!("sev", options(nomem, nostack, preserves_flags));
    }
}

/// The value type stored in the queue on x86_64 architectures.
#[cfg(target_arch = "x86_64")]
pub type QueueValue = u64;

/// The value type stored in the queue on aarch64 (ARM64) architectures.
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
///
/// # Panics
///
/// Panics if memory allocation fails.
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
