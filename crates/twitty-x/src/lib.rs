//! Twitty's adapter for X's internal web GraphQL API.
//!
//! Layers (see `docs/x-api.md`):
//! - [`transport`]: authenticated HTTP, cookie rotation, rate limits, retries;
//! - [`catalog`] + [`discovery`]: which query id to call for each operation;
//! - [`XClient`]: GraphQL requests returning raw JSON;
//! - [`XBackend`]: the `twitty-model` `Backend` port over `XClient` + `translate`.
//!
//! - [`translate`]: X JSON → `twitty-model` (the only code that knows both);
//! - [`anonymise`]: scrubbing captures into committable test fixtures.
//!
//! X's JSON shapes stay inside this crate (`wire` is private); everything
//! public returns `twitty-model` types or raw `serde_json::Value`s for tooling.

#![forbid(unsafe_code)]

pub mod anonymise;
mod backend;
pub mod catalog;
mod client;
pub mod discovery;
mod error;
mod requests;
mod session;
pub mod translate;
pub mod transport;
mod wire;

pub use backend::XBackend;
pub use client::XClient;
pub use error::XError;
pub use session::{Secret, Session, SessionError};
