#[repr(align(64))]
#[cfg_attr(all(target_arch = "aarch64", target_os = "macos"), repr(align(128)))]
pub(crate) struct Padded<T> {
    pub(crate) value: T,
}

impl<T> Padded<T> {
    pub(crate) fn new(value: T) -> Self {
        Self { value }
    }
}
