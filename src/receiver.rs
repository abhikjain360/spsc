use crate::{
    atomic::Ordering,
    hint,
    queue::QueuePtr,
};

pub struct Receiver {
    ptr: QueuePtr,
    local_tail: usize,
    mask: usize,
}

impl Receiver {
    pub(crate) fn new(queue_ptr: QueuePtr, mask: usize) -> Self {
        Self {
            ptr: queue_ptr,
            local_tail: 0,
            mask,
        }
    }

    pub fn try_recv(&mut self) -> Option<usize> {
        let current_head = self.load_head();
        let new_head = (current_head + 1) & self.mask;

        if current_head == self.local_tail {
            self.load_tail();
            if self.local_tail == current_head {
                return None;
            }
        }

        // SAFETY: head != tail which means queue is not empty and head has valid initialised
        //         value
        let ret = unsafe { self.ptr.get(current_head) };
        self.store_head(new_head);

        Some(ret)
    }

    pub fn recv(&mut self) -> usize {
        let current_head = self.load_head();
        let new_head = (current_head + 1) & self.mask;

        while current_head == self.local_tail {
            hint::spin_loop();
            self.load_tail();
        }

        // SAFETY: head != tail which means queue is not empty and head has valid initialised
        //         value
        let ret = unsafe { self.ptr.get(current_head) };
        self.store_head(new_head);

        ret
    }

    #[cfg(feature = "async")]
    pub async fn recv_async(&mut self) -> usize {
        use std::task::Poll;

        futures::future::poll_fn(|ctx| {
            let current_head = self.load_head();

            if current_head == self.local_tail {
                self.load_tail();
                if current_head == self.local_tail {
                    self.ptr.register_receiver_waker(ctx.waker());
                    self.ptr.receiver_sleeping().store(true, Ordering::SeqCst);

                    // prevent lost wake
                    self.local_tail = self.ptr.tail().load(Ordering::SeqCst);
                    if current_head == self.local_tail {
                        return Poll::Pending;
                    }

                    // not sleeping anymore
                    self.ptr.receiver_sleeping().store(false, Ordering::Relaxed);
                }
            }

            let new_head = (current_head + 1) & self.mask;
            // SAFETY: head != tail which means queue is not empty and head has valid initialised
            //         value
            let ret = unsafe { self.ptr.get(current_head) };
            self.store_head(new_head);

            if self.ptr.sender_sleeping().load(Ordering::SeqCst) {
                self.ptr.sender_sleeping().store(false, Ordering::Relaxed);
                self.ptr.wake_sender();
            }
            Poll::Ready(ret)
        })
        .await
    }

    pub fn read_buffer(&mut self) -> &[usize] {
        let start = self.load_head();

        if start == self.local_tail {
            self.load_tail();
        }

        let end = if self.local_tail >= start {
            self.local_tail
        } else {
            self.capacity()
        };

        unsafe {
            let ptr = self.ptr.at(start);
            std::slice::from_raw_parts(ptr.as_ptr(), end - start)
        }
    }

    #[inline(always)]
    /// # Safety
    /// - should be called only after getting a read slice from `read_buffer`
    /// - `len` should be less than or equal to the length of slice given by `read_buffer`.
    pub unsafe fn advance(&mut self, len: usize) {
        // the len can be just right at the edge of buffer, so we need to wrap just in case
        let new_head = (self.load_head() + len) & self.mask;
        self.store_head(new_head);
    }

    #[inline(always)]
    fn capacity(&self) -> usize {
        self.ptr.head_capacity()
    }

    #[inline(always)]
    fn load_head(&self) -> usize {
        self.ptr.head().load(Ordering::Relaxed)
    }

    #[inline(always)]
    fn store_head(&self, value: usize) {
        self.ptr.head().store(value, Ordering::Release);
    }

    #[inline(always)]
    fn load_tail(&mut self) {
        self.local_tail = self.ptr.tail().load(Ordering::Acquire);
    }
}

unsafe impl Send for Receiver {}
