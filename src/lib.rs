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
//! let (mut tx, mut rx) = channel::<u32>(100);
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
//! let (mut tx, mut rx) = channel::<u32>(100);
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

/// Creates a bounded single-producer single-consumer channel with the specified capacity.
///
/// Returns a tuple of `(Sender<T>, Receiver<T>)`. The sender can be used to push values
/// into the queue, and the receiver can be used to pull values from the queue.
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
/// let (mut tx, mut rx) = channel::<i32>(10);
/// tx.send(42);
/// assert_eq!(rx.recv(), 42);
/// ```
#[inline]
pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
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
                    tx.send(i);
                }
            });

            for i in 0..4 {
                let r = rx.recv();
                assert_eq!(r, i);
            }
        })
    }

    #[test]
    fn test_valid_sends() {
        const COUNTS: usize = 4096;
        let (mut tx, mut rx) = channel(COUNTS);

        thread::spawn(move || {
            for i in 0..COUNTS << 3 {
                tx.send(i);
            }
        });

        for i in 0..COUNTS << 3 {
            let r = rx.recv();
            assert_eq!(r, i);
        }
    }

    struct TestSruct(Arc<AtomicUsize>);
    impl TestSruct {
        fn new(counter: Arc<AtomicUsize>) -> Self {
            counter.fetch_add(1, Ordering::SeqCst);
            Self(counter)
        }
    }
    impl Drop for TestSruct {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_teststruct() {
        let counter = Arc::new(AtomicUsize::new(0));
        const COUNTS: usize = 16;
        let mut v = Vec::with_capacity(COUNTS);
        for i in 0..COUNTS {
            assert_eq!(counter.load(Ordering::SeqCst), i);
            v.push(TestSruct::new(counter.clone()));
        }
        assert_eq!(counter.load(Ordering::SeqCst), COUNTS);
        drop(v);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_proper_drops() {
        let counter = Arc::new(AtomicUsize::new(0));
        const COUNTS: usize = 4096;

        {
            let (mut tx, _) = channel(COUNTS);
            for i in 0..COUNTS {
                assert_eq!(counter.load(Ordering::SeqCst), i);
                tx.send(TestSruct::new(counter.clone()));
            }
            assert_eq!(counter.load(Ordering::SeqCst), COUNTS);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_async_send() {
        futures::executor::block_on(async {
            const COUNTS: usize = 4096;

            let (mut tx, mut rx) = channel(COUNTS);

            thread::spawn(move || {
                for i in 0..COUNTS << 1 {
                    futures::executor::block_on(tx.send_async(i));
                }
                drop(tx);
            });
            for i in 0..COUNTS << 1 {
                assert_eq!(rx.recv_async().await, i);
            }
        });
    }

    #[test]
    fn test_batch_send() {
        const COUNTS: usize = 256;
        let (mut tx, mut rx) = channel(COUNTS);

        thread::spawn(move || {
            let v: Vec<u32> = (0..COUNTS as u32).collect();
            for _ in 0..COUNTS << 1 {
                tx.batch_send_all(v.iter().copied());
            }
        });

        for _ in 0..COUNTS << 1 {
            for i in 0..COUNTS as u32 {
                assert_eq!(rx.recv(), i);
            }
        }
    }

    #[test]
    fn test_batch_recv() {
        const COUNTS: usize = 256;

        let (mut tx, mut rx) = channel(COUNTS);

        thread::spawn(move || {
            for i in 0..(COUNTS << 8) + (COUNTS >> 1) + 1 {
                tx.send(i);
            }
        });

        let mut buf = VecDeque::with_capacity(128);
        let mut i = 0;
        while i < (COUNTS << 8) + (COUNTS >> 1) + 1 {
            rx.batch_recv(&mut buf, 128);
            for val in buf.drain(..) {
                assert_eq!(i, val);
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
                let v: Vec<u32> = (0..200).collect();
                for _ in 0..COUNTS << 1 {
                    futures::executor::block_on(tx.batch_send_all_async(v.clone().into_iter()));
                }
            });

            for _ in 0..COUNTS << 1 {
                for i in 0..200 {
                    assert_eq!(rx.recv(), i);
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
                    tx.send(i);
                }
            });

            let mut buf = VecDeque::with_capacity(128);
            let mut i = 0;
            while i < (COUNTS << 8) + (COUNTS >> 1) + 1 {
                rx.batch_recv_async(&mut buf, 128).await;
                for val in buf.drain(..) {
                    assert_eq!(i, val);
                    i += 1;
                }
            }
        });
    }
}
