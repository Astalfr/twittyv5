//! Twitty: a native X client with an X Pro style deck.

// Release builds are GUI apps: no console window on Windows. Debug builds keep
// the console so logs are visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::Context as _;
use gpui_kit::{
    AppContext as _, Bounds, SharedString, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};
use twitty_model::{Backend, FeedSource, HomeMode, mock::MockBackend};
use twitty_x::{Session, SessionError, XBackend, XClient};

use twitty::{
    runtime::Runtime,
    state::deck::{self, DeckFile},
    ui::{
        self,
        app::{AppOptions, TwittyApp},
    },
};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "twitty=info,twitty_x=info".into()),
        )
        .init();

    let runtime = Runtime::tokio().context("starting network runtime")?;
    let http =
        reqwest_client::ReqwestClient::user_agent(concat!("twitty/", env!("CARGO_PKG_VERSION")))
            .context("creating image HTTP client")?;

    let session = find_session(Session::from_env(), &session_files());
    let (backend, mock, banner): (Arc<dyn Backend>, _, _) = match session {
        Some((session, origin)) => {
            tracing::info!("using the X session from {origin}");
            let on_rotate = match origin {
                SessionOrigin::File(path) => Some(persist_rotation(path)),
                SessionOrigin::Env => None,
            };
            let client = XClient::production(session, on_rotate).context("creating X client")?;
            (Arc::new(XBackend::new(Arc::new(client))), None, None)
        }
        None => {
            let mock = MockBackend::new();
            let banner = SharedString::from(
                "Demo mode: no X session found (see \"Signing in\" in docs/x-api.md)",
            );
            (Arc::new(mock.clone()), Some(mock), Some(banner))
        }
    };
    if mock.is_none() {
        let backend = Arc::clone(&backend);
        // Only logged: columns show their own errors.
        drop(runtime.spawn(async move {
            match backend.viewer().await {
                Ok(me) => tracing::info!("signed in as @{}", me.handle),
                Err(e) => tracing::error!("the X session doesn't work: {e}"),
            }
        }));
    }

    let deck_path = deck::default_path();
    let deck = match deck_path.as_deref().map(DeckFile::load) {
        Some(Ok(deck)) => deck,
        Some(Err(e)) => {
            tracing::error!("could not load decks, using defaults: {e}");
            DeckFile::default()
        }
        None => DeckFile::default(),
    };
    let options = AppOptions { deck_path, poll: true, banner };

    gpui_kit::application().with_assets(ui::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        ui::theme::init(ui::theme::NAMES[0], cx);
        cx.set_global(runtime);
        cx.set_http_client(Arc::new(http));

        // Demo: new posts trickle into Home so polling and the "new posts" pill can be seen.
        if let Some(mock) = mock {
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(20)).await;
                    for mode in [HomeMode::Following, HomeMode::ForYou] {
                        mock.push_new(&FeedSource::Home { mode }, 2);
                    }
                }
            })
            .detach();
        }

        let bounds = Bounds::centered(None, size(px(1480.), px(920.)), cx);
        let window = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Twitty".into()), ..Default::default() }),
            ..Default::default()
        };
        if let Err(e) = gpui_kit::open_window(window, cx, |_, cx| {
            cx.new(|cx| TwittyApp::new(deck, backend, options, cx))
        }) {
            tracing::error!("failed to open window: {e:#}");
            cx.quit();
        }
    });
    Ok(())
}

/// Where the X session was found.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionOrigin {
    Env,
    File(PathBuf),
}

impl std::fmt::Display for SessionOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionOrigin::Env => f.write_str("TWITTY_AUTH_TOKEN/TWITTY_CT0"),
            SessionOrigin::File(path) => write!(f, "{}", path.display()),
        }
    }
}

/// Session files, in order of preference: the per-user config directory,
/// then `session.json` in the working directory (handy with `cargo run`).
/// The keyring replaces both in M6.
fn session_files() -> Vec<PathBuf> {
    let config = directories::ProjectDirs::from("", "", "Twitty")
        .map(|d| d.config_dir().join("session.json"));
    config.into_iter().chain([PathBuf::from("session.json")]).collect()
}

/// The first usable session: environment variables, then the first `files`
/// entry that exists and parses. Unusable sources are logged (never their
/// contents) and skipped.
fn find_session(
    env: Result<Session, SessionError>,
    files: &[PathBuf],
) -> Option<(Session, SessionOrigin)> {
    match env {
        Ok(session) => return Some((session, SessionOrigin::Env)),
        Err(SessionError::MissingEnv) => {}
        Err(e) => tracing::error!("ignoring TWITTY_AUTH_TOKEN/TWITTY_CT0: {e}"),
    }
    files.iter().filter(|path| path.is_file()).find_map(|path| match Session::from_file(path) {
        Ok(session) => Some((session, SessionOrigin::File(path.clone()))),
        Err(e) => {
            tracing::error!("ignoring {}: {e}", path.display());
            None
        }
    })
}

/// Writes rotated cookies back to the session file, so the next launch
/// doesn't start with a stale `ct0`.
fn persist_rotation(path: PathBuf) -> twitty_x::transport::RotateHook {
    // Concurrent responses can each carry the rotation; they share the temp file.
    let writing = std::sync::Mutex::new(());
    Arc::new(move |session: &Session| {
        let _guard = writing.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match save_session(&path, session) {
            Ok(()) => tracing::info!("saved rotated X session to {}", path.display()),
            Err(e) => {
                tracing::error!("could not save rotated X session to {}: {e:#}", path.display());
            }
        }
    })
}

fn save_session(path: &Path, session: &Session) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(session)?;
    // Write then rename, so a crash never leaves a half-written session.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn env_wins_over_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "s.json", r#"{"auth_token":"f","ct0":"f"}"#);
        let (session, origin) = find_session(Session::new("e", "e"), &[file]).unwrap();
        assert_eq!(origin, SessionOrigin::Env);
        assert_eq!(session.auth_token.expose(), "e");
    }

    #[test]
    fn first_valid_existing_file_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.json");
        let broken = write(dir.path(), "broken.json", "not json");
        let good = write(dir.path(), "good.json", r#"{"auth_token":"a","ct0":"c"}"#);
        let found = find_session(Err(SessionError::MissingEnv), &[missing, broken, good.clone()]);
        assert_eq!(found.map(|(_, o)| o), Some(SessionOrigin::File(good)));
    }

    #[test]
    fn no_session_means_demo_mode() {
        let dir = tempfile::tempdir().unwrap();
        let files = [dir.path().join("x.json")];
        assert!(find_session(Err(SessionError::MissingEnv), &files).is_none());
    }

    #[test]
    fn rotation_is_written_back_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "session.json", r#"{"auth_token":"a","ct0":"old"}"#);
        persist_rotation(path.clone())(&Session::new("a", "new").unwrap());
        assert_eq!(Session::from_file(&path).unwrap().ct0.expose(), "new");
        assert!(!path.with_extension("json.tmp").exists());
    }
}
