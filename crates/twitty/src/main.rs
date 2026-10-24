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
use gpui_kit::{AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _};
use twitty_model::{Backend, FeedSource, HomeMode, mock::MockBackend};
use twitty_x::{Session, SessionError, XBackend, XClient, transport::RotateHook};

use twitty::{
    account::{
        AccountServices, ConnectError, Connector, CredentialStore, Credentials, KeyringStore,
        SessionSource,
    },
    logging,
    runtime::Runtime,
    state::{
        deck::{self, DeckFile},
        settings::{self, Settings},
    },
    ui::{
        self,
        app::{AppOptions, TwittyApp},
    },
};

fn main() -> anyhow::Result<()> {
    let log_path = logging::default_path();
    let log_file = log_path.as_deref().map(|p| logging::open(p, logging::MAX_BYTES));
    let file_layer = match &log_file {
        Some(Ok(file)) => Some(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file.try_clone()?)),
        ),
        _ => None,
    };
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "twitty=info,twitty_x=info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(file_layer)
        .init();
    logging::log_panics();
    if let Some(Err(e)) = &log_file {
        tracing::warn!("could not open the log file: {e}");
    }
    tracing::info!("Twitty {} starting", env!("CARGO_PKG_VERSION"));

    let runtime = Runtime::tokio().context("starting network runtime")?;
    let http =
        reqwest_client::ReqwestClient::user_agent(concat!("twitty/", env!("CARGO_PKG_VERSION")))
            .context("creating image HTTP client")?;

    let store: Arc<dyn CredentialStore> = Arc::new(KeyringStore::x_session());
    let connect = x_connector(Arc::clone(&store));
    let found = find_session(Session::from_env(), store.as_ref(), &session_files());
    let (backend, mock, session): (Arc<dyn Backend>, _, _) = match found {
        Some((session, origin)) => {
            tracing::info!("using the X session from {origin}");
            let on_rotate = match &origin {
                SessionOrigin::Keyring => Some(keyring_rotation(Arc::clone(&store))),
                SessionOrigin::File(path) => Some(file_rotation(path.clone())),
                SessionOrigin::Env => None,
            };
            let client = XClient::production(session, on_rotate).context("creating X client")?;
            let source = match origin {
                SessionOrigin::Keyring => SessionSource::Keyring,
                other => SessionSource::External(other.to_string()),
            };
            (Arc::new(XBackend::new(Arc::new(client))), None, Some(source))
        }
        None => {
            let mock = MockBackend::new();
            (Arc::new(mock.clone()), Some(mock), None)
        }
    };

    let deck_path = deck::default_path();
    let deck = match deck_path.as_deref().map(DeckFile::load) {
        Some(Ok(deck)) => deck,
        Some(Err(e)) => {
            tracing::error!("could not load decks, using defaults: {e}");
            DeckFile::default()
        }
        None => DeckFile::default(),
    };
    let settings_path = settings::default_path();
    let settings = match settings_path.as_deref().map(Settings::load) {
        Some(Ok(settings)) => settings,
        Some(Err(e)) => {
            tracing::error!("could not load settings, using defaults: {e}");
            Settings::default()
        }
        None => Settings::default(),
    };
    let theme_name = settings.theme_or_default(&ui::theme::NAMES).unwrap_or(ui::theme::NAMES[0]);
    let options = AppOptions {
        deck_path,
        settings_path,
        log_path,
        poll: true,
        session,
        account: Some(AccountServices { connect, store }),
    };

    gpui_kit::application().with_assets(ui::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        ui::theme::init(theme_name, cx);
        ui::keys::bind(cx);
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
        if let Err(e) = gpui_kit::open_window(window, cx, |window, cx| {
            cx.new(|cx| TwittyApp::new(deck, settings, backend, options, window, cx))
        }) {
            tracing::error!("failed to open window: {e:#}");
            cx.quit();
        }
    });
    Ok(())
}

/// Builds X backends for the sign-in dialog. Their rotated cookies go to the keyring.
fn x_connector(store: Arc<dyn CredentialStore>) -> Connector {
    Arc::new(move |c: &Credentials| {
        let session = Session::new(c.auth_token.as_str(), c.ct0.as_str())
            .map_err(|e| ConnectError::Malformed(e.to_string()))?;
        let client = XClient::production(session, Some(keyring_rotation(Arc::clone(&store))))
            .map_err(|e| ConnectError::Setup(e.to_string()))?;
        Ok(Arc::new(XBackend::new(Arc::new(client))) as Arc<dyn Backend>)
    })
}

/// Where the X session was found.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionOrigin {
    Env,
    Keyring,
    File(PathBuf),
}

impl std::fmt::Display for SessionOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionOrigin::Env => f.write_str("TWITTY_AUTH_TOKEN/TWITTY_CT0"),
            SessionOrigin::Keyring => f.write_str("Windows Credential Manager"),
            SessionOrigin::File(path) => write!(f, "{}", path.display()),
        }
    }
}

