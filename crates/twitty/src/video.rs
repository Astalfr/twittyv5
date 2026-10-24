//! Video decoding for the views. Each playback decodes on its own thread
//! (`twitty_media` blocks while it downloads and decodes) and hands frames
//! over as GPUI images through a bounded channel, so the UI thread only
//! swaps pictures. The sound goes straight from that thread to the output
//! device (`crate::audio`), never through the UI.
//!
//! The view steers a playback through its [`Control`]: seek, pause, volume,
//! and the sound's clock. The decoder thread never blocks without watching
//! for commands, so a seek is seen even while the view isn't taking frames
//! (paused), and a view that goes away stops it.

use std::{
    sync::{
        Arc,
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel},
    },
    time::Duration,
};

use gpui_kit::{App, Global, RenderImage};
use image::{Frame, RgbaImage};
use twitty_media::{AudioChunk, Decoder, MediaError, Packet, Tracks, VideoFrame};

use crate::audio::{AudioOut, AudioShared};

/// Frames the decoder thread may run ahead of the view (on top of
/// `state::playback::QUEUE`).
const CHANNEL: usize = 3;

/// The most sound queued ahead of the output. Beyond it the decoder waits,
/// so a file whose picture ends first can't flood memory with its sound.
const AUDIO_AHEAD: Duration = Duration::from_secs(2);

/// How often a waiting decoder thread looks for commands.
const POLL: Duration = Duration::from_millis(5);

/// What a playback sends to its view.
#[derive(Debug)]
pub enum VideoEvent {
    /// The file is open.
    Opened { duration: Option<Duration>, sound: bool },
    /// A picture to show at `time` from the start of the playback, decoded
    /// after the seek `generation` asked for (0 before any). When looping,
    /// times only grow: a loop continues after the last frame.
    Frame { generation: u64, time: Duration, image: Arc<RenderImage> },
    /// The video is over (never sent when it loops). A seek starts it again.
    Ended { generation: u64 },
    /// It can't be played; the reason is for the log.
    Failed(String),
}

/// What happens at the end of a video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    Once,
    Loop,
}

/// Whether a playback plays the file's sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    On,
    Off,
}

/// Steers a playback from the view.
pub trait Control: Send + Sync {
    /// The media time of the sound coming out, while some is: the clock the
    /// picture follows. `None` without sound, while paused or starved.
    fn audio_position(&self) -> Option<Duration>;
    fn set_paused(&self, paused: bool);
    /// Continues from `to`. Events decoded after it carry `generation`.
    fn seek(&self, to: Duration, generation: u64);
    /// Sets the sound's gain (0 is silent).
    fn set_volume(&self, volume: f32);
}

/// A started playback.
pub struct Playback {
    pub events: Receiver<VideoEvent>,
    pub control: Arc<dyn Control>,
}

impl std::fmt::Debug for Playback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Playback").finish_non_exhaustive()
    }
}

/// Something that plays videos.
pub trait VideoSource: Send + Sync {
    /// Starts decoding `url`. Dropping the playback stops the decoding.
    fn open(&self, url: &str, repeat: Repeat, sound: Sound) -> Playback;
}

/// The app's video source, as a GPUI global. Without one (tests, by
/// default) videos show their poster only.
#[derive(Clone)]
pub struct Video(pub Arc<dyn VideoSource>);

impl Global for Video {}

impl std::fmt::Debug for Video {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Video").finish_non_exhaustive()
    }
}

/// Starts playing `url`, if a video source is installed.
pub fn open(url: &str, repeat: Repeat, sound: Sound, cx: &App) -> Option<Playback> {
    cx.try_global::<Video>().map(|video| video.0.open(url, repeat, sound))
}

/// Decodes with the system decoder (Media Foundation on Windows) and plays
/// the sound on the default output device.
#[derive(Debug, Default)]
pub struct SystemVideo;

