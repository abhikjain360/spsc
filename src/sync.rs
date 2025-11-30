//! Synchronization primitives abstraction.
//!
//! This module provides conditional imports to support both standard library
//! synchronization primitives and loom's testing primitives for concurrency verification.

#![allow(unused_imports)]

/// When using loom for concurrency testing, import loom's synchronization primitives.
#[cfg(loomer)]
pub(crate) use loom::{
    hint,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

/// When not using loom, import standard library synchronization primitives.
#[cfg(not(loomer))]
pub(crate) use std::{
    hint,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};
