//! A playing video: frames from `crate::video`, timed by
//! `state::playback::Timeline` (on the sound's clock when the video has
//! sound).
//!
//! Each frame is its own `RenderImage`, so its own texture in the window's
//! sprite atlas: frames taken off screen (and whatever is left when the view
//! goes) are dropped from the atlas, or every frame ever shown would stay
//! there.
//!
//! The player draws pictures and, for videos, a control bar (play/pause,
//! time, progress, mute, volume). Clicks on the picture belong to its
//! container (the media viewer pauses on a click, a timeline tile has a
//! play/pause button); the bar's clicks stay in the bar.

use std::{
    rc::Rc,
    sync::{
        Arc,
        mpsc::{Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};

use gpui_kit::{
    App, Context, Div, Entity, Global, ImageSource, Img, IntoElement, ObjectFit, ParentElement,
    Render, RenderImage, Styled, StyledImage, Subscription, Window, assets::IconName,
    component::slider::*, component::*, div, img, prelude::*, px,
};

use crate::{
    state::playback::{State, Timeline},
    ui::post::fmt_duration,
    video::{self, Control, Playback, Repeat, Sound, VideoEvent},
};

/// How a frame is drawn in the player's box.
pub type DrawFrame = Rc<dyn Fn(ImageSource) -> Img>;

/// Whether the player draws a control bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Controls {
    None,
    Bar,
}

/// How a player plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub repeat: Repeat,
    pub sound: Sound,
    pub controls: Controls,
    /// The length to show until the file says.
    pub duration: Option<Duration>,
}

impl Options {
    /// A GIF: looping, silent, no bar.
    pub const GIF: Self =
        Self { repeat: Repeat::Loop, sound: Sound::Off, controls: Controls::None, duration: None };

    /// A video: once, with sound and the control bar.
    pub fn video(duration: Option<Duration>) -> Self {
        Self { repeat: Repeat::Once, sound: Sound::On, controls: Controls::Bar, duration }
    }
}

/// The video volume for the session (not saved): every player starts with
/// it, and changing it in one sets it for the next.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoVolume {
    /// 0 to 1.
    pub level: f32,
    pub muted: bool,
}

impl Default for VideoVolume {
    fn default() -> Self {
        Self { level: 1.0, muted: false }
    }
}

impl Global for VideoVolume {}

impl VideoVolume {
    /// The gain the sound plays at.
    pub fn gain(self) -> f32 {
        if self.muted { 0.0 } else { self.level }
    }
}

/// Steps of the progress slider.
const PROGRESS_STEPS: f32 = 1000.;

