//! UI-independent application state. Everything here is plain Rust and unit
//! tested; GPUI views wrap these types without adding logic of their own.

pub mod add_column;
pub mod alerts;
pub mod column;
pub mod compose;
pub mod deck;
pub mod diagnostics;
pub mod image_scale;
pub mod interactions;
pub mod links;
pub mod login;
pub mod media_layout;
pub mod motion;
pub mod playback;
pub mod poll;
pub mod profile;
pub mod settings;
pub mod sources;
pub mod thread;
