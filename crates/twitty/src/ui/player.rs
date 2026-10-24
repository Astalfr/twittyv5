//! A playing video: frames from `crate::video`, timed by
//! `state::playback::Timeline`.
//!
//! Each frame is its own `RenderImage`, so its own texture in the window's
//! sprite atlas: frames taken off screen (and whatever is left when the view
//! goes) are dropped from the atlas, or every frame ever shown would stay
//! there.
//!
//! The player only draws pictures; its container handles clicks (the media
//! viewer pauses on a click, a timeline tile has a play/pause button).

use std::{
    rc::Rc,
    sync::{
        Arc,
        mpsc::{Receiver, TryRecvError},
    },
    time::Instant,
};

use gpui_kit::{
    Context, Div, ImageSource, Img, IntoElement, ObjectFit, ParentElement, Render, RenderImage,
    Styled, StyledImage, Subscription, Window, assets::IconName, component::*, div, img, px,
};

use crate::{
    state::playback::{State, Timeline},
    video::{self, Repeat, VideoEvent},
};

/// How a frame is drawn in the player's box.
pub type DrawFrame = Rc<dyn Fn(ImageSource) -> Img>;

/// Plays one video in its box.
pub struct VideoPlayer {
    /// `None` once the decoder is done (or there was none).
    events: Option<Receiver<VideoEvent>>,
    timeline: Timeline<Arc<RenderImage>>,
    failed: bool,
    draw: DrawFrame,
    _release: Subscription,
}

impl std::fmt::Debug for VideoPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoPlayer")
            .field("state", &self.timeline.state())
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

impl VideoPlayer {
    /// Starts playing `url`. Without a video source installed it fails at
    /// once, and the caller keeps showing the poster.
    pub fn new(url: &str, repeat: Repeat, window: &Window, cx: &mut Context<Self>) -> Self {
        let events = video::open(url, repeat, cx);
        // With the window: the player is usually released while the window
        // updates, when `App::drop_image` alone can't reach its atlas.
        let release = cx.on_release_in(window, |this: &mut Self, window, cx| {
            for image in this.timeline.drain() {
                cx.drop_image(image, Some(window));
            }
        });
        Self {
            failed: events.is_none(),
            events,
            timeline: Timeline::default(),
            draw: Rc::new(|src| img(src).size_full().object_fit(ObjectFit::Contain)),
            _release: release,
        }
    }

    /// Sets how frames are drawn (by default fitted inside the box). The
    /// container calls it as it renders, as its box changes.
    pub fn set_draw(&mut self, draw: DrawFrame) {
        self.draw = draw;
    }

    /// Whether the video can't be played (the reason is in the log).
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// The frame on screen.
    pub fn current(&self) -> Option<&Arc<RenderImage>> {
        self.timeline.current()
    }

    pub fn state(&self) -> State {
        self.timeline.state()
    }

    /// Pauses or resumes.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.timeline.toggle(Instant::now());
        cx.notify();
    }

    /// Plays or pauses, whichever it isn't doing yet.
    pub fn set_playing(&mut self, playing: bool, cx: &mut Context<Self>) {
        if (self.timeline.state() == State::Playing) != playing {
            self.toggle(cx);
        }
    }

    /// Takes what the decoder sent, advances the clock and frees the frames
    /// taken off screen. Runs on every draw.
    fn pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_failed = self.failed;
        let mut done = false;
        if let Some(events) = &self.events {
            while self.timeline.wants_more() {
                match events.try_recv() {
                    Ok(VideoEvent::Frame { time, image }) => self.timeline.push(time, image),
                    Ok(VideoEvent::Ended) => self.timeline.end(),
                    Ok(VideoEvent::Failed(_)) => {
                        self.failed = true;
                        self.timeline.end();
                    }
                    Err(TryRecvError::Empty) => break,
                    // The decoder thread is gone without a word: nothing more comes.
                    Err(TryRecvError::Disconnected) => {
                        self.timeline.end();
                        done = true;
                        break;
                    }
                }
            }
        }
        if done || !self.timeline.wants_more() && self.failed {
            self.events = None;
        }
        for image in self.timeline.tick(Instant::now()) {
            cx.drop_image(image, Some(window));
        }
        // A failure learnt while drawing: the viewer has already drawn this
        // frame as if the GIF plays, so draw once more for its fallback.
        if self.timeline.is_running() || self.failed != was_failed {
            window.request_animation_frame();
        }
    }
}

