#![allow(unused_imports)]

#[cfg(loomer)]
pub(crate) use loom::{
    hint,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

#[cfg(not(loomer))]
pub(crate) use std::{
    hint,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};
