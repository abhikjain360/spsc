use std::{
    iter,
    pin::Pin,
    ptr::NonNull,
    task::{Context, Poll},
};

use futures::Future;

use crate::{Queue, sync::*};

pub struct Sender<T> {
    buffer: NonNull<Queue<T>>,
    local_head: usize,
}

impl<T> Sender<T> {
    pub(crate) fn new(buffer: NonNull<Queue<T>>) -> Self {
        Self {
            buffer,
            local_head: 0,
        }
    }

    pub fn try_send(&mut self, val: T) -> Result<(), T> {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };

        let cur_tail = buffer.tail.load(Ordering::Relaxed);
        let mut new_tail = cur_tail + 1;
        if new_tail == buffer.capacity {
            new_tail = 0;
        }

        if self.local_head == new_tail {
            self.local_head = buffer.head.load(Ordering::Acquire);

            if self.local_head == new_tail {
                return Err(val);
            }
        }

        #[cfg(not(loomer))]
        unsafe {
            ((*Queue::elem(self.buffer.as_ptr(), cur_tail)).get() as *mut T).write(val);
        }

        #[cfg(loomer)]
        unsafe {
            (*Queue::elem(self.buffer.as_ptr(), cur_tail))
                .with_mut(|ptr| (ptr as *mut T).write(val));
        }

        // store is fine as we are the only ones writing to tail
        buffer.tail.store(new_tail, Ordering::Release);

        Ok(())
    }

    pub fn send(&mut self, val: T) {
        // SAFETY: we are the only ones accessing it apart from other end which will not remove the
        // buffer, so its safe to derefernce.
        let buffer = unsafe { self.buffer.as_ref() };

        let cur_tail = buffer.tail.load(Ordering::Relaxed);
        let mut new_tail = cur_tail + 1;
        if new_tail == buffer.capacity {
            new_tail = 0;
        }

        if self.local_head == new_tail {
            self.local_head = buffer.head.load(Ordering::Acquire);

            let mut spin_count = 0;
            while self.local_head == new_tail {
                if spin_count > 100 {
                    thread::yield_now();
                    spin_count = 0;
                } else {
                    hint::spin_loop();
                    spin_count += 1;
                }
                self.local_head = buffer.head.load(Ordering::Acquire);
            }
        }

        #[cfg(not(loomer))]
        unsafe {
            ((*Queue::elem(self.buffer.as_ptr(), cur_tail)).get() as *mut T).write(val);
        }

        #[cfg(loomer)]
        unsafe {
            (*Queue::elem(self.buffer.as_ptr(), cur_tail))
                .with_mut(|ptr| (ptr as *mut T).write(val));
        }

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
        let buffer = unsafe { self.buffer.as_ref() };
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

            #[cfg(not(loomer))]
            unsafe {
                ((*Queue::elem(self.buffer.as_ptr(), cur_tail)).get() as *mut T).write(val);
            }

            #[cfg(loomer)]
            unsafe {
                (*Queue::elem(self.buffer.as_ptr(), cur_tail))
                    .with_mut(|ptr| (ptr as *mut T).write(val));
            }

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
        let buffer = unsafe { self.buffer.as_ref() };

        let mut cur_tail = buffer.tail.load(Ordering::Relaxed);
        let mut new_tail = cur_tail + 1;
        if new_tail == buffer.capacity {
            new_tail = 0;
        }

        for val in vals {
            if self.local_head == new_tail {
                buffer.tail.store(cur_tail, Ordering::Release);

                self.local_head = buffer.head.load(Ordering::Acquire);

                let mut spin_count = 0;
                while self.local_head == new_tail {
                    if spin_count > 100 {
                        thread::yield_now();
                        spin_count = 0;
                    } else {
                        hint::spin_loop();
                        spin_count += 1;
                    }

                    self.local_head = buffer.head.load(Ordering::Acquire);
                }
            }

            #[cfg(not(loomer))]
            unsafe {
                ((*Queue::elem(self.buffer.as_ptr(), cur_tail)).get() as *mut T).write(val);
            }

            #[cfg(loomer)]
            unsafe {
                (*Queue::elem(self.buffer.as_ptr(), cur_tail))
                    .with_mut(|ptr| (ptr as *mut T).write(val));
            }

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
                Queue::free(self.buffer);
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
        (me.sender, &mut me.to_send)
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