impl VideoSource for SystemVideo {
    fn open(&self, url: &str, repeat: Repeat, sound: Sound) -> Playback {
        let (tx, rx) = sync_channel(CHANNEL);
        let (commands_tx, commands) = mpsc::channel();
        let audio = AudioShared::new();
        let control = Arc::new(SystemControl { commands: commands_tx, audio: Arc::clone(&audio) });
        let url = url.to_owned();
        let failed = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || run(&url, repeat, sound, &tx, &commands, &audio));
        if let Err(e) = spawned {
            let _ = failed.try_send(VideoEvent::Failed(format!("no decoder thread: {e}")));
        }
        Playback { events: rx, control }
    }
}

#[derive(Debug)]
enum Command {
    Seek { to: Duration, generation: u64 },
}

struct SystemControl {
    commands: mpsc::Sender<Command>,
    audio: Arc<AudioShared>,
}

impl Drop for SystemControl {
    fn drop(&mut self) {
        // The view is gone: silence now, even if the decoder thread is still
        // blocked in a download and exits later.
        self.audio.stop();
    }
}

impl Control for SystemControl {
    fn audio_position(&self) -> Option<Duration> {
        self.audio.position()
    }

    fn set_paused(&self, paused: bool) {
        self.audio.set_paused(paused);
    }

    fn seek(&self, to: Duration, generation: u64) {
        // Silence the old sound now; the thread decodes the new one.
        self.audio.seek(generation, to);
        let _ = self.commands.send(Command::Seek { to, generation });
    }

    fn set_volume(&self, volume: f32) {
        self.audio.set_volume(volume);
    }
}

/// The decoder thread: opens, starts the sound, then pumps until the view
/// hangs up. The decoder and the output device live and die here.
fn run(
    url: &str,
    repeat: Repeat,
    sound: Sound,
    tx: &SyncSender<VideoEvent>,
    commands: &Receiver<Command>,
    audio: &Arc<AudioShared>,
) {
    let tracks = match sound {
        Sound::On => Tracks::VideoAndAudio,
        Sound::Off => Tracks::Video,
    };
    let mut decoder = match Decoder::open(url, tracks) {
        Ok(decoder) => decoder,
        Err(e) => return fail(tx, &e),
    };
    let device = decoder.audio_format().and_then(|format| {
        audio.set_format(format);
        let mut device = match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(device) => device,
            Err(e) => {
                tracing::warn!("no audio output, the video plays silently: {e}");
                return None;
            }
        };
        // Rodio otherwise prints to stderr when the device is dropped.
        device.log_on_drop(false);
        device.mixer().add(AudioOut::new(Arc::clone(audio), format));
        Some(device)
    });
    let opened = VideoEvent::Opened { duration: decoder.duration(), sound: device.is_some() };
    if tx.send(opened).is_ok() {
        pump(&mut decoder, repeat, tx, commands, device.as_ref().map(|_| audio.as_ref()));
    }
    audio.stop();
    drop(device);
}

fn fail(tx: &SyncSender<VideoEvent>, e: &MediaError) {
    tracing::warn!("video playback failed: {e}");
    let _ = tx.send(VideoEvent::Failed(e.to_string()));
}

/// The decoder calls [`pump`] needs; a trait so the loop logic is tested
/// without a video file.
trait Packets {
    fn next_packet(&mut self) -> Result<Option<Packet>, MediaError>;
    fn seek(&mut self, to: Duration) -> Result<(), MediaError>;
}

impl Packets for Decoder {
    fn next_packet(&mut self) -> Result<Option<Packet>, MediaError> {
        Decoder::next_packet(self)
    }

    fn seek(&mut self, to: Duration) -> Result<(), MediaError> {
        Decoder::seek(self, to)
    }
}

/// Where the decoder is on the playback's clock.
#[derive(Debug, Default)]
struct Pass {
    generation: u64,
    /// Where the current pass starts (grows with each loop).
    offset: Duration,
    /// The end of the last frame sent, on the playback's clock.
    end: Option<Duration>,
}

/// Why sending stopped short.
enum Interrupted {
    Seek { to: Duration, generation: u64 },
    HungUp,
}

/// Sends `event`, watching for commands while the channel is full.
fn send(
    tx: &SyncSender<VideoEvent>,
    commands: &Receiver<Command>,
    mut event: VideoEvent,
) -> Result<(), Interrupted> {
    loop {
        match tx.try_send(event) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => return Err(Interrupted::HungUp),
            Err(TrySendError::Full(back)) => event = back,
        }
        wait(commands)?;
    }
}

