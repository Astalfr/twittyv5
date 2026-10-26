//! UI-independent application state. Everything here is plain Rust and unit
//! tested; GPUI views wrap these types without adding logic of their own.

pub mod add_column;
pub mod alerts;
pub mod autoscroll;
pub mod column;
pub mod community_note;
pub mod compose;
pub mod deck;
pub mod diagnostics;
pub mod emoji;
pub mod filters;
pub mod follow;
pub mod image_scale;
pub mod interactions;
pub mod links;
pub mod login;
pub mod long_text;
pub mod media_layout;
pub mod motion;
pub mod playback;
pub mod poll;
pub mod profile;
pub mod repost;
pub mod settings;
pub mod sheets;
pub mod sources;
pub mod stats;
pub mod thread;
pub mod toasts;
pub mod translation;
pub mod update;
pub mod user_search;
