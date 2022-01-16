#![allow(dead_code, unused_imports)]

use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    ptr::{self, NonNull},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

pub struct Queue<T> {
    buffer: Box<[UnsafeCell<MaybeUninit<T>>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    capacity: usize,
}

pub struct Sender<T> {
    buffer: NonNull<Queue<T>>,
}

pub struct Receiver<T> {
    buffer: NonNull<Queue<T>>,
}

impl<T> Queue<T> {
    pub fn with_capacity(capacity: usize) -> Self {
        let buffer = (0..capacity)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();
        Self {
            buffer,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            capacity,
        }
    }
}

impl<T> Sender<T> {
    pub fn send(&mut self, val: T) {
        let buffer = unsafe { self.buffer.as_mut() };

        let mut cur_tail = buffer.tail.load(Ordering::SeqCst);
        let mut new_tail = (cur_tail + 1) % buffer.capacity;
        // println!("s nt {} ct {}", new_tail, cur_tail);

        loop {
            let head = buffer.head.load(Ordering::SeqCst);
            // println!("s h {}", head);

            if head == new_tail {
                std::hint::spin_loop();
                continue;
            }

            buffer.buffer[cur_tail].get_mut().write(val);
            break;
        }

        loop {
            match buffer.tail.compare_exchange_weak(
                cur_tail,
                new_tail,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(actual_cur_tail) => cur_tail = actual_cur_tail,
            };

            new_tail = (cur_tail + 1) % buffer.capacity;
            std::hint::spin_loop();
        }
    }
}

impl<T> Receiver<T> {
    pub fn recv(&mut self) -> T {
        let buffer = unsafe { self.buffer.as_mut() };
        let mut cur_head = buffer.head.load(Ordering::SeqCst);
        let head = loop {
            let tail = buffer.tail.load(Ordering::SeqCst);
            if cur_head == tail {
                std::hint::spin_loop();
                continue;
            }

            let new_head = (cur_head + 1) % buffer.capacity;
            match buffer.head.compare_exchange_weak(
                cur_head,
                new_head,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break cur_head,
                Err(actual_cur_head) => cur_head = actual_cur_head,
            };

            std::hint::spin_loop();
        };
        unsafe { ptr::read(buffer.buffer[head].get() as *const _) }
    }
}

pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    let queue = Box::new(Queue::with_capacity(capacity));
    let queue_ptr = NonNull::new(Box::into_raw(queue)).unwrap();
    (Sender { buffer: queue_ptr }, Receiver { buffer: queue_ptr })
}

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Send for Receiver<T> {}

#[cfg(test)]
mod test {
    use std::thread;

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
}