/// Waits a moment for a command; a seek interrupts.
fn wait(commands: &Receiver<Command>) -> Result<(), Interrupted> {
    match commands.recv_timeout(POLL) {
        Ok(Command::Seek { to, generation }) => Err(Interrupted::Seek { to, generation }),
        Err(RecvTimeoutError::Timeout) => Ok(()),
        Err(RecvTimeoutError::Disconnected) => Err(Interrupted::HungUp),
    }
}

/// Sends every frame (and queues every chunk of sound) until the view hangs
/// up. At the end it loops, or reports it and waits for a seek.
fn pump(
    decoder: &mut impl Packets,
    repeat: Repeat,
    tx: &SyncSender<VideoEvent>,
    commands: &Receiver<Command>,
    audio: Option<&AudioShared>,
) {
    let mut pass = Pass::default();
    let mut pending = None;
    loop {
        let seek = pending.take().or_else(|| match commands.try_recv() {
            Ok(Command::Seek { to, generation }) => Some((to, generation)),
            Err(_) => None,
        });
        if let Some((to, generation)) = seek {
            if let Err(e) = decoder.seek(to) {
                return fail(tx, &e);
            }
            pass = Pass { generation, ..Pass::default() };
        }
        let step = match decoder.next_packet() {
            Err(e) => return fail(tx, &e),
            Ok(Some(Packet::Video(frame))) => send_frame(frame, &mut pass, tx, commands),
            Ok(Some(Packet::Audio(chunk))) => match audio {
                Some(audio) => queue_sound(chunk, &pass, audio, commands),
                None => Ok(()),
            },
            Ok(None) => match (repeat, pass.end) {
                // A pass without frames would loop forever on nothing.
                (Repeat::Loop, Some(end)) => {
                    if let Err(e) = decoder.seek(Duration::ZERO) {
                        return fail(tx, &e);
                    }
                    pass = Pass { offset: end, end: None, ..pass };
                    Ok(())
                }
                _ => send(tx, commands, VideoEvent::Ended { generation: pass.generation })
                    .and_then(|()| match commands.recv() {
                        Ok(Command::Seek { to, generation }) => {
                            Err(Interrupted::Seek { to, generation })
                        }
                        Err(_) => Err(Interrupted::HungUp),
                    }),
            },
        };
        match step {
            Ok(()) => {}
            Err(Interrupted::Seek { to, generation }) => pending = Some((to, generation)),
            Err(Interrupted::HungUp) => return,
        }
    }
}

fn send_frame(
    frame: VideoFrame,
    pass: &mut Pass,
    tx: &SyncSender<VideoEvent>,
    commands: &Receiver<Command>,
) -> Result<(), Interrupted> {
    let time = pass.offset + frame.time;
    pass.end = Some(time + frame.duration);
    let Some(image) = render_image(frame) else {
        fail(tx, &MediaError::Decode("a frame has the wrong size".into()));
        return Err(Interrupted::HungUp);
    };
    send(tx, commands, VideoEvent::Frame { generation: pass.generation, time, image })
}

fn queue_sound(
    mut chunk: AudioChunk,
    pass: &Pass,
    audio: &AudioShared,
    commands: &Receiver<Command>,
) -> Result<(), Interrupted> {
    while audio.buffered() > AUDIO_AHEAD {
        wait(commands)?;
    }
    chunk.time += pass.offset;
    audio.push(pass.generation, chunk);
    Ok(())
}

/// A frame as a one-frame GPUI image (GPUI takes BGRA, as decoded).
fn render_image(frame: VideoFrame) -> Option<Arc<RenderImage>> {
    let pixels = RgbaImage::from_raw(frame.width, frame.height, frame.bgra)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(pixels)])))
}

/// A playback's control that records what it was asked, for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct FakeControl {
    pub seeks: std::sync::Mutex<Vec<(Duration, u64)>>,
    pub paused: std::sync::Mutex<Option<bool>>,
    pub volume: std::sync::Mutex<Option<f32>>,
}

