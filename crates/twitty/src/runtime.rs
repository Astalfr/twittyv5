//! Where backend futures run.
//!
//! GPUI has its own executor; network backends (reqwest) need tokio. Views send
//! backend work through [`Runtime::spawn`] and await the result from a GPUI
//! task. Tests use [`Runtime::inline`], which polls the future directly on the
//! awaiting executor so `run_until_parked` stays deterministic.

use std::{future::Future, pin::Pin, sync::Arc};

use gpui_kit::{App, Global};

type Boxed<T> = Pin<Box<dyn Future<Output = Result<T, TaskFailed>> + 'static>>;

/// Handle to the backend runtime. Cheap to clone.
#[derive(Debug, Clone)]
pub enum Runtime {
    /// A background multi-threaded tokio runtime (production).
    Tokio(Arc<tokio::runtime::Runtime>),
    /// Run futures on whoever awaits them (tests, runtime-agnostic backends).
    Inline,
}

impl Global for Runtime {}

/// The background task was cancelled or panicked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("background task failed: {0}")]
pub struct TaskFailed(String);

impl Runtime {
    /// Starts a background tokio runtime with two worker threads.
    pub fn tokio() -> std::io::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("twitty-net")
            .enable_all()
            .build()?;
        Ok(Self::Tokio(Arc::new(rt)))
    }

    /// A runtime that polls futures where they are awaited.
    pub fn inline() -> Self {
        Self::Inline
    }

    /// Runs `future` and returns a handle that any executor can await.
    pub fn spawn<F>(&self, future: F) -> Boxed<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        match self {
            Runtime::Tokio(rt) => {
                let handle = rt.spawn(future);
                Box::pin(async move { handle.await.map_err(|e| TaskFailed(e.to_string())) })
            }
            Runtime::Inline => Box::pin(async move { Ok(future.await) }),
        }
    }

    /// The runtime registered on the app.
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
    }

    #[test]
    fn tokio_result_is_awaitable_from_another_executor() {
        let rt = Runtime::tokio().unwrap();
        assert_eq!(block_on(rt.spawn(async { 40 + 2 })), Ok(42));
    }

    #[test]
    fn inline_runs_where_awaited() {
        assert_eq!(block_on(Runtime::inline().spawn(async { "ok" })), Ok("ok"));
    }

    #[test]
    fn tokio_panics_become_errors() {
        let rt = Runtime::tokio().unwrap();
        let out = block_on(rt.spawn(async {
            #[expect(clippy::panic, reason = "testing panic propagation")]
            {
                panic!("boom")
            }
        }));
        assert!(out.is_err());
    }
}