/// Plays one video in its box.
pub struct VideoPlayer {
    /// `None` once the decoder is done (or there was none).
    events: Option<Receiver<VideoEvent>>,
    control: Option<Arc<dyn Control>>,
    /// Bumped by each seek; older frames are dropped.
    generation: u64,
    timeline: Timeline<Arc<RenderImage>>,
    failed: bool,
    draw: DrawFrame,
    options: Options,
    duration: Option<Duration>,
    /// A seek's target, shown until its first frame arrives.
    seeking: Option<Duration>,
    /// Where the progress slider is being dragged (0 to 1), until released.
    scrub: Option<f32>,
    volume: VideoVolume,
    progress: Option<Entity<SliderState>>,
    volume_slider: Option<Entity<SliderState>>,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for VideoPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoPlayer")
            .field("state", &self.timeline.state())
            .field("failed", &self.failed)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl VideoPlayer {
    /// Starts playing `url`. Without a video source installed it fails at
    /// once, and the caller keeps showing the poster.
    pub fn new(url: &str, options: Options, window: &Window, cx: &mut Context<Self>) -> Self {
        let playback = video::open(url, options.repeat, options.sound, cx);
        Self::with_playback(playback, options, window, cx)
    }

    fn with_playback(
        playback: Option<Playback>,
        options: Options,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // With the window: the player is usually released while the window
        // updates, when `App::drop_image` alone can't reach its atlas.
        let mut subscriptions = vec![cx.on_release_in(window, |this: &mut Self, window, cx| {
            for image in this.timeline.drain() {
                cx.drop_image(image, Some(window));
            }
        })];
        let volume = cx.try_global::<VideoVolume>().copied().unwrap_or_default();
        let (progress, volume_slider) = match options.controls {
            Controls::None => (None, None),
            Controls::Bar => {
                let progress = cx.new(|_| SliderState::new().max(PROGRESS_STEPS).min(0.).step(1.));
                let level = cx.new(|_| {
                    SliderState::new().min(0.).max(100.).step(1.).default_value(volume.level * 100.)
                });
                subscriptions.push(cx.subscribe(&progress, |this, _, e: &SliderEvent, cx| {
                    match e {
                        SliderEvent::Change(v) => this.scrub = Some(v.start() / PROGRESS_STEPS),
                        SliderEvent::Release(v) => {
                            this.scrub = None;
                            // Seek once, on release: each seek restarts the download.
                            if let Some(total) = this.duration {
                                this.seek(total.mul_f32(v.start() / PROGRESS_STEPS), cx);
                            }
                        }
                    }
                    cx.notify();
                }));
                subscriptions.push(cx.subscribe(&level, |this, _, e: &SliderEvent, cx| {
                    let (SliderEvent::Change(v) | SliderEvent::Release(v)) = e;
                    this.set_volume(VideoVolume { level: v.start() / 100., muted: false }, cx);
                }));
                (Some(progress), Some(level))
            }
        };
        let (events, control) = match playback {
            Some(p) => (Some(p.events), Some(p.control)),
            None => (None, None),
        };
        if let Some(control) = &control {
            control.set_volume(volume.gain());
        }
        Self {
            failed: events.is_none(),
            events,
            control,
            generation: 0,
            timeline: Timeline::default(),
            draw: Rc::new(|src| img(src).size_full().object_fit(ObjectFit::Contain)),
            options,
            duration: options.duration,
            seeking: None,
            scrub: None,
            volume,
            progress,
            volume_slider,
            _subscriptions: subscriptions,
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

    /// Whether the video played to its end (its last picture stays up).
    pub fn is_ended(&self) -> bool {
        self.timeline.is_ended()
    }

    /// The length of the video, when known.
    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }

    /// Where playback is: a pending seek's target, the dragged progress, or
    /// the clock.
    pub fn position(&self) -> Duration {
        let dragged = self.scrub.zip(self.duration).map(|(f, total)| total.mul_f32(f));
        let at = self.seeking.or(dragged).unwrap_or_else(|| self.timeline.position(Instant::now()));
        self.duration.map_or(at, |total| at.min(total))
    }

    pub fn volume(&self) -> VideoVolume {
        self.volume
    }

    /// Pauses or resumes; at the end, plays again from the start.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.timeline.is_ended() {
            self.seek(Duration::ZERO, cx);
            if self.timeline.state() == State::Paused {
                self.timeline.toggle(Instant::now());
            }
        } else {
            self.timeline.toggle(Instant::now());
        }
        if let Some(control) = &self.control {
            control.set_paused(self.timeline.state() == State::Paused);
        }
        cx.notify();
    }

    /// Plays or pauses, whichever it isn't doing yet.
    pub fn set_playing(&mut self, playing: bool, cx: &mut Context<Self>) {
        let is_playing = self.timeline.state() == State::Playing && !self.timeline.is_ended();
        if is_playing != playing {
            self.toggle(cx);
        }
    }

    /// Continues from `to`.
    pub fn seek(&mut self, to: Duration, cx: &mut Context<Self>) {
        self.generation += 1;
        // Queued frames were never drawn, so they aren't in the atlas.
        drop(self.timeline.seek(to));
        self.seeking = Some(to);
        if let Some(control) = &self.control {
            control.seek(to, self.generation);
        }
        cx.notify();
    }

    /// Sets the volume of this player and, for the session, of the next.
    pub fn set_volume(&mut self, volume: VideoVolume, cx: &mut Context<Self>) {
        self.volume = volume;
        if let Some(control) = &self.control {
            control.set_volume(volume.gain());
        }
        cx.set_global(volume);
        cx.notify();
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let volume = VideoVolume { muted: !self.volume.muted, ..self.volume };
        self.set_volume(volume, cx);
    }

