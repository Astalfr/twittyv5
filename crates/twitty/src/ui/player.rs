//! A playing video: frames from `crate::video`, timed by
//! `state::playback::Timeline`.
//!
//! Each frame is its own `RenderImage`, so its own texture in the window's
//! sprite atlas: frames taken off screen (and whatever is left when the view
//! goes) are dropped from the atlas, or every frame ever shown would stay
//! there.

use std::{
    sync::{
        Arc,
        mpsc::{Receiver, TryRecvError},
    },
    time::Instant,
};

use gpui_kit::{
    Context, ImageSource, IntoElement, ObjectFit, ParentElement, Render, RenderImage, Styled,
    StyledImage, Subscription, Window, assets::IconName, component::*, div, img, prelude::*, px,
};

use crate::{
    state::playback::{State, Timeline},
    video::{self, Repeat, VideoEvent},
};

/// Plays one video, filling its box; a click pauses or resumes it.
pub struct VideoPlayer {
    /// `None` once the decoder is done (or there was none).
    events: Option<Receiver<VideoEvent>>,
    timeline: Timeline<Arc<RenderImage>>,
    failed: bool,
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
        Self { failed: events.is_none(), events, timeline: Timeline::default(), _release: release }
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
        let paused = self.timeline.state() == State::Paused;
        div()
            .id("video-player")
            .relative()
            .size_full()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                // Not a click on the viewer's backdrop.
                cx.stop_propagation();
                this.toggle(cx);
            }))
            .children(self.timeline.current().map(|frame| {
                img(ImageSource::Render(Arc::clone(frame)))
                    .size_full()
                    .object_fit(ObjectFit::Contain)
            }))
            .when(paused, |d| {
                d.child(
                    div().absolute().inset_0().flex().items_center().justify_center().child(
                        div()
                            .p_3()
                            .rounded_full()
                            .bg(gpui_kit::black().opacity(0.6))
                            .text_color(gpui_kit::white())
                            .child(Icon::new(IconName::Play).size(px(32.))),
                    ),
                )
            })
    }
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
    fn a_click_pauses_and_resumes(cx: &mut TestAppContext) {
        let (view, cx) = player(vec![Duration::ZERO], cx);
        let center = cx.debug_bounds("video-player").map(|b| b.center());
        let at = center.unwrap_or_else(|| gpui_kit::point(px(10.), px(10.)));
        cx.simulate_click(at, gpui_kit::Modifiers::default());
        assert_eq!(view.read_with(cx, |p, _| p.state()), State::Paused);
        cx.simulate_click(at, gpui_kit::Modifiers::default());
        assert_eq!(view.read_with(cx, |p, _| p.state()), State::Playing);
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
