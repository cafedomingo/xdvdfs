#![no_std]
// async_trait 0.1.88 puts a bare #[must_use] on the trait methods it generates
#![allow(clippy::double_must_use)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

macro_rules! traceln {
    ($($x:expr),*) => {
        #[cfg(all(feature = "std", feature = "logging"))]
        log::trace!($($x),*);
    };
}

macro_rules! debugln {
    ($($x:expr),*) => {
        #[cfg(all(feature = "std", feature = "logging"))]
        log::debug!($($x),*);
    };
}

pub mod blockdev;
pub mod layout;

#[cfg(feature = "read")]
pub mod read;

#[cfg(feature = "write")]
pub mod write;

#[cfg(all(feature = "checksum", feature = "std"))]
pub mod checksum;
