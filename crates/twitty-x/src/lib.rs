//! Twitty's adapter for X's internal web GraphQL API.
//!
//! Layers (see `docs/x-api.md`):
//! - [`transport`]: authenticated HTTP, cookie rotation, rate limits, retries;
//! - [`catalog`] + [`discovery`]: which query id to call for each operation;
//! - [`XClient`]: GraphQL requests returning raw JSON.
//!
//! X's JSON never leaves this crate once translation lands (M4): the public
//! surface for the app will be `twitty_model::Backend`.

#![forbid(unsafe_code)]

pub mod catalog;
mod client;
pub mod discovery;
mod error;
mod session;
pub mod transport;

pub use client::XClient;
pub use error::XError;
pub use session::{Secret, Session, SessionError};
