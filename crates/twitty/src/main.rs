//! Twitty: a native X client with an X Pro style deck.

use std::{sync::Arc, time::Duration};

use anyhow::Context as _;
use gpui_kit::{
    AppContext as _, Bounds, SharedString, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};
use twitty_model::{Backend, FeedSource, HomeMode, mock::MockBackend};

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

    // Until the X backend lands (M3–M5) the app runs on demo data.
    let mock = MockBackend::new();
    let backend: Arc<dyn Backend> = Arc::new(mock.clone());

    let deck_path = deck::default_path();
    let deck = match deck_path.as_deref().map(DeckFile::load) {
        Some(Ok(deck)) => deck,
        Some(Err(e)) => {
            tracing::error!("could not load decks, using defaults: {e}");
            DeckFile::default()
        }
        None => DeckFile::default(),
    };
    let options = AppOptions {
        deck_path,
        poll: true,
        banner: Some(SharedString::from("Demo mode: showing generated posts")),
    };

    gpui_kit::application().with_assets(ui::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        ui::theme::init(ui::theme::NAMES[0], cx);
        cx.set_global(runtime);
        cx.set_http_client(Arc::new(http));

        // Demo: new posts trickle into Home so polling and the "new posts" pill can be seen.
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(20)).await;
                for mode in [HomeMode::Following, HomeMode::ForYou] {
                    mock.push_new(&FeedSource::Home { mode }, 2);
                }
            }
        })
        .detach();

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
