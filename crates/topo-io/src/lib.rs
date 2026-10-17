//! CADCraft topography file import: total-station carnets and (incrementally) the coordinate
//! reference system registry.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod gsi;

pub use gsi::{GsiImport, GsiPoint};