    /// Takes what the decoder sent, advances the clock and frees the frames
    /// taken off screen. Runs on every draw.
    fn pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_failed = self.failed;
        let mut done = false;
        if let Some(events) = &self.events {
            while self.timeline.wants_more() {
                match events.try_recv() {
                    Ok(VideoEvent::Opened { duration, .. }) => {
                        self.duration = duration.or(self.duration);
                    }
                    Ok(VideoEvent::Frame { generation, time, image }) => {
                        if generation == self.generation {
                            self.seeking = None;
                            self.timeline.push(time, image);
                        }
                    }
                    Ok(VideoEvent::Ended { generation }) => {
                        if generation == self.generation {
                            self.seeking = None;
                            self.timeline.end();
                        }
                    }
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
        let audio = self.control.as_ref().and_then(|c| c.audio_position());
        for image in self.timeline.tick_with(Instant::now(), audio) {
            cx.drop_image(image, Some(window));
        }
        // A failure learnt while drawing: the viewer has already drawn this
        // frame as if the video plays, so draw once more for its fallback.
        if self.timeline.is_running() || self.seeking.is_some() || self.failed != was_failed {
            window.request_animation_frame();
        }
    }

    /// Moves the sliders to the playback, unless they are being dragged.
    fn sync_sliders(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let (Some(progress), None, Some(total)) = (&self.progress, self.scrub, self.duration)
            && !total.is_zero()
        {
            let value = (self.position().as_secs_f32() / total.as_secs_f32()) * PROGRESS_STEPS;
            // Only on a change: setting the value redraws the slider.
            if (progress.read(cx).value().start() - value).abs() >= 1. {
                progress.update(cx, |s, cx| s.set_value(value.round(), window, cx));
            }
        }
        if let Some(level) = &self.volume_slider {
            let value = self.volume.level * 100.;
            if (level.read(cx).value().start() - value).abs() >= 1. {
                level.update(cx, |s, cx| s.set_value(value.round(), window, cx));
            }
        }
    }

    fn render_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let playing = self.timeline.state() == State::Playing && !self.timeline.is_ended();
        let play_icon = if self.timeline.is_ended() {
            IconName::RotateCcw
        } else if playing {
            IconName::Pause
        } else {
            IconName::Play
        };
        let mute_icon =
            if self.volume.gain() == 0.0 { IconName::VolumeX } else { IconName::Volume2 };
        let time = format!(
            "{} / {}",
            fmt_duration(self.position()),
            self.duration.map_or_else(|| "–:––".to_owned(), fmt_duration)
        );
        let icon_button = |id: &'static str, icon: IconName| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .flex_none()
                .p_1()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(gpui_kit::white().opacity(0.15)))
                .child(Icon::new(icon).size(px(16.)))
        };
        h_flex()
            .id("video-controls")
            .debug_selector(|| "video-controls".into())
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .px_2()
            .py_1()
            .gap_2()
            .items_center()
            // Over the picture: fixed light-on-dark like the media badges.
            .bg(gpui_kit::black().opacity(0.6))
            .text_color(gpui_kit::white())
            .text_xs()
            // Shown while hovered, and always while it doesn't play.
            .opacity(if playing { 0. } else { 1. })
            .group_hover("video-player", |s| s.opacity(1.))
            // The bar's clicks aren't the picture's (pause) or the backdrop's.
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(icon_button("video-play", play_icon).on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.toggle(cx);
            })))
            .child(div().flex_none().child(time))
            .children(self.progress.as_ref().map(|p| {
                div()
                    .debug_selector(|| "video-progress".into())
                    .flex_1()
                    .min_w_0()
                    .child(Slider::new(p).accessibility_label("Progress"))
            }))
            .child(icon_button("video-mute", mute_icon).on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_mute(cx);
            })))
            .children(self.volume_slider.as_ref().map(|v| {
                div().flex_none().w(px(72.)).child(Slider::new(v).accessibility_label("Volume"))
            }))
    }
}

impl Render for VideoPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.pump(window, cx);
        let bar = self.options.controls == Controls::Bar && !self.failed;
        if bar {
            self.sync_sliders(window, cx);
        }
        div()
            .group("video-player")
            .relative()
            .size_full()
            .children(
                self.timeline.current().map(|f| (self.draw)(ImageSource::Render(Arc::clone(f)))),
            )
            .when(bar, |d| d.child(self.render_bar(cx)))
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

