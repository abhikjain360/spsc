use std::ptr::NonNull;

mod queue;
mod receiver;
mod sender;
mod sync;

use queue::Queue;
use receiver::Receiver;
use sender::Sender;
use sync::*;

#[inline]
pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    let queue = Queue::with_capacity(capacity);
    queue.rc.store(2, Ordering::SeqCst);

    let queue = Box::new(queue);
    let queue_ptr = NonNull::new(Box::into_raw(queue)).unwrap();
    (Sender::new(queue_ptr), Receiver::new(queue_ptr))
}

#[cfg(test)]
mod test {
    use std::collections::VecDeque;

    #[cfg(loomer)]
    use loom::{sync::Arc, thread};
    #[cfg(not(loomer))]
    use std::{sync::Arc, thread};

    use super::*;

    #[test]
    fn test_valid_sends() {
        const COUNTS: usize = 4096;
        let (mut tx, mut rx) = channel(COUNTS);

        thread::spawn(move || {
            for i in 0..COUNTS << 12 {
                tx.send(i);
            }
        });

        for i in 0..COUNTS << 12 {
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

    #[cfg(loomer)]
    #[test]
    fn loom_tester() {
        loom::model(|| {
            const COUNTS: usize = 3;
            let (mut tx, mut rx) = channel(COUNTS);

            thread::spawn(move || {
                for i in 0..COUNTS + 4 {
                    tx.send(i);
                }
            });

            for i in 0..COUNTS + 4 {
                let r = rx.recv();
                assert_eq!(r, i);
            }
        });
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_async_send() {
        const COUNTS: usize = 4096;
        let (mut tx, mut rx) = channel(COUNTS);

        tokio::spawn(async move {
            for i in 0..COUNTS << 8 + 1 {
                tx.send_async(i).await;
            }
            drop(tx);
        });
        for i in 0..COUNTS << 8 + 1 {
            assert_eq!(rx.recv_async().await, i);
        }
    }

    #[test]
    fn test_batch_send() {
        const COUNTS: usize = 256;
        let (mut tx, mut rx) = channel(COUNTS);

        thread::spawn(move || {
            let v: Vec<u32> = (0..200).collect();
            for _ in 0..COUNTS << 1 {
                tx.batch_send_all(v.iter().map(|x| *x));
            }
        });

        for _ in 0..COUNTS << 1 {
            for i in 0..200 {
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_async_batch_send() {
        const COUNTS: usize = 256;
        let (mut tx, mut rx) = channel(COUNTS);

        tokio::spawn(async move {
            let v: Vec<u32> = (0..200).collect();
            for _ in 0..COUNTS << 1 {
                tx.batch_send_all_async(v.clone().into_iter()).await;
            }
        });

        for _ in 0..COUNTS << 1 {
            for i in 0..200 {
                assert_eq!(rx.recv(), i);
            }
        }
    }
}
