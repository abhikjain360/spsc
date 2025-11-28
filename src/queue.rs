#[cfg(not(loomer))]
use std::cell::UnsafeCell;
use std::{
    alloc::{Layout, alloc, dealloc, handle_alloc_error},
    mem::MaybeUninit,
    ptr,
};

#[cfg(loomer)]
use loom::cell::UnsafeCell;

use crate::sync::*;

#[cfg(not(loomer))]
pub(crate) type BufferCell<T> = UnsafeCell<MaybeUninit<T>>;

#[cfg(loomer)]
pub(crate) type BufferCell<T> = loom::cell::UnsafeCell<MaybeUninit<T>>;

#[cfg(target_arch = "x86_64")]
const CACHE_LINE_SIZE: usize = 64;
#[cfg(target_arch = "aarch64")]
const CACHE_LINE_SIZE: usize = 128;

/// The inner queue used by `Sender` and `Receiver`.
///
/// # Invariants
/// - head is valid value, unlesss head == tail,
/// - tail is invalid, where we add next value
#[cfg_attr(target_arch = "aarch64", repr(C, align(128)))]
#[cfg_attr(target_arch = "x86_64", repr(C, align(64)))]
pub(crate) struct Queue<T> {
    pub(crate) head: AtomicUsize,
    _pad1: [u8; CACHE_LINE_SIZE - size_of::<AtomicUsize>()],
    pub(crate) tail: AtomicUsize,
    _pad2: [u8; CACHE_LINE_SIZE - size_of::<AtomicUsize>()],
    pub(crate) rc: AtomicUsize,
    pub(crate) capacity: usize,
    _pad3: [u8; CACHE_LINE_SIZE - size_of::<AtomicUsize>() - size_of::<usize>()],
    pub(crate) buffer: [BufferCell<T>; 0],
}

impl<T> Queue<T> {
    #[inline]
    pub(crate) fn with_capacity(capacity: usize) -> ptr::NonNull<Self> {
        let capacity = capacity + 1;
        let layout = Self::layout(capacity);

        unsafe {
            let ptr = alloc(layout) as *mut Self;
            if ptr.is_null() {
                handle_alloc_error(layout);
            }

            ptr::addr_of_mut!((*ptr).head).write(AtomicUsize::new(0));
            ptr::addr_of_mut!((*ptr).tail).write(AtomicUsize::new(0));
            ptr::addr_of_mut!((*ptr).rc).write(AtomicUsize::new(0));
            ptr::addr_of_mut!((*ptr).capacity).write(capacity);

            // Initialize buffer elements
            let buffer_ptr = ptr::addr_of_mut!((*ptr).buffer) as *mut BufferCell<T>;
            for i in 0..capacity {
                buffer_ptr
                    .add(i)
                    .write(UnsafeCell::new(MaybeUninit::uninit()));
            }

            ptr::NonNull::new_unchecked(ptr)
        }
    }

    #[inline]
    pub(crate) fn layout(capacity: usize) -> Layout {
        let header = Layout::new::<Self>();
        let array = Layout::array::<BufferCell<T>>(capacity).unwrap();
        header.extend(array).unwrap().0.pad_to_align()
    }

    #[inline]
    pub(crate) unsafe fn elem(ptr: *mut Self, idx: usize) -> *const BufferCell<T> {
        unsafe {
            let ptr = (ptr as *mut u8).add(std::mem::size_of::<Self>()) as *const BufferCell<T>;
            ptr.add(idx)
        }
    }

    #[cfg(loomer)]
    #[inline]
    pub(crate) unsafe fn elem_mut(ptr: *mut Self, idx: usize) -> *mut BufferCell<T> {
        unsafe {
            let ptr = (ptr as *mut u8).add(std::mem::size_of::<Self>()) as *mut BufferCell<T>;
            ptr.add(idx)
        }
    }

    // SAFETY: we verify rc before calling free, so we have exclusive access for dealloc
    pub(crate) unsafe fn free(mut ptr: ptr::NonNull<Self>) {
        let queue = unsafe { ptr.as_mut() };
        let capacity = queue.capacity;
        let layout = Self::layout(capacity);

        let mut head = queue.head.load(Ordering::SeqCst);
        let tail = queue.tail.load(Ordering::SeqCst);

        while head != tail {
            // SAFETY: we own all the existing values in the queue.
            #[cfg(not(loomer))]
            unsafe {
                ptr::drop_in_place((*Self::elem(ptr.as_ptr(), head)).get().cast::<T>());
            }
            #[cfg(loomer)]
            unsafe {
                ptr::drop_in_place(
                    (*Self::elem_mut(ptr.as_ptr(), head))
                        .get_mut()
                        .deref()
                        .as_mut_ptr(),
                );
            }
            head += 1;
            if head == capacity {
                head = 0;
            }
        }

        #[cfg(loomer)]
        {
            let buffer_ptr = queue.buffer.as_mut_ptr();
            for i in 0..capacity {
                unsafe {
                    ptr::drop_in_place(buffer_ptr.add(i));
                }
            }
        }

        unsafe { dealloc(ptr.as_ptr() as *mut u8, layout) };
    }
}