/// The session's video volume.
pub fn session_volume(cx: &App) -> VideoVolume {
    cx.try_global::<VideoVolume>().copied().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::video::FakeVideo;

    const URL: &str = "https://video.twimg.com/a.mp4";

    fn player_with(
        frames: Vec<Duration>,
        options: Options,
        cx: &mut TestAppContext,
    ) -> (Entity<VideoPlayer>, &mut VisualTestContext, Arc<FakeVideo>) {
        let video = cx.update(|cx| {
            gpui_kit::init(cx);
            FakeVideo::install_with_duration(frames, Duration::from_secs(10), cx)
        });
        let (view, cx) =
            cx.add_window_view(|window, cx| VideoPlayer::new(URL, options, window, cx));
        redraw(cx);
        (view, cx, video)
    }

    fn player(
        frames: Vec<Duration>,
        cx: &mut TestAppContext,
    ) -> (Entity<VideoPlayer>, &mut VisualTestContext) {
        let (view, cx, _) = player_with(frames, Options::GIF, cx);
        (view, cx)
    }

    fn redraw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    fn click(name: &'static str, cx: &mut VisualTestContext) {
        let at = cx.debug_bounds(name).unwrap_or_else(|| panic!("{name} is drawn")).center();
        cx.simulate_click(at, gpui_kit::Modifiers::default());
        redraw(cx);
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
        let (view, cx) = player(vec![Duration::ZERO, Duration::from_secs(60)], cx);
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
        let (view, cx) = cx
            .add_window_view(|window, cx| VideoPlayer::new(URL, Options::video(None), window, cx));
        cx.run_until_parked();
        assert!(view.read_with(cx, |p, _| p.failed() && p.current().is_none()));
    }

    #[gpui_kit::test]
    fn a_gif_asks_for_no_sound_and_draws_no_bar(cx: &mut TestAppContext) {
        let (_view, cx, video) = player_with(vec![Duration::ZERO], Options::GIF, cx);
        assert_eq!(video.sounds(), vec![Sound::Off]);
        assert!(cx.debug_bounds("video-controls").is_none());
    }

    #[gpui_kit::test]
    fn a_video_asks_for_sound_and_its_bar_plays_and_pauses(cx: &mut TestAppContext) {
        let frames = vec![Duration::ZERO, Duration::from_secs(60)];
        let (view, cx, video) = player_with(frames, Options::video(None), cx);
        assert_eq!(video.sounds(), vec![Sound::On]);
        assert_eq!(view.read_with(cx, |p, _| p.duration()), Some(Duration::from_secs(10)));
        click("video-play", cx);
        assert_eq!(view.read_with(cx, |p, _| p.state()), State::Paused);
        let control = video.control(0).unwrap();
        assert_eq!(control.paused(), Some(true), "the sound pauses too");
        click("video-play", cx);
        assert_eq!(view.read_with(cx, |p, _| p.state()), State::Playing);
        assert_eq!(control.paused(), Some(false));
    }

    #[gpui_kit::test]
    fn a_seek_shows_its_target_and_ignores_older_frames(cx: &mut TestAppContext) {
        // Both frames were decoded before the seek (generation 0).
        let frames = vec![Duration::ZERO, Duration::from_millis(1)];
        let (view, cx, video) = player_with(frames, Options::video(None), cx);
        let before = view.read_with(cx, |p, _| p.current().cloned());
        view.update(cx, |p, cx| p.seek(Duration::from_secs(4), cx));
        redraw(cx);
        assert_eq!(video.control(0).unwrap().seeks(), vec![(Duration::from_secs(4), 1)]);
        assert_eq!(view.read_with(cx, |p, _| p.position()), Duration::from_secs(4));
        let after = view.read_with(cx, |p, _| p.current().cloned());
        assert!(before.zip(after).is_some_and(|(a, b)| Arc::ptr_eq(&a, &b)), "no stale frame");
    }

    #[gpui_kit::test]
    fn at_the_end_play_starts_over(cx: &mut TestAppContext) {
        let (view, cx, video) = player_with(vec![Duration::ZERO], Options::video(None), cx);
        assert!(view.read_with(cx, |p, _| p.is_ended()));
        click("video-play", cx);
        assert_eq!(video.control(0).unwrap().seeks(), vec![(Duration::ZERO, 1)]);
        assert_eq!(view.read_with(cx, |p, _| p.state()), State::Playing);
    }

    #[gpui_kit::test]
    fn mute_and_volume_set_the_gain_and_carry_to_the_next_video(cx: &mut TestAppContext) {
        let (view, cx, video) =
            player_with(vec![Duration::ZERO, Duration::from_secs(60)], Options::video(None), cx);
        let control = video.control(0).unwrap();
        assert_eq!(control.volume(), Some(1.0), "full by default");
        click("video-mute", cx);
        assert_eq!(control.volume(), Some(0.0));
        view.update(cx, |p, cx| p.set_volume(VideoVolume { level: 0.4, muted: false }, cx));
        assert_eq!(control.volume(), Some(0.4));
        let next = cx.update(|window, cx| {
            cx.new(|cx| VideoPlayer::new(URL, Options::video(None), window, cx))
        });
        assert_eq!(next.read_with(cx, |p, _| p.volume().level), 0.4, "the session's volume");
        assert_eq!(video.control(1).unwrap().volume(), Some(0.4));
    }
}
