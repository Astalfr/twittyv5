//! Signing in on x.com in a window, and reading the session cookies from it.
//!
//! The window is a WebView2 page in a child process (`twitty --login-helper`),
//! because its tao event loop can't share the main thread with GPUI. The helper
//! writes one [`Reply`] line to its stdout, which the app reads through
//! [`HelperProcess`]. The cookies are never written to disk or logged on the
//! way: the page runs InPrivate, and the reply only goes through the pipe.

use std::{
    fmt,
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Mutex, PoisonError},
};

use serde::{Deserialize, Serialize};

use crate::account::Credentials;

/// The argument that makes `twitty.exe` run the sign-in window instead of the app.
pub const HELPER_FLAG: &str = "--login-helper";

/// Why the sign-in window couldn't produce a session. Carries no secret data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrowserError {
    #[error("Couldn't open the sign-in window ({0})")]
    Launch(String),
    #[error("The sign-in window failed ({0})")]
    Helper(String),
}

/// Signs in on x.com in a window.
pub trait BrowserSignIn: Send + Sync {
    /// Opens the window and blocks until it closes: `Ok(None)` when the user
    /// closed it (or [`cancel`](Self::cancel) did) without signing in.
    fn sign_in(&self) -> Result<Option<Credentials>, BrowserError>;
    /// Closes the window of a running [`sign_in`](Self::sign_in).
    fn cancel(&self);
}

/// What the helper tells the app, as one JSON line on its stdout. Not
/// persisted: both ends are the same executable. `Debug` never shows the values.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(crate) enum Reply {
    SignedIn { auth_token: String, ct0: String },
    Closed,
    Failed { reason: String },
}

impl fmt::Debug for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reply::SignedIn { .. } => f.write_str("SignedIn { auth_token: ***, ct0: *** }"),
            Reply::Closed => f.write_str("Closed"),
            Reply::Failed { reason } => write!(f, "Failed({reason:?})"),
        }
    }
}

impl From<Credentials> for Reply {
    fn from(c: Credentials) -> Self {
        Reply::SignedIn { auth_token: c.auth_token, ct0: c.ct0 }
    }
}

/// Writes `reply` as one line and flushes it.
pub(crate) fn write_reply(mut out: impl Write, reply: &Reply) -> io::Result<()> {
    serde_json::to_writer(&mut out, reply)?;
    out.write_all(b"\n")?;
    out.flush()
}

/// Reads the helper's single reply line. Never waits for the end of the
/// stream: WebView2's own processes may keep the pipe open after the helper is
/// done. Errors never quote the line, which may hold the cookies.
pub fn read_reply(mut reader: impl BufRead) -> Result<Option<Credentials>, BrowserError> {
    let mut line = String::new();
    let read = reader.read_line(&mut line).map_err(|e| BrowserError::Helper(e.to_string()))?;
    if read == 0 {
        return Err(BrowserError::Helper("it stopped without an answer".into()));
    }
    match serde_json::from_str(&line) {
        Ok(Reply::SignedIn { auth_token, ct0 }) => Ok(Some(Credentials { auth_token, ct0 })),
        Ok(Reply::Closed) => Ok(None),
        Ok(Reply::Failed { reason }) => Err(BrowserError::Helper(reason)),
        Err(_) => Err(BrowserError::Helper("its answer was unreadable".into())),
    }
}

/// Runs the helper as a child of this executable.
#[derive(Debug)]
pub struct HelperProcess {
    exe: PathBuf,
    running: Mutex<Option<Child>>,
}

impl HelperProcess {
    pub fn new(exe: PathBuf) -> Self {
        Self { exe, running: Mutex::new(None) }
    }

    fn running(&self) -> std::sync::MutexGuard<'_, Option<Child>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl BrowserSignIn for HelperProcess {
    fn sign_in(&self) -> Result<Option<Credentials>, BrowserError> {
        let mut child = Command::new(&self.exe)
            .arg(HELPER_FLAG)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| BrowserError::Launch(e.to_string()))?;
        let Some(stdout) = child.stdout.take() else {
            // Can't happen with `Stdio::piped`; don't leave the window behind.
            child.kill().ok();
            return Err(BrowserError::Launch("no pipe to the window".into()));
        };
        let id = child.id();
        *self.running() = Some(child);
        let reply = read_reply(BufReader::new(stdout));
        // `cancel` takes the child when it kills it; a later sign-in may have
        // put its own there since. The helper exits by itself after replying,
        // so the child is dropped without waiting for it.
        match self.running().take_if(|child| child.id() == id) {
            Some(_) => reply,
            None => Ok(None),
        }
    }

    fn cancel(&self) {
        if let Some(mut child) = self.running().take() {
            // Not waited for: this runs on the UI thread.
            if let Err(e) = child.kill() {
                tracing::warn!("could not close the sign-in window: {e}");
            }
        }
    }
}

