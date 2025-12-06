use crate::{
    atomic::Ordering,
    hint,
    queue::QueuePtr,
};

pub struct Sender {
    ptr: QueuePtr,
    local_head: usize,
    mask: usize,
}

impl Sender {
    pub(crate) fn new(queue_ptr: QueuePtr, mask: usize) -> Self {
        Self {
            ptr: queue_ptr,
            local_head: 0,
            mask,
        }
    }

    pub fn try_send(&mut self, value: usize) -> bool {
        let current_tail = self.load_tail();
        let new_tail = (current_tail + 1) & self.mask;

        if new_tail == self.local_head {
            self.load_head();
            if new_tail == self.local_head {
                return false;
            }
        }

        self.ptr.set(current_tail, value);
        self.store_tail(new_tail);

        true
    }

    pub fn send(&mut self, value: usize) {
        let current_tail = self.load_tail();
        let new_tail = (current_tail + 1) & self.mask;

        while new_tail == self.local_head {
            hint::spin_loop();
            self.load_head();
        }

        self.ptr.set(current_tail, value);
        self.store_tail(new_tail);
    }

    #[cfg(feature = "async")]
    pub async fn send_async(&mut self, value: usize) {
        use std::task::Poll;

        futures::future::poll_fn(|ctx| {
            let current_tail = self.load_tail();
            let new_tail = (current_tail + 1) & self.mask;

            if new_tail == self.local_head {
                self.load_head();
                if new_tail == self.local_head {
                    self.ptr.register_sender_waker(ctx.waker());
                    self.ptr.sender_sleeping().store(true, Ordering::SeqCst);

                    // prevent lost wake
                    self.local_head = self.ptr.head().load(Ordering::SeqCst);
                    if new_tail == self.local_head {
                        return Poll::Pending;
                    }

                    // not sleeping anymore
                    self.ptr.sender_sleeping().store(false, Ordering::Relaxed);
                }
            }

            self.ptr.set(current_tail, value);
            self.store_tail(new_tail);

            if self.ptr.receiver_sleeping().load(Ordering::SeqCst) {
                self.ptr.receiver_sleeping().store(false, Ordering::Relaxed);
                self.ptr.wake_receiver();
            }
            Poll::Ready(())
        })
        .await
    }

    pub fn write_buffer(&mut self) -> &mut [usize] {
        let start = self.load_tail();
        let next = (start + 1) & self.mask;

        if next == self.local_head {
            self.load_head();
        }

        let end = if self.local_head > start {
            self.local_head - 1
        } else if self.local_head == 0 {
            self.capacity() - 1
        } else {
            self.capacity()
        };

        unsafe {
            let ptr = self.ptr.at(start);
            std::slice::from_raw_parts_mut(ptr.as_ptr(), end - start)
        }
    }

    /// # Safety
    /// - should be called only after getting a write slice from `write_buffer`
    /// - `len` should be less than or equal to the length of slice given by `write_buffer`.
    #[inline(always)]
    pub unsafe fn commit(&mut self, len: usize) {
        // the len can be just right at the edge of buffer, so we need to wrap just in case
        let new_tail = (self.load_tail() + len) & self.mask;
        self.store_tail(new_tail);
    }

    #[inline(always)]
    fn capacity(&self) -> usize {
        self.ptr.tail_capacity()
    }

    #[inline(always)]
    fn load_tail(&self) -> usize {
        self.ptr.tail().load(Ordering::Relaxed)
    }

    #[inline(always)]
    fn store_tail(&self, value: usize) {
        self.ptr.tail().store(value, Ordering::Release);
    }

    #[inline(always)]
    fn load_head(&mut self) {
        self.local_head = self.ptr.head().load(Ordering::Acquire);
    }
}

unsafe impl Send for Sender {}
