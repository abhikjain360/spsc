#[cfg(feature = "async")]
use std::task::Waker;
use std::{
    mem::{align_of, offset_of, size_of},
    num::NonZeroUsize,
    ptr::NonNull,
};

#[cfg(feature = "async")]
use futures::task::AtomicWaker;

#[cfg(feature = "async")]
use crate::atomic::AtomicBool;
use crate::{
    alloc,
    atomic::{AtomicUsize, Ordering},
    padded::Padded,
};

struct CacheLine {
    shared: AtomicUsize,
    capacity: usize,
}

/// # Invariants
/// - tail should always point to the place where we can write next to.
// avoid re-ordering fields
#[repr(C)]
struct Queue {
    head: Padded<CacheLine>,
    #[cfg(feature = "async")]
    sender_sleeping: Padded<AtomicBool>,
    #[cfg(feature = "async")]
    receiver_waker: Padded<AtomicWaker>,

    tail: Padded<CacheLine>,
    #[cfg(feature = "async")]
    receiver_sleeping: Padded<AtomicBool>,
    #[cfg(feature = "async")]
    sender_waker: Padded<AtomicWaker>,

    rc: AtomicUsize,

    buffer: [usize; 0],
}

#[derive(Clone)]
pub(crate) struct QueuePtr {
    ptr: NonNull<Queue>,
}

macro_rules! _field {
    ($ptr:expr, $($path:tt).+) => {
        $ptr.byte_add(offset_of!(Queue, $($path).+))
    };

    ($ptr:expr, $($path:tt).+, $ty:ty) => {
        $ptr.byte_add(offset_of!(Queue, $($path).+)).cast::<$ty>()
    };
}

impl QueuePtr {
    pub(crate) fn with_capacity(capacity: NonZeroUsize) -> (Self, usize) {
        let capacity = capacity.saturating_add(1).get().next_power_of_two();

        let (layout, _offset) = Self::layout(capacity);

        // SAFETY: capacity > 0, so layout is non-zero too
        let ptr = unsafe { alloc::alloc(layout) } as *mut Queue;
        let Some(ptr) = NonNull::new(ptr) else {
            std::alloc::handle_alloc_error(layout);
        };

        let mask = capacity - 1;

        unsafe {
            ptr.write(Queue {
                head: Padded::new(CacheLine {
                    shared: AtomicUsize::new(0),
                    capacity,
                }),
                tail: Padded::new(CacheLine {
                    shared: AtomicUsize::new(0),
                    capacity,
                }),

                #[cfg(feature = "async")]
                sender_sleeping: Padded::new(AtomicBool::new(false)),

                #[cfg(feature = "async")]
                receiver_sleeping: Padded::new(AtomicBool::new(false)),

                #[cfg(feature = "async")]
                sender_waker: Padded::new(AtomicWaker::new()),

                #[cfg(feature = "async")]
                receiver_waker: Padded::new(AtomicWaker::new()),

                rc: AtomicUsize::new(2),

                buffer: [],
            });
        };

        (Self { ptr }, mask)
    }

    fn layout(capacity: usize) -> (alloc::Layout, usize) {
        let header_layout =
            alloc::Layout::from_size_align(size_of::<Queue>(), align_of::<Queue>()).unwrap();
        let buffer_layout = alloc::Layout::array::<usize>(capacity).unwrap();
        header_layout.extend(buffer_layout).unwrap()
    }

    #[inline(always)]
    pub(crate) fn head(&self) -> &AtomicUsize {
        unsafe { _field!(self.ptr, head.value.shared, AtomicUsize).as_ref() }
    }

    #[inline(always)]
    pub(crate) fn tail(&self) -> &AtomicUsize {
        unsafe { _field!(self.ptr, tail.value.shared, AtomicUsize).as_ref() }
    }

    #[inline(always)]
    pub(crate) unsafe fn at(&self, index: usize) -> NonNull<usize> {
        unsafe { _field!(self.ptr, buffer, usize).add(index) }
    }

    #[inline(always)]
    pub(crate) unsafe fn get(&self, index: usize) -> usize {
        unsafe { self.at(index).read() }
    }

    #[inline(always)]
    pub(crate) fn set(&self, index: usize, value: usize) {
        unsafe { self.at(index).write(value) }
    }

    #[inline(always)]
    pub(crate) fn head_capacity(&self) -> usize {
        unsafe { _field!(self.ptr, head.value.capacity, usize).read() }
    }

    #[inline(always)]
    pub(crate) fn tail_capacity(&self) -> usize {
        unsafe { _field!(self.ptr, tail.value.capacity, usize).read() }
    }
}

#[cfg(feature = "async")]
impl QueuePtr {
    #[inline(always)]
    pub(crate) fn register_sender_waker(&self, waker: &Waker) {
        unsafe {
            _field!(self.ptr, sender_waker.value, AtomicWaker)
                .as_ref()
                .register(waker);
        }
    }

    #[inline(always)]
    pub(crate) fn register_receiver_waker(&self, waker: &Waker) {
        unsafe {
            _field!(self.ptr, receiver_waker.value, AtomicWaker)
                .as_ref()
                .register(waker);
        }
    }

    #[inline(always)]
    pub(crate) fn wake_sender(&self) {
        unsafe {
            _field!(self.ptr, sender_waker.value, AtomicWaker)
                .as_ref()
                .wake();
        }
    }

    #[inline(always)]
    pub(crate) fn wake_receiver(&self) {
        unsafe {
            _field!(self.ptr, receiver_waker.value, AtomicWaker)
                .as_ref()
                .wake();
        }
    }

    #[inline(always)]
    pub(crate) fn sender_sleeping(&self) -> &AtomicBool {
        unsafe { _field!(self.ptr, sender_sleeping.value, AtomicBool).as_ref() }
    }

    #[inline(always)]
    pub(crate) fn receiver_sleeping(&self) -> &AtomicBool {
        unsafe { _field!(self.ptr, receiver_sleeping.value, AtomicBool).as_ref() }
    }
}

impl Drop for QueuePtr {
    fn drop(&mut self) {
        let rc = unsafe { _field!(self.ptr, rc, AtomicUsize).as_ref() };
        if rc.fetch_sub(1, Ordering::AcqRel) == 1 {
            let capacity = self.head_capacity();
            let (layout, _) = Self::layout(capacity);
            unsafe {
                self.ptr.drop_in_place();
                alloc::dealloc(self.ptr.cast().as_ptr(), layout);
            }
        }
    }
}
