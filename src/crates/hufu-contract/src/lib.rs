//! The adapter contract of Hufu.
//!
//! `hufu-core` and `hufu-adapters` both depend on this crate and not on each other. It holds the
//! [`Adapter`] trait, the types that cross it, the normalized event model and the small set of
//! cross-platform process helpers both sides need.

pub mod event;
pub mod json;
pub mod proc;
pub mod time;
pub mod types;

pub use event::{Event, EventBody};
pub use types::*;
