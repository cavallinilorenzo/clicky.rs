//! The `.clickypack` soundpack format shared by the Clicky app (reader) and its
//! build tooling (writer), plus the macOS keyboard geometry both of them need.

pub mod format;
pub mod keys;
pub mod mechvibes;

pub use format::{Clip, FormatError, KEY_SLOTS, Meta, Pack, PackInfo};
