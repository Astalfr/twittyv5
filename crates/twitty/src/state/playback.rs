//! Frame timing for video playback: which decoded frame is on screen now.
//!
//! The decoder runs ahead and hands frames over with their media time; the
//! view calls [`Timeline::tick`] on every animation frame. The clock starts
//! with the first frame, so a slow connection doesn't make the opening frames
//! late. If the decoder falls behind (the queue runs dry), the clock waits for
//! it rather than dropping the frames that arrive late.
//!
//! A video with sound follows the sound instead ([`Timeline::tick_with`]):
//! while sound comes out, its position is the clock. When it doesn't (no
//! sound track, its end, a dry sound queue), the wall clock carries on from
//! the last position the sound gave, so a sound track shorter than the
//! picture, or sound that arrives late, never freezes the picture. Only when
//! both run dry does the clock wait.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Whether GIFs in timelines start on their own when they are on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Autoplay {
    On,
    Off,
}

/// What the viewer last asked an inline GIF to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GifChoice {
    Play,
    Pause,
}

/// Whether an inline GIF plays: the viewer's choice if they made one, else
/// the autoplay setting.
pub fn gif_plays(autoplay: Autoplay, choice: Option<GifChoice>) -> bool {
    match choice {
        Some(GifChoice::Play) => true,
        Some(GifChoice::Pause) => false,
        None => autoplay == Autoplay::On,
    }
}

/// The choice a click on an inline GIF's play/pause button makes: the
/// opposite of what it does now.
pub fn gif_toggled(autoplay: Autoplay, choice: Option<GifChoice>) -> GifChoice {
    if gif_plays(autoplay, choice) { GifChoice::Pause } else { GifChoice::Play }
}

/// How many decoded frames the view holds beyond the one on screen. The
/// decoder's own channel adds a few more; together they bound memory.
pub const QUEUE: usize = 4;

/// Frames waiting to be shown and the one on screen. `T` is the picture (a
/// GPUI image in the app, anything in tests).
#[derive(Debug)]
pub struct Timeline<T> {
    queue: VecDeque<(Duration, T)>,
    current: Option<T>,
    /// Media time when the clock last stopped.
    base: Duration,
    /// When the clock last started, while it runs.
    since: Option<Instant>,
    state: State,
    /// No frame will follow the queued ones.
    ended: bool,
    /// After a seek: the first new frame shows at once, even while paused.
    preview: bool,
}

/// What the viewer asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Playing,
    Paused,
}

impl<T> Default for Timeline<T> {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
            current: None,
            base: Duration::ZERO,
            since: None,
            state: State::Playing,
            ended: false,
            preview: false,
        }
    }
}

impl<T> Timeline<T> {
    /// Whether the view should take another frame from the decoder.
    pub fn wants_more(&self) -> bool {
        !self.ended && self.queue.len() < QUEUE
    }

    /// Queues a decoded frame shown at media time `time`. Frames arrive in
    /// order; one that would go back in time is shown right after the
    /// previous one instead.
    pub fn push(&mut self, time: Duration, frame: T) {
        let floor = self.queue.back().map_or(Duration::ZERO, |(t, _)| *t);
        self.queue.push_back((time.max(floor), frame));
    }

    /// No more frames will be pushed.
    pub fn end(&mut self) {
        self.ended = true;
        // A seek to the very end has no frame to preview.
        if self.queue.is_empty() {
            self.preview = false;
        }
    }

    /// Advances to time `at`: the latest due frame goes on screen. Returns
    /// the frames taken off (the old one and any skipped), for the caller to
    /// free.
    pub fn tick(&mut self, at: Instant) -> Vec<T> {
        if self.preview
            && let Some((_, frame)) = self.queue.pop_front()
        {
            self.preview = false;
            let retired = self.current.replace(frame).into_iter().collect();
            if self.state == State::Paused {
                return retired;
            }
            // Playing: the clock starts from the seek target with the
            // following frames.
            self.since = Some(at);
            return retired.into_iter().chain(self.tick(at)).collect();
        }
        if self.state == State::Playing
            && self.since.is_none()
            && let Some((first, _)) = self.queue.front()
        {
            // Start, or resume after waiting for the decoder. A video may
            // not start at 0: the first frame shows straight away.
            if self.current.is_none() {
                self.base = self.base.max(*first);
            }
            self.since = Some(at);
        }
        let now = self.now(at);
        let mut retired = Vec::new();
        while let Some((_, frame)) = self.queue.pop_front_if(|(time, _)| *time <= now) {
            retired.extend(self.current.replace(frame));
        }
        if self.queue.is_empty() && self.since.is_some() {
            // Nothing to show next: wait here for the decoder (or the end).
            self.stop_clock(at);
        }
        retired
    }

