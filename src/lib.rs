use std::{
    cell::UnsafeCell,
    hint,
    mem::MaybeUninit,
    ptr::{self, NonNull},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

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

impl<T> Queue<T> {
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
            unsafe {
                ptr::drop_in_place((&mut *self.buffer[head].get()).as_mut_ptr());
            }
            head = (head + 1) % self.capacity;
        }
    }
}

impl<T> Sender<T> {
    pub fn send(&mut self, val: T) {
        let buffer = unsafe { self.buffer.as_mut() };

        let mut cur_tail = buffer.tail.load(Ordering::Relaxed);
        let mut new_tail = (cur_tail + 1) % buffer.capacity;

        while self.local_head == new_tail {
            self.local_head = buffer.head.load(Ordering::Relaxed);
            thread::yield_now();
        }

        buffer.buffer[cur_tail].get_mut().write(val);

        loop {
            match buffer.tail.compare_exchange_weak(
                cur_tail,
                new_tail,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual_cur_tail) => cur_tail = actual_cur_tail,
            };

            new_tail = (cur_tail + 1) % buffer.capacity;

            hint::spin_loop();
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let buffer = unsafe { self.buffer.as_ref() };
        let rc = buffer.rc.fetch_sub(1, Ordering::SeqCst);

        if rc == 1 {
            unsafe {
                ptr::drop_in_place(self.buffer.as_ptr());
            }
        }
    }
}

impl<T> Receiver<T> {
    pub fn recv(&mut self) -> T {
        let buffer = unsafe { self.buffer.as_mut() };

        let mut cur_head = buffer.head.load(Ordering::Relaxed);

        while cur_head == self.local_tail {
            self.local_tail = buffer.tail.load(Ordering::Relaxed);
            thread::yield_now();
        }

        let head = loop {
            let new_head = (cur_head + 1) % buffer.capacity;
            match buffer.head.compare_exchange_weak(
                cur_head,
                new_head,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => break cur_head,
                Err(actual_cur_head) => cur_head = actual_cur_head,
            };

            std::hint::spin_loop();
        };

        unsafe { ptr::read(buffer.buffer[head].get() as *const _) }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let buffer = unsafe { self.buffer.as_ref() };
        let rc = buffer.rc.fetch_sub(1, Ordering::SeqCst);

        if rc == 1 {
            unsafe {
                ptr::drop_in_place(self.buffer.as_ptr());
            }
        }
    }
}

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

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Send for Receiver<T> {}

#[cfg(test)]
mod test {
    use std::{sync::Arc, thread};

    use super::*;

    #[test]
    fn test_valid_sends() {
        let (mut tx, mut rx) = channel::<u32>(1024);

        thread::spawn(move || {
            for i in 0..1024 << 3 {
                tx.send(i);
            }
        });

        for i in 0..1024 << 3 {
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
}
