//! The sound of a playing video, and the clock it keeps.
//!
//! The decoder thread queues decoded chunks in an [`AudioShared`]; an
//! [`AudioOut`] added to a rodio mixer plays them. The output device pulls
//! samples at its own pace, so the sample being played is the playback's
//! master clock ([`AudioShared::position`]); the view times the picture to it.
//!
//! [`AudioOut::next`] runs on the audio callback thread. It takes a whole
//! chunk at a time (one lock per chunk, not per sample), plays silence without
//! consuming while paused or when the queue is dry, and returns `None` once
//! stopped so the mixer drops it.

use std::{
    collections::VecDeque,
    num::NonZero,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// After this long without the device pulling a sample (it stopped: unplugged
/// headphones, a changed default device), the sound stops being the clock.
const STALE: Duration = Duration::from_millis(250);

/// How often (in samples) the output notes that the device still pulls.
const PULL_NOTE: usize = 256;

/// A process-wide origin, so instants fit in an atomic.
fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn since_epoch(at: Instant) -> u64 {
    u64::try_from(at.saturating_duration_since(epoch()).as_nanos()).unwrap_or(u64::MAX)
}

use twitty_media::{AudioChunk, AudioFormat};

/// Sound shared by the decoder thread, the output and the view.
#[derive(Debug, Default)]
pub struct AudioShared {
    format: OnceLock<AudioFormat>,
    queue: Mutex<VecDeque<(u64, AudioChunk)>>,
    /// Chunks of other generations (before the latest seek) are dropped.
    generation: AtomicU64,
    paused: AtomicBool,
    stopped: AtomicBool,
    /// The gain, as `f32` bits.
    volume: AtomicU32,
    /// Media time of the sample last played, in microseconds.
    position_us: AtomicU64,
    /// Whether sound is coming out (not paused, not starved).
    flowing: AtomicBool,
    /// When the device last pulled a sample, in nanoseconds since `epoch`.
    pulled_ns: AtomicU64,
}

impl AudioShared {
    pub fn new() -> Arc<Self> {
        let shared = Self::default();
        shared.volume.store(1.0f32.to_bits(), Ordering::Relaxed);
        Arc::new(shared)
    }

    /// Sets the format once the decoder knows it.
    pub fn set_format(&self, format: AudioFormat) {
        let _ = self.format.set(format);
    }

    /// Queues `chunk`, unless a seek made its generation stale.
    pub fn push(&self, generation: u64, chunk: AudioChunk) {
        if generation != self.generation.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut queue) = self.queue.lock() {
            queue.push_back((generation, chunk));
        }
    }

    /// How much sound is queued and not played yet.
    pub fn buffered(&self) -> Duration {
        let Some(format) = self.format.get() else { return Duration::ZERO };
        let samples =
            self.queue.lock().map(|q| q.iter().map(|(_, c)| c.samples.len()).sum()).unwrap_or(0);
        format.duration_of(samples)
    }

    /// Forgets the queued sound and continues from `to` with `generation`.
    pub fn seek(&self, generation: u64, to: Duration) {
        self.generation.store(generation, Ordering::Release);
        if let Ok(mut queue) = self.queue.lock() {
            queue.clear();
        }
        self.position_us.store(micros(to), Ordering::Relaxed);
        self.flowing.store(false, Ordering::Relaxed);
    }

    /// The media time of the sound coming out, while some is.
    pub fn position(&self) -> Option<Duration> {
        self.position_at(Instant::now())
    }

    /// [`AudioShared::position`] as seen at `now`: `None` once the device
    /// hasn't pulled a sample for a while.
    pub fn position_at(&self, now: Instant) -> Option<Duration> {
        let pulled = self.pulled_ns.load(Ordering::Relaxed);
        let fresh = since_epoch(now).saturating_sub(pulled) <= since_nanos(STALE);
        (fresh && self.flowing.load(Ordering::Relaxed) && !self.paused.load(Ordering::Relaxed))
            .then(|| Duration::from_micros(self.position_us.load(Ordering::Relaxed)))
    }

    fn note_pull(&self) {
        self.pulled_ns.store(since_epoch(Instant::now()), Ordering::Relaxed);
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// Sets the gain (0 is silent, 1 as decoded).
    pub fn set_volume(&self, volume: f32) {
        self.volume.store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Ends the output: the mixer drops it.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }

    /// The next chunk of the current generation, dropping stale ones.
    fn take(&self) -> Option<(u64, AudioChunk)> {
        let generation = self.generation.load(Ordering::Acquire);
        let mut queue = self.queue.lock().ok()?;
        while let Some((g, chunk)) = queue.pop_front() {
            if g == generation {
                return Some((g, chunk));
            }
        }
        None
    }
}

fn since_nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

fn micros(d: Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

/// Plays an [`AudioShared`]'s queue; a rodio source.
#[derive(Debug)]
pub struct AudioOut {
    shared: Arc<AudioShared>,
    format: AudioFormat,
    chunk: Option<(u64, AudioChunk)>,
    /// The next sample of `chunk`.
    index: usize,
}

impl AudioOut {
    pub fn new(shared: Arc<AudioShared>, format: AudioFormat) -> Self {
        Self { shared, format, chunk: None, index: 0 }
    }
}

impl Iterator for AudioOut {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let shared = &self.shared;
        if shared.stopped.load(Ordering::Relaxed) {
            return None;
        }
        if shared.paused.load(Ordering::Relaxed) {
            return Some(0.0);
        }
        let generation = shared.generation.load(Ordering::Acquire);
        let current = self
            .chunk
            .as_ref()
            .is_some_and(|(g, c)| *g == generation && self.index < c.samples.len());
        if !current {
            self.chunk = shared.take();
            self.index = 0;
            shared.note_pull();
        }
        let Some((_, chunk)) = &self.chunk else {
            shared.flowing.store(false, Ordering::Relaxed);
            return Some(0.0);
        };
        let sample = chunk.samples.get(self.index).copied().unwrap_or(0.0);
        self.index += 1;
        if self.index.is_multiple_of(PULL_NOTE) {
            shared.note_pull();
        }
        if self.index.is_multiple_of(usize::from(self.format.channels.max(1))) {
            let at = chunk.time + self.format.duration_of(self.index);
            shared.position_us.store(micros(at), Ordering::Relaxed);
        }
        shared.flowing.store(true, Ordering::Relaxed);
        Some(sample * f32::from_bits(shared.volume.load(Ordering::Relaxed)))
    }
}

impl rodio::Source for AudioOut {
    fn current_span_len(&self) -> Option<usize> {
        // The format never changes.
        None
    }

    fn channels(&self) -> rodio::ChannelCount {
        NonZero::new(self.format.channels).unwrap_or(NonZero::<u16>::MIN)
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        NonZero::new(self.format.sample_rate).unwrap_or(NonZero::<u32>::MIN)
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 stereo frames per second, so each frame is 100 ms.
    const FORMAT: AudioFormat = AudioFormat { sample_rate: 10, channels: 2 };

    fn chunk(time_ms: u64, samples: &[f32]) -> AudioChunk {
        AudioChunk { time: Duration::from_millis(time_ms), samples: samples.to_vec() }
    }

    fn setup() -> (Arc<AudioShared>, AudioOut) {
        let shared = AudioShared::new();
        shared.set_format(FORMAT);
        let out = AudioOut::new(Arc::clone(&shared), FORMAT);
        (shared, out)
    }

    fn pull(out: &mut AudioOut, n: usize) -> Vec<f32> {
        (0..n).map_while(|_| out.next()).collect()
    }

    #[test]
    fn queued_sound_plays_in_order_and_sets_the_clock() {
        let (shared, mut out) = setup();
        shared.push(0, chunk(1000, &[0.1, 0.2, 0.3, 0.4]));
        assert_eq!(shared.position(), None, "nothing played yet");
        assert_eq!(pull(&mut out, 2), [0.1, 0.2]);
        assert_eq!(shared.position(), Some(Duration::from_millis(1100)), "one frame in");
        assert_eq!(pull(&mut out, 2), [0.3, 0.4]);
        assert_eq!(shared.position(), Some(Duration::from_millis(1200)));
    }

    #[test]
    fn a_dry_queue_plays_silence_and_stops_the_clock() {
        let (shared, mut out) = setup();
        shared.push(0, chunk(0, &[0.5, 0.5]));
        pull(&mut out, 2);
        assert_eq!(pull(&mut out, 4), [0.0; 4]);
        assert_eq!(shared.position(), None, "starved: the view falls back to wall time");
        shared.push(0, chunk(100, &[0.7, 0.7]));
        assert_eq!(pull(&mut out, 2), [0.7, 0.7], "and picks up where it was");
    }

    #[test]
    fn pausing_plays_silence_without_consuming() {
        let (shared, mut out) = setup();
        shared.push(0, chunk(0, &[0.1, 0.2, 0.3, 0.4]));
        pull(&mut out, 2);
        shared.set_paused(true);
        assert_eq!(pull(&mut out, 6), [0.0; 6]);
        assert_eq!(shared.position(), None, "the clock is not running");
        shared.set_paused(false);
        assert_eq!(pull(&mut out, 2), [0.3, 0.4], "nothing was skipped");
    }

    #[test]
    fn the_volume_is_a_gain() {
        let (shared, mut out) = setup();
        shared.push(0, chunk(0, &[0.8, -0.8, 0.8, -0.8]));
        shared.set_volume(0.5);
        assert_eq!(pull(&mut out, 2), [0.4, -0.4]);
        shared.set_volume(0.0);
        assert_eq!(pull(&mut out, 2), [0.0, 0.0], "muted");
        shared.set_volume(7.0);
        assert_eq!(f32::from_bits(shared.volume.load(Ordering::Relaxed)), 1.0, "clamped");
    }

    #[test]
    fn a_seek_drops_the_old_sound_even_mid_chunk() {
        let (shared, mut out) = setup();
        shared.push(0, chunk(0, &[0.1, 0.1, 0.2, 0.2]));
        shared.push(0, chunk(200, &[0.3, 0.3]));
        pull(&mut out, 2);
        shared.seek(1, Duration::from_secs(5));
        shared.push(0, chunk(400, &[0.9, 0.9]));
        assert_eq!(pull(&mut out, 2), [0.0, 0.0], "old and late-arriving stale chunks are gone");
        shared.push(1, chunk(5000, &[0.6, 0.6]));
        assert_eq!(pull(&mut out, 2), [0.6, 0.6]);
        assert_eq!(shared.position(), Some(Duration::from_millis(5100)));
    }

    #[test]
    fn a_device_that_stops_pulling_stops_being_the_clock() {
        let (shared, mut out) = setup();
        shared.push(0, chunk(0, &[0.1; 8]));
        pull(&mut out, 2);
        let now = Instant::now();
        assert!(shared.position_at(now).is_some());
        assert_eq!(shared.position_at(now + Duration::from_secs(1)), None, "nothing pulled since");
    }

    #[test]
    fn buffered_counts_the_queue_and_stop_ends_the_source() {
        let (shared, mut out) = setup();
        shared.push(0, chunk(0, &[0.0; 8]));
        assert_eq!(shared.buffered(), Duration::from_millis(400));
        shared.stop();
        assert_eq!(out.next(), None);
    }
}