    /// As [`Timeline::tick`], following the sound: `audio` is the media
    /// time of the sound coming out, when some is.
    pub fn tick_with(&mut self, at: Instant, audio: Option<Duration>) -> Vec<T> {
        if self.state == State::Playing
            && !self.preview
            && let Some(position) = audio
        {
            self.base = position;
            self.since = Some(at);
        }
        self.tick(at)
    }

    /// Continues from `to`: drops the queued frames (returned, for the caller
    /// to free) and shows the next pushed frame as soon as it arrives. The
    /// frame on screen stays until then.
    pub fn seek(&mut self, to: Duration) -> Vec<T> {
        self.base = to;
        self.since = None;
        self.ended = false;
        self.preview = true;
        self.queue.drain(..).map(|(_, f)| f).collect()
    }

    /// The media time at `at` (the playback position).
    pub fn position(&self, at: Instant) -> Duration {
        self.now(at)
    }

    /// Whether the last frame is on screen and no other follows.
    pub fn is_ended(&self) -> bool {
        self.ended && self.queue.is_empty() && !self.preview
    }

    /// The frame on screen.
    pub fn current(&self) -> Option<&T> {
        self.current.as_ref()
    }

    /// Takes every frame out, for the caller to free.
    pub fn drain(&mut self) -> Vec<T> {
        self.current.take().into_iter().chain(self.queue.drain(..).map(|(_, f)| f)).collect()
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Plays from where it is, or pauses there.
    pub fn toggle(&mut self, at: Instant) {
        self.state = match self.state {
            State::Playing => {
                self.stop_clock(at);
                State::Paused
            }
            State::Paused => State::Playing,
        };
    }

    /// Whether [`Timeline::tick`] needs calling again: playing and not past
    /// the last frame.
    pub fn is_running(&self) -> bool {
        self.state == State::Playing && !(self.ended && self.queue.is_empty())
    }

    /// The media time at `at`.
    fn now(&self, at: Instant) -> Duration {
        self.base + self.since.map_or(Duration::ZERO, |since| at.saturating_duration_since(since))
    }

    fn stop_clock(&mut self, at: Instant) {
        self.base = self.now(at);
        self.since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A timeline holding frames `0..n`, 100 ms apart.
    fn frames(n: u64) -> Timeline<u64> {
        let mut t = Timeline::default();
        for k in 0..n {
            t.push(ms(100 * k), k);
        }
        t
    }

    #[test]
    fn the_first_frame_shows_as_soon_as_it_arrives() {
        let start = Instant::now();
        let mut t = Timeline::default();
        assert!(t.tick(start).is_empty());
        assert_eq!(t.current(), None, "nothing decoded yet");
        t.push(ms(0), 0);
        t.push(ms(100), 1);
        // Arriving late doesn't make the frames late: the clock starts now.
        let later = start + ms(5000);
        t.tick(later);
        assert_eq!(t.current(), Some(&0));
        t.tick(later + ms(99));
        assert_eq!(t.current(), Some(&0));
        assert_eq!(t.tick(later + ms(100)), vec![0], "the old frame is handed back");
        assert_eq!(t.current(), Some(&1));
    }

    #[test]
    fn late_ticks_skip_to_the_latest_due_frame() {
        let start = Instant::now();
        let mut t = frames(4);
        t.tick(start);
        assert_eq!(t.tick(start + ms(250)), vec![0, 1], "frame 1 was never shown");
        assert_eq!(t.current(), Some(&2));
    }

    #[test]
    fn the_clock_waits_for_a_slow_decoder() {
        let start = Instant::now();
        let mut t = frames(2);
        t.tick(start);
        t.tick(start + ms(100));
        assert_eq!(t.current(), Some(&1));
        // Frame 2 (at 200 ms) arrives a second late.
        t.tick(start + ms(150));
        t.push(ms(200), 2);
        t.push(ms(300), 3);
        let arrived = start + ms(1150);
        t.tick(arrived);
        assert_eq!(t.current(), Some(&1), "the clock stopped at 100 ms, when the queue ran dry");
        t.tick(arrived + ms(99));
        assert_eq!(t.current(), Some(&1));
        t.tick(arrived + ms(100));
        assert_eq!(t.current(), Some(&2), "and resumed when frames came");
        t.tick(arrived + ms(199));
        assert_eq!(t.current(), Some(&2), "frame 3 isn't skipped");
    }

    #[test]
    fn pausing_freezes_the_picture_and_resumes_where_it_was() {
        let start = Instant::now();
        let mut t = frames(3);
        t.tick(start);
        t.tick(start + ms(50));
        t.toggle(start + ms(50));
        assert_eq!(t.state(), State::Paused);
        assert!(!t.is_running());
        t.tick(start + ms(5000));
        assert_eq!(t.current(), Some(&0), "paused");
        t.toggle(start + ms(5000));
        t.tick(start + ms(5000));
        t.tick(start + ms(5049));
        assert_eq!(t.current(), Some(&0), "50 ms of frame 0 were left");
        t.tick(start + ms(5050));
        assert_eq!(t.current(), Some(&1));
    }

    #[test]
    fn the_queue_is_bounded_and_the_end_stops_it() {
        let mut t = Timeline::default();
        for k in 0..QUEUE as u64 {
            assert!(t.wants_more());
            t.push(ms(k), k);
        }
        assert!(!t.wants_more(), "full");
        let start = Instant::now();
        t.tick(start);
        assert!(t.wants_more(), "a frame went on screen");
        t.end();
        assert!(!t.wants_more());
        assert!(t.is_running(), "frames are still queued");
        t.tick(start + ms(10));
        assert_eq!(t.current(), Some(&(QUEUE as u64 - 1)));
        assert!(!t.is_running(), "the last frame stays up");
    }

    #[test]
    fn a_video_starting_late_shows_its_first_frame_at_once() {
        let start = Instant::now();
        let mut t = Timeline::default();
        t.push(ms(500), 0);
        t.push(ms(600), 1);
        t.tick(start);
        assert_eq!(t.current(), Some(&0));
        t.tick(start + ms(100));
        assert_eq!(t.current(), Some(&1));
    }

    #[test]
    fn frames_never_go_back_in_time() {
        let start = Instant::now();
        let mut t = Timeline::default();
        t.push(ms(200), 0);
        t.push(ms(100), 1);
        t.push(ms(300), 2);
        assert_eq!(t.tick(start), vec![0], "frame 1 counts as 200 ms, not before 0");
        assert_eq!(t.current(), Some(&1));
        t.tick(start + ms(100));
        assert_eq!(t.current(), Some(&2));
    }

    #[test]
    fn with_sound_the_picture_follows_the_sound() {
        let start = Instant::now();
        let mut t = frames(5);
        t.tick_with(start, Some(ms(0)));
        assert_eq!(t.current(), Some(&0));
        // The device is behind the wall clock: the picture waits for it.
        t.tick_with(start + ms(250), Some(ms(150)));
        assert_eq!(t.current(), Some(&1));
        // And ahead of it: the picture keeps up.
        t.tick_with(start + ms(260), Some(ms(310)));
        assert_eq!(t.current(), Some(&3));
    }

    #[test]
    fn when_the_sound_stops_the_wall_clock_carries_on_from_it() {
        let start = Instant::now();
        let mut t = frames(6);
        t.tick_with(start, Some(ms(150)));
        assert_eq!(t.current(), Some(&1));
        // The sound track ended (or ran dry); the frames are there.
        t.tick_with(start + ms(100), None);
        assert_eq!(t.current(), Some(&2), "150 ms + 100 ms of wall time");
        t.tick_with(start + ms(300), None);
        assert_eq!(t.current(), Some(&4), "never frozen");
    }

    #[test]
    fn a_seek_while_paused_shows_the_new_picture_and_stays_paused() {
        let start = Instant::now();
        let mut t = frames(2);
        t.tick(start);
        t.toggle(start);
        assert_eq!(t.seek(ms(5000)), vec![1], "the queued frame is handed back");
        assert_eq!(t.current(), Some(&0), "the old picture stays until a new one comes");
        assert_eq!(t.position(start + ms(999)), ms(5000), "the position is the target");
        t.push(ms(5000), 50);
        t.push(ms(5100), 51);
        assert_eq!(t.tick(start + ms(10)), vec![0]);
        assert_eq!(t.current(), Some(&50), "a preview of where it went");
        t.tick(start + ms(500));
        assert_eq!(t.current(), Some(&50), "still paused");
        assert_eq!(t.state(), State::Paused);
    }

    #[test]
    fn a_seek_while_playing_continues_from_the_target() {
        let start = Instant::now();
        let mut t = frames(3);
        t.tick(start);
        t.seek(ms(2000));
        t.push(ms(2000), 20);
        t.push(ms(2100), 21);
        t.tick(start + ms(10));
        assert_eq!(t.current(), Some(&20));
        t.tick(start + ms(109));
        assert_eq!(t.current(), Some(&20));
        t.tick(start + ms(110));
        assert_eq!(t.current(), Some(&21));
    }

    #[test]
    fn a_seek_to_the_very_end_ends() {
        let start = Instant::now();
        let mut t = frames(2);
        t.tick(start);
        t.seek(ms(5000));
        // The decoder skipped every frame: only the end comes.
        t.end();
        t.tick(start + ms(10));
        assert!(t.is_ended(), "so play can start it over");
        assert!(!t.is_running());
    }

    #[test]
    fn a_preview_arriving_with_the_end_still_shows() {
        let start = Instant::now();
        let mut t = frames(1);
        t.tick(start);
        t.toggle(start);
        t.seek(ms(900));
        t.push(ms(900), 9);
        t.end();
        t.tick(start + ms(10));
        assert_eq!(t.current(), Some(&9));
        assert!(t.is_ended());
    }

    #[test]
    fn the_end_is_the_last_frame_with_nothing_after_it() {
        let start = Instant::now();
        let mut t = frames(2);
        t.end();
        t.tick(start);
        assert!(!t.is_ended(), "a frame is still queued");
        t.tick(start + ms(100));
        assert!(t.is_ended());
        assert_eq!(t.current(), Some(&1), "the last picture stays up");
        t.seek(Duration::ZERO);
        assert!(!t.is_ended(), "a seek starts it again");
    }

    #[test]
    fn inline_gifs_follow_the_setting_until_the_viewer_chooses() {
        use Autoplay::{Off, On};
        assert!(!gif_plays(Off, None), "off by default: wait for a click");
        assert!(gif_plays(On, None));
        for autoplay in [On, Off] {
            assert!(gif_plays(autoplay, Some(GifChoice::Play)));
            assert!(!gif_plays(autoplay, Some(GifChoice::Pause)));
        }
    }

    #[test]
    fn the_button_does_the_opposite_of_what_the_gif_does() {
        use Autoplay::{Off, On};
        assert_eq!(gif_toggled(Off, None), GifChoice::Play);
        assert_eq!(gif_toggled(On, None), GifChoice::Pause);
        assert_eq!(gif_toggled(Off, Some(GifChoice::Play)), GifChoice::Pause);
        assert_eq!(gif_toggled(On, Some(GifChoice::Pause)), GifChoice::Play);
    }

    #[test]
    fn draining_hands_back_every_frame() {
        let start = Instant::now();
        let mut t = frames(3);
        t.tick(start);
        let mut all = t.drain();
        all.sort();
        assert_eq!(all, vec![0, 1, 2]);
        assert_eq!(t.current(), None);
    }
}
