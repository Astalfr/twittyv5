//! GIFs playing inside timeline media tiles.
//!
//! Each GIF tile keeps a [`GifTile`] as GPUI keyed state, which lives only
//! while the tile is drawn: a row scrolled out of the list drops it, and with
//! it the player and its decoder thread. Whether it plays comes from
//! `state::playback::gif_plays` (the autoplay setting until the viewer clicks
//! the play/pause button).

use std::rc::Rc;

use gpui_kit::{
    App, Context, ElementId, Entity, Global, IntoElement, ParentElement, Styled, Window, div,
    prelude::*,
};

use crate::{
    state::playback::{Autoplay, GifChoice, gif_plays, gif_toggled},
    ui::player::{DrawFrame, Options, VideoPlayer, play_pause_sign},
};

/// Diameter of the play/pause button.
const BUTTON: f32 = 44.;

/// The autoplay setting, for the rows to read. Without it, GIFs wait for a
/// click.
#[derive(Debug, Clone, Copy)]
pub struct GifAutoplay(pub Autoplay);

impl Global for GifAutoplay {}

/// The app's autoplay setting.
pub fn autoplay(cx: &App) -> Autoplay {
    cx.try_global::<GifAutoplay>().map_or(Autoplay::Off, |g| g.0)
}

/// One inline GIF, while its tile is on screen.
#[derive(Debug)]
pub struct GifTile {
    url: String,
    choice: Option<GifChoice>,
    player: Option<Entity<VideoPlayer>>,
}

impl GifTile {
    pub fn new(url: String) -> Self {
        Self { url, choice: None, player: None }
    }

    pub fn player(&self) -> Option<&Entity<VideoPlayer>> {
        self.player.as_ref()
    }

    pub fn plays(&self, autoplay: Autoplay) -> bool {
        gif_plays(autoplay, self.choice)
    }

    /// What the play/pause button does.
    pub fn toggle(&mut self, autoplay: Autoplay, cx: &mut Context<Self>) {
        self.choice = Some(gif_toggled(autoplay, self.choice));
        cx.notify();
    }

    /// Starts, resumes or pauses the player to match [`GifTile::plays`]. The
    /// player is made the first time it should play.
    fn sync(&mut self, autoplay: Autoplay, window: &mut Window, cx: &mut Context<Self>) {
        let plays = self.plays(autoplay);
        match &self.player {
            Some(player) => player.update(cx, |p, cx| p.set_playing(plays, cx)),
            None if plays => {
                let url = self.url.clone();
                self.player = Some(cx.new(|cx| VideoPlayer::new(&url, Options::GIF, window, cx)));
            }
            None => {}
        }
    }
}

/// The GIF layer of a media tile, drawn over its poster: the playing frames
/// (drawn by `draw`, which matches the poster's fit and corners) and, while
/// the tile is hovered, a play/pause button in the middle. The button doesn't
/// open the media viewer; the rest of the tile still does.
///
/// `key` must be unique among the tiles drawn in the current element
/// namespace.
pub fn inline_gif(
    key: ElementId,
    url: &str,
    draw: DrawFrame,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let tile = window.use_keyed_state(key, cx, |_, _| GifTile::new(url.to_owned()));
    let autoplay = autoplay(cx);
    tile.update(cx, |t, cx| t.sync(autoplay, window, cx));
    let (plays, player) = {
        let t = tile.read(cx);
        (t.plays(autoplay), t.player().filter(|p| !p.read(cx).failed()).cloned())
    };
    if let Some(player) = &player {
        player.update(cx, |p, _| p.set_draw(Rc::clone(&draw)));
    }
    div()
        .absolute()
        .inset_0()
        .group("gif-tile")
        .children(player.map(|player| div().absolute().inset_0().child(player)))
        .child(
            div().absolute().inset_0().flex().items_center().justify_center().child(
                play_pause_sign(plays, BUTTON)
                    .id("gif-toggle")
                    .debug_selector(move || if plays { "gif-pause" } else { "gif-play" }.into())
                    .cursor_pointer()
                    .opacity(0.)
                    .group_hover("gif-tile", |s| s.opacity(1.))
                    .on_click(move |_, _, cx| {
                        // Play or pause here; don't open the viewer.
                        cx.stop_propagation();
                        tile.update(cx, |t, cx| t.toggle(autoplay, cx));
                    }),
            ),
        )
}
