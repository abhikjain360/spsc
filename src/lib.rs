use std::{
    cell::UnsafeCell,
    iter,
    mem::MaybeUninit,
    pin::Pin,
    ptr::{self, NonNull},
    task::{Context, Poll},
};

#[cfg(loomer)]
use loom::{
    hint,
    sync::atomic::{AtomicUsize, Ordering},
};

#[cfg(not(loomer))]
use std::{
    hint,
    sync::atomic::{AtomicUsize, Ordering},
};

use futures::Future;

/// The inner queue used by `Sender` and `Receiver`.
///
/// # Invariants
/// - head is valid value, unlesss head == tail,
/// - tail is invalid, where we add next value
pub struct Queue<T> {
    buffer: Box<[UnsafeCell<MaybeUninit<T>>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    rc: AtomicUsize,
    capacity: usize,
}

pub struct Sender<T> {
    buffer: NonNull<Queue<T>>,
    local_head: usize,
}

pub struct Receiver<T> {
    buffer: NonNull<Queue<T>>,
    local_tail: usize,
}

#[inline]
pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    let queue = Queue::with_capacity(capacity);
    queue.rc.store(2, Ordering::SeqCst);

    let queue = Box::new(queue);
    let queue_ptr = NonNull::new(Box::into_raw(queue)).unwrap();
    (
        Sender {
            buffer: queue_ptr,
            local_head: 0,
        },
        Receiver {
            buffer: queue_ptr,
            local_tail: 0,
        },
    )
}

impl<T> Queue<T> {
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity + 1;
        let buffer = (0..capacity)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();

        Self {
            buffer,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            rc: AtomicUsize::new(0),
            capacity,
        }
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        let mut head = self.head.load(Ordering::SeqCst);
        let tail = self.tail.load(Ordering::SeqCst);

        while head != tail {
            // SAFETY: we own all the existing values in the queue.
            unsafe {
                ptr::drop_in_place((&mut *self.buffer[head].get()).as_mut_ptr());
            }
            head = head + 1;
            if head == self.capacity {
                head = 0;
            }
        }
    }
}

impl<T> Sender<T> {
    pub fn try_send(&mut self, val: T) -> Result<(), T> {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_mut() };

        let cur_tail = buffer.tail.load(Ordering::Relaxed);
        let mut new_tail = cur_tail + 1;
        if new_tail == buffer.capacity {
            new_tail = 0;
        }

        if self.local_head == new_tail {
            self.local_head = buffer.head.load(Ordering::Relaxed);

            if self.local_head == new_tail {
                return Err(val);
            }
        }

        buffer.buffer[cur_tail].get_mut().write(val);

        // store is fine as we are the only ones writing to tail
        buffer.tail.store(new_tail, Ordering::Relaxed);

        Ok(())
    }

    pub fn send(&mut self, val: T) {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_mut() };

        let cur_tail = buffer.tail.load(Ordering::Relaxed);
        let mut new_tail = cur_tail + 1;
        if new_tail == buffer.capacity {
            new_tail = 0;
        }

        if self.local_head == new_tail {
            self.local_head = buffer.head.load(Ordering::Acquire);

            while self.local_head == new_tail {
                hint::spin_loop();
                self.local_head = buffer.head.load(Ordering::Acquire);
            }
        }

        buffer.buffer[cur_tail].get_mut().write(val);

        buffer.tail.store(new_tail, Ordering::Release);
    }

    #[inline]
    pub fn send_async(&mut self, val: T) -> SendFut<'_, T> {
        SendFut {
            sender: self,
            to_send: Some(val),
        }
    }

    pub fn batch_send(&mut self, vals: &mut impl Iterator<Item = T>) -> usize {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_mut() };
        let mut count = 0;

        let mut cur_tail = buffer.tail.load(Ordering::Relaxed);
        let mut new_tail = cur_tail + 1;
        if new_tail == buffer.capacity {
            new_tail = 0;
        }

        loop {
            if self.local_head == new_tail {
                buffer.tail.store(cur_tail, Ordering::Release);

                self.local_head = buffer.head.load(Ordering::Acquire);
                if self.local_head == new_tail {
                    return count;
                }
            }

            let val = match vals.next() {
                Some(v) => v,
                None => break,
            };

            buffer.buffer[cur_tail].get_mut().write(val);
            count += 1;

            cur_tail = new_tail;
            new_tail += 1;
            if new_tail == buffer.capacity {
                new_tail = 0;
            }
        }

        buffer.tail.store(cur_tail, Ordering::Release);
        count
    }

    pub fn batch_send_all(&mut self, vals: impl Iterator<Item = T>) {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_mut() };

        let mut cur_tail = buffer.tail.load(Ordering::Relaxed);
        let mut new_tail = cur_tail + 1;
        if new_tail == buffer.capacity {
            new_tail = 0;
        }

        for val in vals {
            if self.local_head == new_tail {
                buffer.tail.store(cur_tail, Ordering::Release);

                self.local_head = buffer.head.load(Ordering::Acquire);
                while self.local_head == new_tail {
                    hint::spin_loop();
                    self.local_head = buffer.head.load(Ordering::Acquire);
                }
            }

            buffer.buffer[cur_tail].get_mut().write(val);

            cur_tail = new_tail;
            new_tail += 1;
            if new_tail == buffer.capacity {
                new_tail = 0;
            }
        }

        buffer.tail.store(cur_tail, Ordering::Release);
    }

    #[inline]
    pub fn batch_send_all_async<I: Iterator<Item = T>>(
        &mut self,
        vals: I,
    ) -> BatchSendAllFut<'_, T, I> {
        BatchSendAllFut {
            sender: self,
            iter: vals.peekable(),
            total_count: 0,
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        // SAFETY: we are the only ones accessing it apart from other end which should not drop the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };
        let rc = buffer.rc.fetch_sub(1, Ordering::SeqCst);

        if rc == 1 {
            // SAFETY: only dropped when buffer.rc == 0, so this is fine as other end shouldn't
            // attempt to drop it.
            unsafe {
                ptr::drop_in_place(self.buffer.as_ptr());
            }
        }
    }
}