impl Render for VideoPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.pump(window, cx);
        div().size_full().children(
            self.timeline.current().map(|f| (self.draw)(ImageSource::Render(Arc::clone(f)))),
        )
    }
}

/// A round play or pause sign of `size` drawn over media: "play" when the
/// media is still, "pause" while it plays. Over pictures, so it uses fixed
/// light-on-dark colours like the media badges, not theme colours.
pub fn play_pause_sign(playing: bool, size: f32) -> Div {
    let icon = if playing { IconName::Pause } else { IconName::Play };
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded_full()
        .bg(gpui_kit::black().opacity(0.6))
        .text_color(gpui_kit::white())
        .child(Icon::new(icon).size(px(size / 2.)))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use gpui_kit::{Entity, TestAppContext, VisualTestContext};

    use super::*;
    use crate::video::FakeVideo;

    fn player(
        frames: Vec<Duration>,
        cx: &mut TestAppContext,
    ) -> (Entity<VideoPlayer>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            FakeVideo::install(frames, cx);
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            VideoPlayer::new("https://video.twimg.com/a.mp4", Repeat::Loop, window, cx)
        });
        redraw(cx);
        (view, cx)
    }

    fn redraw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn a_frame_taken_off_screen_leaves_the_atlas(cx: &mut TestAppContext) {
        let (view, cx) = player(vec![Duration::ZERO, Duration::from_millis(20)], cx);
        let first = view.read_with(cx, |p, _| p.current().cloned()).expect("a frame is up");
        assert!(cx.update(|window, _| window.has_image_atlas_entry(&first)), "it was painted");
        // The playback clock is real time.
        std::thread::sleep(Duration::from_millis(40));
        redraw(cx);
        let second = view.read_with(cx, |p, _| p.current().cloned()).expect("a frame is up");
        assert!(!Arc::ptr_eq(&first, &second), "the next frame is up");
        assert!(cx.update(|window, _| window.has_image_atlas_entry(&second)));
        assert!(!cx.update(|window, _| window.has_image_atlas_entry(&first)), "the old one left");
        assert!(!view.read_with(cx, |p, _| p.failed()));
    }

    #[gpui_kit::test]
    fn closing_the_player_frees_its_last_frame(cx: &mut TestAppContext) {
        let (view, cx) = player(vec![Duration::ZERO], cx);
        let shown = view.read_with(cx, |p, _| p.current().cloned()).expect("a frame is up");
        assert!(cx.update(|window, _| window.has_image_atlas_entry(&shown)));
        // Replacing the window's root releases the player.
        cx.update(|window, cx| {
            window.replace_root(cx, |_, _| gpui_kit::Empty);
        });
        let weak = view.downgrade();
        drop(view);
        cx.run_until_parked();
        redraw(cx);
        assert!(weak.upgrade().is_none(), "the player was released");
        assert!(!cx.update(|window, _| window.has_image_atlas_entry(&shown)));
    }

    #[gpui_kit::test]
    fn set_playing_only_changes_what_differs(cx: &mut TestAppContext) {
        let (view, cx) = player(vec![Duration::ZERO], cx);
        let state = |cx: &mut VisualTestContext| view.read_with(cx, |p, _| p.state());
        view.update(cx, |p, cx| p.set_playing(true, cx));
        assert_eq!(state(cx), State::Playing, "already playing");
        view.update(cx, |p, cx| p.set_playing(false, cx));
        assert_eq!(state(cx), State::Paused);
        view.update(cx, |p, cx| p.set_playing(false, cx));
        assert_eq!(state(cx), State::Paused, "already paused");
        view.update(cx, |p, cx| p.set_playing(true, cx));
        assert_eq!(state(cx), State::Playing);
    }

    #[gpui_kit::test]
    fn without_a_video_source_it_fails_soft(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| {
            VideoPlayer::new("https://video.twimg.com/a.mp4", Repeat::Once, window, cx)
        });
        cx.run_until_parked();
        assert!(view.read_with(cx, |p, _| p.failed() && p.current().is_none()));
    }
}
