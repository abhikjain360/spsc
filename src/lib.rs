#![doc = include_str!("../README.md")]

use std::num::NonZeroUsize;
#[allow(unused_imports)]
#[cfg(not(feature = "loom"))]
pub(crate) use std::{
    alloc, cell, hint,
    sync::{self, atomic},
    thread,
};

use crate::queue::QueuePtr;
pub use crate::{receiver::Receiver, sender::Sender};
#[allow(unused_imports)]
#[cfg(feature = "loom")]
pub(crate) use loom::{
    alloc, cell, hint,
    sync::{self, atomic},
    thread,
};

mod padded;
mod queue;
mod receiver;
mod sender;

/// Creates a new single-producer single-consumer (SPSC) queue.
///
/// The queue has a fixed capacity and is lock-free. The capacity is rounded up to the next power of two.
///
/// # Arguments
///
/// * `capacity` - The minimum capacity of the queue. Will be rounded up to the next power of two.
///
/// # Returns
///
/// A tuple containing the [`Sender`] and [`Receiver`] handles.
///
/// # Examples
///
/// ```
/// use std::num::NonZeroUsize;
/// use gil::channel;
///
/// let (tx, rx) = channel(NonZeroUsize::new(1024).unwrap());
/// ```
pub fn channel(capacity: NonZeroUsize) -> (Sender, Receiver) {
    let (queue, mask) = QueuePtr::with_capacity(capacity);
    (Sender::new(queue.clone(), mask), Receiver::new(queue, mask))
}

#[cfg(all(test, not(feature = "loom")))]
mod test {
    use super::*;

    #[test]
    fn test_valid_sends() {
        const COUNTS: NonZeroUsize = NonZeroUsize::new(4096).unwrap();
        let (mut tx, mut rx) = channel(COUNTS);

        thread::spawn(move || {
            for i in 0..COUNTS.get() << 3 {
                tx.send(i as usize);
            }
        });

        for i in 0..COUNTS.get() << 3 {
            let r = rx.recv();
            assert_eq!(r, i as usize);
        }
    }

    #[cfg(feature = "async")]
    #[test]
    fn test_async_send() {
        futures::executor::block_on(async {
            const COUNTS: NonZeroUsize = NonZeroUsize::new(4096).unwrap();

            let (mut tx, mut rx) = channel(COUNTS);

            thread::spawn(move || {
                for i in 0..COUNTS.get() << 1 {
                    futures::executor::block_on(tx.send_async(i));
                }
                drop(tx);
            });
            for i in 0..COUNTS.get() << 1 {
                assert_eq!(rx.recv_async().await, i);
            }
        });
    }

    #[test]
    fn test_batched_send_recv() {
        const CAPACITY: NonZeroUsize = NonZeroUsize::new(1024).unwrap();
        const TOTAL_ITEMS: usize = 1024 << 4;
        let (mut tx, mut rx) = channel(CAPACITY);

        thread::spawn(move || {
            let mut sent = 0;
            while sent < TOTAL_ITEMS {
                let buffer = tx.write_buffer();
                let batch_size = buffer.len().min(TOTAL_ITEMS - sent);
                for i in 0..batch_size {
                    buffer[i].write(sent + i);
                }
                unsafe { tx.commit(batch_size) };
                sent += batch_size;
            }
        });

        let mut received = 0;
        let mut expected = 0;

        while received < TOTAL_ITEMS {
            let buffer = rx.read_buffer();
            if buffer.is_empty() {
                continue;
            }
            for &value in buffer.iter() {
                assert_eq!(value, expected);
                expected += 1;
            }
            let count = buffer.len();
            unsafe { rx.advance(count) };
            received += count;
        }

        assert_eq!(received, TOTAL_ITEMS);
    }
}

#[cfg(all(test, feature = "loom"))]
mod loom_test {
    use super::*;

    #[test]
    fn basic_loom() {
        loom::model(|| {
            let (mut tx, mut rx) = channel(NonZeroUsize::new(4).unwrap());
            let counts = 7;

            thread::spawn(move || {
                for i in 0..counts {
                    tx.send(i);
                }
            });

            for i in 0..counts {
                let r = rx.recv();
                assert_eq!(r, i);
            }
        })
    }
}