/// Session files for development, in order of preference: the per-user
/// config directory, then `session.json` in the working directory (handy with
/// `cargo run`). The sign-in dialog stores sessions in the keyring instead.
fn session_files() -> Vec<PathBuf> {
    let config = directories::ProjectDirs::from("", "", "Twitty")
        .map(|d| d.config_dir().join("session.json"));
    config.into_iter().chain([PathBuf::from("session.json")]).collect()
}

/// The first usable session: environment variables, then the keyring, then
/// the first `files` entry that exists and parses. Unusable sources are
/// logged (never their contents) and skipped.
fn find_session(
    env: Result<Session, SessionError>,
    store: &dyn CredentialStore,
    files: &[PathBuf],
) -> Option<(Session, SessionOrigin)> {
    match env {
        Ok(session) => return Some((session, SessionOrigin::Env)),
        Err(SessionError::MissingEnv) => {}
        Err(e) => tracing::error!("ignoring TWITTY_AUTH_TOKEN/TWITTY_CT0: {e}"),
    }
    match store.load() {
        Ok(Some(c)) => match Session::new(c.auth_token, c.ct0) {
            Ok(session) => return Some((session, SessionOrigin::Keyring)),
            Err(e) => tracing::error!("ignoring the stored session: {e}"),
        },
        Ok(None) => {}
        Err(e) => tracing::error!("could not read the stored session: {e}"),
    }
    files.iter().filter(|path| path.is_file()).find_map(|path| match Session::from_file(path) {
        Ok(session) => Some((session, SessionOrigin::File(path.clone()))),
        Err(e) => {
            tracing::error!("ignoring {}: {e}", path.display());
            None
        }
    })
}

/// Writes rotated cookies back to the keyring, so the next launch doesn't
/// start with a stale `ct0`.
fn keyring_rotation(store: Arc<dyn CredentialStore>) -> RotateHook {
    Arc::new(move |session: &Session| {
        let credentials = Credentials {
            auth_token: session.auth_token.expose().to_owned(),
            ct0: session.ct0.expose().to_owned(),
        };
        match store.save(&credentials) {
            Ok(()) => tracing::info!("saved rotated X session"),
            Err(e) => tracing::error!("could not save rotated X session: {e}"),
        }
    })
}

/// Writes rotated cookies back to the session file.
fn file_rotation(path: PathBuf) -> RotateHook {
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
    use twitty::account::MemoryStore;

    fn empty() -> MemoryStore {
        MemoryStore::default()
    }

    fn stored(auth_token: &str, ct0: &str) -> MemoryStore {
        let store = MemoryStore::default();
        store.save(&Credentials { auth_token: auth_token.into(), ct0: ct0.into() }).unwrap();
        store
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn env_wins_over_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "s.json", r#"{"auth_token":"f","ct0":"f"}"#);
        let (session, origin) =
            find_session(Session::new("e", "e"), &stored("k", "k"), &[file]).unwrap();
        assert_eq!(origin, SessionOrigin::Env);
        assert_eq!(session.auth_token.expose(), "e");
    }

    #[test]
    fn first_valid_existing_file_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.json");
        let broken = write(dir.path(), "broken.json", "not json");
        let good = write(dir.path(), "good.json", r#"{"auth_token":"a","ct0":"c"}"#);
        let found =
            find_session(Err(SessionError::MissingEnv), &empty(), &[missing, broken, good.clone()]);
        assert_eq!(found.map(|(_, o)| o), Some(SessionOrigin::File(good)));
    }

    #[test]
    fn no_session_means_demo_mode() {
        let dir = tempfile::tempdir().unwrap();
        let files = [dir.path().join("x.json")];
        assert!(find_session(Err(SessionError::MissingEnv), &empty(), &files).is_none());
    }

    #[test]
    fn rotation_is_written_back_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "session.json", r#"{"auth_token":"a","ct0":"old"}"#);
        file_rotation(path.clone())(&Session::new("a", "new").unwrap());
        assert_eq!(Session::from_file(&path).unwrap().ct0.expose(), "new");
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn keyring_wins_over_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "s.json", r#"{"auth_token":"f","ct0":"f"}"#);
        let found = find_session(Err(SessionError::MissingEnv), &stored("k", "c"), &[file]);
        let (session, origin) = found.unwrap();
        assert_eq!(origin, SessionOrigin::Keyring);
        assert_eq!(session.auth_token.expose(), "k");
    }

    #[test]
    fn malformed_stored_session_falls_through_to_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "s.json", r#"{"auth_token":"f","ct0":"f"}"#);
        let found = find_session(
            Err(SessionError::MissingEnv),
            &stored("a b", "c"),
            std::slice::from_ref(&file),
        );
        assert_eq!(found.map(|(_, o)| o), Some(SessionOrigin::File(file)));
    }

    #[test]
    fn keyring_rotation_updates_the_store() {
        let store = stored("a", "old");
        keyring_rotation(Arc::new(store.clone()))(&Session::new("a", "new").unwrap());
        assert_eq!(store.load().unwrap().unwrap().ct0, "new");
    }

    #[test]
    fn connector_rejects_malformed_cookies() {
        let connect = x_connector(Arc::new(empty()));
        let bad = Credentials { auth_token: "has space".into(), ct0: "c".into() };
        assert!(matches!(connect(&bad), Err(ConnectError::Malformed(_))));
    }
}
