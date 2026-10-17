//! CADCraft terrain modelling: Delaunay triangulation (TIN), elevation queries, and contour-line
//! extraction.
//!
//! **Scope of this first increment**: triangulation, point-in-triangle elevation queries, and
//! per-triangle contour segments. Chaining those segments into continuous polylines, cubatures
//! (cut/fill volumes) and long/cross profiles are later increments - not pretended-complete here.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod contour;
pub mod tin;

pub use contour::{ContourSegment, contours};
pub use tin::{TerrainError, Tin, triangulate};