/// The sign-in window: `twitty --login-helper`. Replies on stdout, then exits.
#[cfg(windows)]
pub fn run_helper() -> anyhow::Result<()> {
    use std::time::{Duration, Instant};

    use tao::{
        dpi::LogicalSize,
        event::{Event, StartCause, WindowEvent},
        event_loop::{ControlFlow, EventLoop},
        window::WindowBuilder,
    };
    use wry::{WebContext, WebViewBuilder};

    use crate::state::login::CookieWatch;

    const LOGIN_URL: &str = "https://x.com/i/flow/login";
    const COOKIE_URL: &str = "https://x.com";
    /// How often the cookies are read (x.com is a single-page app, so page
    /// loads don't say when the sign-in is done).
    const POLL: Duration = Duration::from_millis(500);

    fn reply(reply: &Reply) {
        // Nobody to tell if the app is gone; the helper exits either way.
        write_reply(io::stdout().lock(), reply).ok();
    }

    // WebView2's profile folder defaults to one next to the exe, which isn't
    // writable under Program Files. InPrivate keeps the cookies out of it.
    let data_dir = directories::ProjectDirs::from("", "", "Twitty")
        .map_or_else(std::env::temp_dir, |d| d.cache_dir().to_path_buf())
        .join("webview2");
    let mut context = WebContext::new(Some(data_dir));
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Sign in to Twitter — Twitty")
        .with_inner_size(LogicalSize::new(520.0, 760.0))
        .build(&event_loop);
    let window = match window {
        Ok(window) => window,
        Err(e) => {
            reply(&Reply::Failed { reason: format!("no window: {e}") });
            return Ok(());
        }
    };
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(LOGIN_URL)
        .with_incognito(true)
        .build(&window);
    let webview = match webview {
        Ok(webview) => webview,
        Err(e) => {
            reply(&Reply::Failed { reason: format!("WebView2 is unavailable: {e}") });
            return Ok(());
        }
    };

    let mut watch = CookieWatch::default();
    let mut done = false;
    event_loop.run(move |event, _, control_flow| {
        // Both must live as long as the loop.
        let _ = (&window, &context);
        if done {
            *control_flow = ControlFlow::Exit;
            return;
        }
        match event {
            Event::NewEvents(StartCause::Init | StartCause::ResumeTimeReached { .. }) => {
                *control_flow = ControlFlow::WaitUntil(Instant::now() + POLL);
                // Not from a wry callback: reading cookies pumps messages.
                let cookies = webview.cookies_for_url(COOKIE_URL).unwrap_or_default();
                let value = |name: &str| {
                    cookies.iter().find(|c| c.name() == name).map(|c| c.value().to_owned())
                };
                let (auth_token, ct0) = (value("auth_token"), value("ct0"));
                if let Some(credentials) = watch.observe(auth_token.as_deref(), ct0.as_deref()) {
                    reply(&Reply::from(credentials));
                    done = true;
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                reply(&Reply::Closed);
                done = true;
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    })
}

/// The sign-in window needs WebView2.
#[cfg(not(windows))]
pub fn run_helper() -> anyhow::Result<()> {
    write_reply(io::stdout().lock(), &Reply::Failed { reason: "only on Windows".into() })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn line(reply: &Reply) -> Vec<u8> {
        let mut out = Vec::new();
        write_reply(&mut out, reply).unwrap();
        out
    }

    fn creds() -> Credentials {
        Credentials { auth_token: "tok-123".into(), ct0: "csrf-456".into() }
    }

    #[test]
    fn replies_are_single_lines_that_read_back() {
        let signed_in = line(&Reply::from(creds()));
        assert_eq!(signed_in.iter().filter(|&&b| b == b'\n').count(), 1);
        assert_eq!(read_reply(&signed_in[..]), Ok(Some(creds())));
        assert_eq!(read_reply(&line(&Reply::Closed)[..]), Ok(None));
        assert_eq!(
            read_reply(&line(&Reply::Failed { reason: "no WebView2".into() })[..]),
            Err(BrowserError::Helper("no WebView2".into()))
        );
    }

    #[test]
    fn only_the_first_line_is_read() {
        let mut both = line(&Reply::Closed);
        both.extend(b"{not json");
        assert_eq!(read_reply(&both[..]), Ok(None));
    }

    #[test]
    fn silence_and_garbage_are_errors_that_quote_nothing() {
        assert!(matches!(read_reply(&b""[..]), Err(BrowserError::Helper(_))));
        let Err(e) = read_reply(&b"{\"auth_token\":\"tok-123\"}\n"[..]) else { panic!("error") };
        assert!(!format!("{e} {e:?}").contains("tok-123"));
    }

    #[test]
    fn reply_debug_hides_values() {
        let shown = format!("{:?}", Reply::from(creds()));
        assert!(!shown.contains("tok-123") && !shown.contains("csrf-456"), "{shown}");
    }

    #[test]
    fn missing_helper_is_a_launch_error() {
        let dir = tempfile::tempdir().unwrap();
        let helper = HelperProcess::new(dir.path().join("missing.exe"));
        assert!(matches!(helper.sign_in(), Err(BrowserError::Launch(_))));
        helper.cancel();
    }
}
