#[cfg(loomer)]
pub(crate) use loom::{
    hint,
    sync::atomic::{AtomicUsize, Ordering},
};

#[cfg(not(loomer))]
pub(crate) use std::{
    hint,
    sync::atomic::{AtomicUsize, Ordering},
};
