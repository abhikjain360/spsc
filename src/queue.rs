//! Internal queue implementation for the SPSC channel.
//!
//! This module contains the low-level ring buffer implementation that powers
//! both the synchronous and asynchronous channel operations.

#[cfg(not(loomer))]
use std::cell::UnsafeCell;
use std::{
    alloc::{Layout, alloc, dealloc, handle_alloc_error},
    mem::{MaybeUninit, size_of},
    ptr,
};

#[cfg(loomer)]
use loom::cell::UnsafeCell;

use crate::{QueueValue, sync::*};

/// Type alias for buffer cells, conditionally using loom's UnsafeCell for testing.
#[cfg(not(loomer))]
pub(crate) type BufferCell = UnsafeCell<MaybeUninit<QueueValue>>;

/// Type alias for buffer cells when using loom for concurrency testing.
#[cfg(loomer)]
pub(crate) type BufferCell = loom::cell::UnsafeCell<MaybeUninit<QueueValue>>;

/// Cache line size for x86_64 architectures (64 bytes).
#[cfg(target_arch = "x86_64")]
const CACHE_LINE_SIZE: usize = 64;

/// Cache line size for aarch64 architectures (128 bytes).
#[cfg(target_arch = "aarch64")]
const CACHE_LINE_SIZE: usize = 128;

/// The internal lock-free ring buffer used by `Sender` and `Receiver`.
///
/// This structure is carefully aligned to cache line boundaries to prevent false sharing
/// between the producer and consumer. The head and tail indices are kept on separate
/// cache lines to maximize performance.
///
/// # Layout
///
/// ```text
/// ┌──────────────┐
/// │ head         │  <- Consumer's read position (cache line aligned)
/// ├──────────────┤
/// │ padding      │
/// ├──────────────┤
/// │ tail         │  <- Producer's write position (cache line aligned)
/// ├──────────────┤
/// │ padding      │
/// ├──────────────┤
/// │ rc           │  <- Reference count
/// │ capacity     │  <- Buffer capacity
/// ├──────────────┤
/// │ padding      │
/// ├──────────────┤
/// │ buffer[...]  │  <- Actual data storage
/// └──────────────┘
/// ```
///
/// # Invariants
///
/// - `head` points to the next valid value to read (unless `head == tail`)
/// - `tail` points to the next position where a value should be written
/// - When `head == tail`, the queue is empty
/// - When `(tail + 1) % capacity == head`, the queue is full
/// - The actual capacity is `capacity - 1` to distinguish empty from full
#[cfg_attr(target_arch = "aarch64", repr(C, align(128)))]
#[cfg_attr(target_arch = "x86_64", repr(C, align(64)))]
pub(crate) struct Queue {
    /// The read position (consumer side). Always on its own cache line to prevent false sharing.
    pub(crate) head: AtomicUsize,
    /// Padding to ensure head is on its own cache line.
    _pad1: [u8; CACHE_LINE_SIZE - size_of::<AtomicUsize>()],
    
    /// The write position (producer side). Always on its own cache line to prevent false sharing.
    pub(crate) tail: AtomicUsize,
    /// Padding to ensure tail is on its own cache line.
    _pad2: [u8; CACHE_LINE_SIZE - size_of::<AtomicUsize>()],
    
    /// Reference count for safe deallocation (2 when both sender and receiver exist).
    pub(crate) rc: AtomicUsize,
    /// The total capacity of the buffer (actual usable capacity is capacity - 1).
    pub(crate) capacity: usize,
    /// Padding to separate metadata from data.
    _pad3: [u8; CACHE_LINE_SIZE - size_of::<AtomicUsize>() - size_of::<usize>()],
    
    /// Zero-sized array that marks the start of the dynamically-sized buffer.
    /// The actual buffer is allocated immediately after this structure.
    pub(crate) buffer: [BufferCell; 0],
}

impl Queue {
    /// Creates a new queue with the specified capacity.
    ///
    /// The queue is heap-allocated with a single allocation that includes both
    /// the header (head, tail, rc, capacity) and the buffer array.
    ///
    /// # Arguments
    ///
    /// * `capacity` - The requested capacity. The actual internal capacity will be
    ///   `capacity + 1` to distinguish between empty and full states.
    ///
    /// # Returns
    ///
    /// A non-null pointer to the allocated queue structure.
    ///
    /// # Panics
    ///
    /// Panics if memory allocation fails.
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
            let buffer_ptr = ptr::addr_of_mut!((*ptr).buffer) as *mut BufferCell;
            for i in 0..capacity {
                buffer_ptr
                    .add(i)
                    .write(UnsafeCell::new(MaybeUninit::uninit()));
            }

            ptr::NonNull::new_unchecked(ptr)
        }
    }

    /// Calculates the memory layout for a queue with the given capacity.
    ///
    /// This includes both the queue header and the buffer array.
    ///
    /// # Arguments
    ///
    /// * `capacity` - The buffer capacity (not the usable capacity).
    ///
    /// # Returns
    ///
    /// The memory layout required for the queue allocation.
    #[inline]
    pub(crate) fn layout(capacity: usize) -> Layout {
        let header = Layout::new::<Self>();
        let array = Layout::array::<BufferCell>(capacity).unwrap();
        header.extend(array).unwrap().0.pad_to_align()
    }

    /// Returns a pointer to the buffer element at the given index.
    ///
    /// # Safety
    ///
    /// - `ptr` must be a valid pointer to a Queue
    /// - `idx` must be less than the queue's capacity
    ///
    /// # Arguments
    ///
    /// * `ptr` - Pointer to the queue
    /// * `idx` - Index into the buffer
    ///
    /// # Returns
    ///
    /// A const pointer to the buffer cell at the given index.
    #[inline]
    pub(crate) unsafe fn elem(ptr: *mut Self, idx: usize) -> *const BufferCell {
        unsafe {
            let ptr = (ptr as *mut u8).add(std::mem::size_of::<Self>()) as *const BufferCell;
            ptr.add(idx)
        }
    }

    /// Returns a mutable pointer to the buffer element at the given index (loom only).
    ///
    /// This variant is only used when testing with loom for concurrency checking.
    ///
    /// # Safety
    ///
    /// - `ptr` must be a valid pointer to a Queue
    /// - `idx` must be less than the queue's capacity
    ///
    /// # Arguments
    ///
    /// * `ptr` - Pointer to the queue
    /// * `idx` - Index into the buffer
    ///
    /// # Returns
    ///
    /// A mutable pointer to the buffer cell at the given index.
    #[cfg(loomer)]
    #[inline]
    pub(crate) unsafe fn elem_mut(ptr: *mut Self, idx: usize) -> *mut BufferCell {
        unsafe {
            let ptr = (ptr as *mut u8).add(std::mem::size_of::<Self>()) as *mut BufferCell;
            ptr.add(idx)
        }
    }

    /// Frees the queue and all remaining elements.
    ///
    /// This method drops all elements still in the queue and deallocates the memory.
    ///
    /// # Safety
    ///
    /// - Must only be called when the reference count has reached 0
    /// - The caller must ensure exclusive access to the queue
    /// - This is enforced by checking `rc` before calling `free`
    ///
    /// # Arguments
    ///
    /// * `ptr` - Non-null pointer to the queue to be freed
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
                ptr::drop_in_place((*Self::elem(ptr.as_ptr(), head)).get().cast::<QueueValue>());
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