#[cfg(test)]
impl FakeControl {
    pub fn seeks(&self) -> Vec<(Duration, u64)> {
        self.seeks.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn paused(&self) -> Option<bool> {
        self.paused.lock().ok().and_then(|p| *p)
    }

    pub fn volume(&self) -> Option<f32> {
        self.volume.lock().ok().and_then(|v| *v)
    }
}

#[cfg(test)]
impl Control for FakeControl {
    fn audio_position(&self) -> Option<Duration> {
        None
    }

    fn set_paused(&self, paused: bool) {
        if let Ok(mut p) = self.paused.lock() {
            *p = Some(paused);
        }
    }

    fn seek(&self, to: Duration, generation: u64) {
        if let Ok(mut seeks) = self.seeks.lock() {
            seeks.push((to, generation));
        }
    }

    fn set_volume(&self, volume: f32) {
        if let Ok(mut v) = self.volume.lock() {
            *v = Some(volume);
        }
    }
}

/// A source that plays the same frames for every URL, for tests.
#[cfg(test)]
#[derive(Debug)]
pub struct FakeVideo {
    /// Frame times; each picture is 2 × 2 pixels.
    pub frames: Vec<Duration>,
    /// Sent after the frames instead of the end, when set.
    pub failure: Option<String>,
    /// The length `Opened` reports.
    pub duration: Option<Duration>,
    pub opened: std::sync::Mutex<Vec<(String, Sound)>>,
    controls: std::sync::Mutex<Vec<Arc<FakeControl>>>,
    /// Each playback's sender, to tell which views still listen.
    senders: std::sync::Mutex<Vec<SyncSender<VideoEvent>>>,
}

#[cfg(test)]
impl FakeVideo {
    fn new(frames: Vec<Duration>, failure: Option<String>) -> Self {
        Self {
            frames,
            failure,
            duration: None,
            opened: Default::default(),
            controls: Default::default(),
            senders: Default::default(),
        }
    }

    /// Installs a source playing `frames` and returns it.
    pub fn install(frames: Vec<Duration>, cx: &mut App) -> Arc<Self> {
        Self::install_as(Self::new(frames, None), cx)
    }

    /// Installs a source playing `frames` of a video `duration` long.
    pub fn install_with_duration(
        frames: Vec<Duration>,
        duration: Duration,
        cx: &mut App,
    ) -> Arc<Self> {
        Self::install_as(Self { duration: Some(duration), ..Self::new(frames, None) }, cx)
    }

    /// Installs a source whose every playback fails before its first frame,
    /// as a 404 or a missing decoder does.
    pub fn install_failing(cx: &mut App) -> Arc<Self> {
        Self::install_as(Self::new(Vec::new(), Some("not found".into())), cx)
    }

    fn install_as(fake: Self, cx: &mut App) -> Arc<Self> {
        let fake = Arc::new(fake);
        cx.set_global(Video(fake.clone()));
        fake
    }

    /// The URLs opened so far.
    pub fn opened(&self) -> Vec<String> {
        self.opened.lock().map(|v| v.iter().map(|(u, _)| u.clone()).collect()).unwrap_or_default()
    }

    /// Whether each playback asked for sound.
    pub fn sounds(&self) -> Vec<Sound> {
        self.opened.lock().map(|v| v.iter().map(|(_, s)| *s).collect()).unwrap_or_default()
    }

    /// The control of the `n`th playback.
    pub fn control(&self, n: usize) -> Option<Arc<FakeControl>> {
        self.controls.lock().ok()?.get(n).cloned()
    }

