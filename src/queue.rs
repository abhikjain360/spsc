use std::{cell::UnsafeCell, mem::MaybeUninit, ptr};

use crate::sync::*;

/// The inner queue used by `Sender` and `Receiver`.
///
/// # Invariants
/// - head is valid value, unlesss head == tail,
/// - tail is invalid, where we add next value
pub(crate) struct Queue<T> {
    pub(crate) buffer: Box<[UnsafeCell<MaybeUninit<T>>]>,
    pub(crate) head: AtomicUsize,
    pub(crate) tail: AtomicUsize,
    pub(crate) rc: AtomicUsize,
    pub(crate) capacity: usize,
}

impl<T> Queue<T> {
    #[inline]
    pub(crate) fn with_capacity(capacity: usize) -> Self {
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
            head += 1;
            if head == self.capacity {
                head = 0;
            }
        }
    }
}