impl<T> Receiver<T> {
    pub fn try_recv(&mut self) -> Option<T> {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_mut() };

        let cur_head = buffer.head.load(Ordering::Relaxed);

        if cur_head == self.local_tail {
            self.local_tail = buffer.tail.load(Ordering::Relaxed);

            // cretry with updated head value
            // hecking twice is still cheaper than reading atomically twice
            if cur_head == self.local_tail {
                return None;
            }
        }

        let val = unsafe { ptr::read(buffer.buffer[cur_head].get() as *const _) };

        let mut new_head = cur_head + 1;
        if new_head == buffer.capacity {
            new_head = 0;
        }

        // this is fine as we are the only ones writing to head
        buffer.head.store(new_head, Ordering::Relaxed);

        Some(val)
    }

    pub fn recv(&mut self) -> T {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_mut() };

        let cur_head = buffer.head.load(Ordering::Relaxed);

        if cur_head == self.local_tail {
            self.local_tail = buffer.tail.load(Ordering::Acquire);

            while cur_head == self.local_tail {
                hint::spin_loop();
                self.local_tail = buffer.tail.load(Ordering::Acquire);
            }
        }

        let val = unsafe { ptr::read(buffer.buffer[cur_head].get() as *const _) };

        let mut new_head = cur_head + 1;
        if new_head == buffer.capacity {
            new_head = 0;
        }

        // this is fine as we are the only ones writing to head
        buffer.head.store(new_head, Ordering::Release);

        val
    }

    #[inline]
    pub fn recv_async(&mut self) -> RecvFut<'_, T> {
        RecvFut { receiver: self }
    }

    pub fn batch_recv(&mut self, buf: &mut impl iter::Extend<T>, limit: usize) -> usize {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_mut() };
        let mut count = 0;

        let mut cur_head = buffer.head.load(Ordering::Relaxed);

        while count < limit {
            if cur_head == self.local_tail {
                self.local_tail = buffer.tail.load(Ordering::Relaxed);

                // cretry with updated head value
                // hecking twice is still cheaper than reading atomically twice
                if cur_head == self.local_tail {
                    break;
                }
            }

            // TODO: change this to extend_one when it stabilizes
            buf.extend(iter::once(unsafe {
                ptr::read(buffer.buffer[cur_head].get() as *const _)
            }));
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

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        // SAFETY: we are the only ones accessing it apart from other end which should not drop the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };
        let rc = buffer.rc.fetch_sub(1, Ordering::SeqCst);

        if rc == 1 {
            // SAFETY: only dropped when buffer.rc == 0, so this is fine as other end shouldn't
            // attempt to drop it.
            unsafe {
                ptr::drop_in_place(self.buffer.as_ptr());
            }
        }
    }
}

pub struct SendFut<'sender, T> {
    sender: &'sender mut Sender<T>,
    to_send: Option<T>,
}

impl<'sender, T> SendFut<'sender, T> {
    #[inline]
    fn project(self: Pin<&mut Self>) -> (&mut Sender<T>, &mut Option<T>) {
        // SAFETY: we should NEVER move out any values
        let me = unsafe { self.get_unchecked_mut() };
        (&mut me.sender, &mut me.to_send)
    }
}

impl<'sender, T> Future for SendFut<'sender, T> {
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

pub struct RecvFut<'receiver, T> {
    receiver: &'receiver mut Receiver<T>,
}

impl<'receiver, T> RecvFut<'receiver, T> {
    #[inline]
    fn project(self: Pin<&mut Self>) -> &mut Receiver<T> {
        // SAFETY: we should NEVER move out any values
        let me = unsafe { self.get_unchecked_mut() };
        me.receiver
    }
}

impl<'receiver, T> Future for RecvFut<'receiver, T> {
    type Output = T;

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

pub struct BatchSendAllFut<'sender, T, I: Iterator<Item = T>> {
    sender: &'sender mut Sender<T>,
    iter: iter::Peekable<I>,
    total_count: usize,
}

impl<'sender, T, I: Iterator<Item = T>> BatchSendAllFut<'sender, T, I> {
    fn project(self: Pin<&mut Self>) -> (&mut Sender<T>, &mut iter::Peekable<I>, &mut usize) {
        // SAFETY: we should NEVER move out any values
        let me = unsafe { self.get_unchecked_mut() };
        (me.sender, &mut me.iter, &mut me.total_count)
    }
}

impl<'sender, T, I: Iterator<Item = T>> Future for BatchSendAllFut<'sender, T, I> {
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
unsafe impl<T: Send> Send for Sender<T> {}

// SAFETY: internal queue has atomic pointers to head and tails, and thus is safe to send
unsafe impl<T: Send> Send for Receiver<T> {}

// TODO: is it safe to send Sender & Receiver? possible because `Sender::send` and `Receiver::recv`
// take mutable reference to self, so exclusivity is guaranteed.
//
// TODO: async version of send and recv
//
// TODO: batched version of send and recv

#[cfg(test)]
mod test {
    use std::collections::VecDeque;

    #[cfg(not(loomer))]
    use std::{sync::Arc, thread};
    #[cfg(loomer)]
    use loom::{sync::Arc, thread};

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