    /// How many playbacks a view still holds (the others hung up, as a
    /// decoder thread would notice).
    pub fn listening(&self) -> usize {
        let Ok(senders) = self.senders.lock() else { return 0 };
        // A probe of no generation, which every view ignores.
        senders
            .iter()
            .filter(|tx| tx.try_send(VideoEvent::Ended { generation: u64::MAX }).is_ok())
            .count()
    }
}

#[cfg(test)]
impl VideoSource for FakeVideo {
    fn open(&self, url: &str, repeat: Repeat, sound: Sound) -> Playback {
        if let Ok(mut opened) = self.opened.lock() {
            opened.push((url.to_owned(), sound));
        }
        // Room for the probes of `listening` too.
        let (tx, rx) = sync_channel(self.frames.len() + 64);
        let _ = tx.send(VideoEvent::Opened { duration: self.duration, sound: sound == Sound::On });
        for &time in &self.frames {
            let image = Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(2, 2))]));
            let _ = tx.send(VideoEvent::Frame { generation: 0, time, image });
        }
        // A loop never ends; a failure ends it either way.
        match (&self.failure, repeat) {
            (Some(why), _) => drop(tx.send(VideoEvent::Failed(why.clone()))),
            (None, Repeat::Once) => drop(tx.send(VideoEvent::Ended { generation: 0 })),
            (None, Repeat::Loop) => {}
        }
        if let Ok(mut senders) = self.senders.lock() {
            senders.push(tx);
        }
        let control = Arc::new(FakeControl::default());
        if let Ok(mut controls) = self.controls.lock() {
            controls.push(Arc::clone(&control));
        }
        Playback { events: rx, control }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `passes` passes of three 100 ms frames (each followed by its sound),
    /// then the end. Seeks go to the frame at or after the target.
    struct Clip {
        passes: usize,
        next: u32,
        rewinds: usize,
        sound_next: bool,
        /// Set once the second frame is handed out.
        second: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Clip {
        fn new(passes: usize) -> Self {
            Self { passes, next: 0, rewinds: 0, sound_next: false, second: Arc::default() }
        }
    }

    impl Packets for Clip {
        fn next_packet(&mut self) -> Result<Option<Packet>, MediaError> {
            if self.rewinds >= self.passes || self.next == 3 {
                return Ok(None);
            }
            let k = self.next;
            let time = Duration::from_millis(100) * k;
            if self.sound_next {
                self.sound_next = false;
                self.next += 1;
                return Ok(Some(Packet::Audio(AudioChunk { time, samples: vec![0.5; 2] })));
            }
            self.sound_next = true;
            if k == 1 {
                self.second.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(Some(Packet::Video(VideoFrame {
                time,
                duration: Duration::from_millis(100),
                width: 1,
                height: 1,
                bgra: vec![0, 0, 0, 255],
            })))
        }

        fn seek(&mut self, to: Duration) -> Result<(), MediaError> {
            if to.is_zero() {
                self.rewinds += 1;
            }
            self.next = (to.as_millis() / 100) as u32;
            self.sound_next = false;
            Ok(())
        }
    }

    fn describe(e: VideoEvent) -> String {
        match e {
            VideoEvent::Opened { .. } => "opened".into(),
            VideoEvent::Frame { generation, time, .. } => {
                format!("{}@{generation}", time.as_millis())
            }
            VideoEvent::Ended { generation } => format!("end@{generation}"),
            VideoEvent::Failed(why) => format!("failed: {why}"),
        }
    }

    /// Runs the pump to the end; `commands` is dropped when the clip ends so
    /// a `Once` pump stops waiting.
    fn run(clip: &mut Clip, repeat: Repeat, audio: Option<&AudioShared>) -> Vec<String> {
        let (tx, rx) = sync_channel(64);
        let (commands_tx, commands) = mpsc::channel();
        drop(commands_tx);
        pump(clip, repeat, &tx, &commands, audio);
        drop(tx);
        rx.iter().map(describe).collect()
    }

    #[test]
    fn a_video_plays_once_then_ends() {
        let mut clip = Clip::new(5);
        assert_eq!(run(&mut clip, Repeat::Once, None), ["0@0", "100@0", "200@0", "end@0"]);
        assert_eq!(clip.rewinds, 0);
    }

    #[test]
    fn a_loop_keeps_the_clock_going() {
        // Rewinds until the clip has no frames left (the third pass).
        let mut clip = Clip::new(2);
        assert_eq!(
            run(&mut clip, Repeat::Loop, None),
            ["0@0", "100@0", "200@0", "300@0", "400@0", "500@0", "end@0"],
            "the second pass follows the first's last frame"
        );
    }

    #[test]
    fn the_sound_is_queued_beside_the_frames() {
        let audio = AudioShared::new();
        audio.set_format(twitty_media::AudioFormat { sample_rate: 10, channels: 2 });
        let mut clip = Clip::new(1);
        run(&mut clip, Repeat::Once, Some(&audio));
        assert_eq!(audio.buffered(), Duration::from_millis(300), "three chunks of one frame");
    }

    #[test]
    fn hanging_up_stops_the_decoder() {
        let (tx, rx) = sync_channel(16);
        drop(rx);
        let (_commands_tx, commands) = mpsc::channel();
        let mut clip = Clip::new(usize::MAX);
        pump(&mut clip, Repeat::Loop, &tx, &commands, None);
        assert_eq!((clip.next, clip.rewinds), (0, 0), "stopped at the first send");
    }

    #[test]
    fn a_seek_reaches_a_decoder_blocked_on_a_full_channel() {
        // The view takes nothing (paused): the channel fills at once.
        let (tx, rx) = sync_channel(1);
        let (commands_tx, commands) = mpsc::channel();
        let mut clip = Clip::new(1);
        let second = Arc::clone(&clip.second);
        let pump = std::thread::spawn(move || {
            pump(&mut clip, Repeat::Once, &tx, &commands, None);
        });
        // Frame 0 fills the channel; frame 1 is decoded and can't be sent.
        let waited = std::time::Instant::now();
        while !second.load(std::sync::atomic::Ordering::SeqCst) {
            assert!(waited.elapsed() < Duration::from_secs(5), "the pump never got to frame 1");
            std::thread::yield_now();
        }
        commands_tx.send(Command::Seek { to: Duration::from_millis(200), generation: 1 }).unwrap();
        let first = describe(rx.recv().unwrap());
        assert_eq!(first, "0@0", "the frame queued before the seek");
        let rest: Vec<String> =
            std::iter::from_fn(|| rx.recv().ok().map(describe)).take(2).collect();
        assert_eq!(rest, ["200@1", "end@1"], "the frame it was stuck on is dropped");
        drop(commands_tx);
        drop(rx);
        pump.join().unwrap();
    }

    #[test]
    fn after_the_end_a_seek_plays_again() {
        let (tx, rx) = sync_channel(16);
        let (commands_tx, commands) = mpsc::channel();
        let pump = std::thread::spawn(move || {
            let mut clip = Clip::new(9);
            pump(&mut clip, Repeat::Once, &tx, &commands, None);
        });
        let first: Vec<String> =
            std::iter::from_fn(|| rx.recv().ok().map(describe)).take(4).collect();
        assert_eq!(first.last().map(String::as_str), Some("end@0"));
        commands_tx.send(Command::Seek { to: Duration::ZERO, generation: 1 }).unwrap();
        let again: Vec<String> =
            std::iter::from_fn(|| rx.recv().ok().map(describe)).take(4).collect();
        assert_eq!(again, ["0@1", "100@1", "200@1", "end@1"]);
        drop(commands_tx);
        pump.join().unwrap();
    }

    #[test]
    fn dropping_the_control_silences_the_sound_at_once() {
        let audio = AudioShared::new();
        let format = twitty_media::AudioFormat { sample_rate: 10, channels: 2 };
        audio.set_format(format);
        audio.push(0, AudioChunk { time: Duration::ZERO, samples: vec![0.5; 8] });
        let mut out = AudioOut::new(Arc::clone(&audio), format);
        let control = SystemControl { commands: mpsc::channel().0, audio: Arc::clone(&audio) };
        assert_eq!(out.next(), Some(0.5));
        drop(control);
        assert_eq!(out.next(), None, "the mixer drops the source");
    }

    #[test]
    fn a_frame_of_the_wrong_size_fails_the_playback() {
        let frame = VideoFrame {
            time: Duration::ZERO,
            duration: Duration::ZERO,
            width: 2,
            height: 2,
            bgra: vec![0; 4],
        };
        assert!(render_image(frame).is_none());
    }
}
