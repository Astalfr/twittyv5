//! Video decoding for the views. Each playback decodes on its own thread
//! (`twitty_media` blocks while it downloads and decodes) and hands frames
//! over as GPUI images through a bounded channel, so the UI thread only
//! swaps pictures.

use std::{
    sync::{
        Arc,
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    time::Duration,
};

use gpui_kit::{App, Global, RenderImage};
use image::{Frame, RgbaImage};
use twitty_media::{Decoder, MediaError, VideoFrame};

/// Frames the decoder thread may run ahead of the view (on top of
/// `state::playback::QUEUE`).
const CHANNEL: usize = 3;

/// What a playback sends to its view.
#[derive(Debug)]
pub enum VideoEvent {
    /// A picture to show at `time` from the start of the playback. Times
    /// only grow: a loop continues after the last frame instead of
    /// restarting at 0.
    Frame { time: Duration, image: Arc<RenderImage> },
    /// The video is over (never sent when it loops).
    Ended,
    /// It can't be played; the reason is for the log.
    Failed(String),
}

/// What happens at the end of a video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    Once,
    Loop,
}

/// Something that plays videos.
pub trait VideoSource: Send + Sync {
    /// Starts decoding `url`. Events arrive until the receiver is dropped,
    /// which also stops the decoding.
    fn open(&self, url: &str, repeat: Repeat) -> Receiver<VideoEvent>;
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
pub fn open(url: &str, repeat: Repeat, cx: &App) -> Option<Receiver<VideoEvent>> {
    cx.try_global::<Video>().map(|video| video.0.open(url, repeat))
}

/// Decodes with the system decoder (Media Foundation on Windows).
#[derive(Debug, Default)]
pub struct SystemVideo;

impl VideoSource for SystemVideo {
    fn open(&self, url: &str, repeat: Repeat) -> Receiver<VideoEvent> {
        let (tx, rx) = sync_channel(CHANNEL);
        let url = url.to_owned();
        let failed = tx.clone();
        let spawned = std::thread::Builder::new().name("video-decode".into()).spawn(move || {
            // The decoder holds COM objects: it lives and dies on this thread.
            match Decoder::open(&url) {
                Ok(mut decoder) => pump(&mut decoder, repeat, &tx),
                Err(e) => fail(&tx, &e),
            }
        });
        if let Err(e) = spawned {
            let _ = failed.try_send(VideoEvent::Failed(format!("no decoder thread: {e}")));
        }
        rx
    }
}

fn fail(tx: &SyncSender<VideoEvent>, e: &MediaError) {
    tracing::warn!("video playback failed: {e}");
    let _ = tx.send(VideoEvent::Failed(e.to_string()));
}

/// The decoder calls [`pump`] needs; a trait so the loop logic is tested
/// without a video file.
trait Frames {
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError>;
    fn rewind(&mut self) -> Result<(), MediaError>;
}

impl Frames for Decoder {
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        Decoder::next_frame(self)
    }

    fn rewind(&mut self) -> Result<(), MediaError> {
        Decoder::rewind(self)
    }
}

/// Sends every frame of `decoder` (forever, when looping) until the end, an
/// error, or the view hanging up.
fn pump(decoder: &mut impl Frames, repeat: Repeat, tx: &SyncSender<VideoEvent>) {
    // Where the current pass starts on the playback's clock.
    let mut offset = Duration::ZERO;
    loop {
        let mut end = None;
        loop {
            let frame = match decoder.next_frame() {
                Ok(Some(frame)) => frame,
                Ok(None) => break,
                Err(e) => return fail(tx, &e),
            };
            let time = offset + frame.time;
            end = Some(time + frame.duration);
            let Some(image) = render_image(frame) else {
                return fail(tx, &MediaError::Decode("a frame has the wrong size".into()));
            };
            if tx.send(VideoEvent::Frame { time, image }).is_err() {
                return; // The view is gone.
            }
        }
        // A pass without frames would loop forever on nothing.
        match (repeat, end) {
            (Repeat::Loop, Some(end)) => {
                if let Err(e) = decoder.rewind() {
                    return fail(tx, &e);
                }
                offset = end;
            }
            _ => {
                let _ = tx.send(VideoEvent::Ended);
                return;
            }
        }
    }
}

/// A frame as a one-frame GPUI image (GPUI takes BGRA, as decoded).
fn render_image(frame: VideoFrame) -> Option<Arc<RenderImage>> {
    let pixels = RgbaImage::from_raw(frame.width, frame.height, frame.bgra)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(pixels)])))
}

/// A source that plays the same frames for every URL, for tests.
#[cfg(test)]
#[derive(Debug)]
pub struct FakeVideo {
    /// Frame times; each picture is 2 × 2 pixels.
    pub frames: Vec<Duration>,
    /// Sent after the frames instead of the end, when set.
    pub failure: Option<String>,
    pub opened: std::sync::Mutex<Vec<String>>,
    /// Each playback's sender, to tell which views still listen.
    senders: std::sync::Mutex<Vec<SyncSender<VideoEvent>>>,
}

#[cfg(test)]
impl FakeVideo {
    /// Installs a source playing `frames` and returns it.
    pub fn install(frames: Vec<Duration>, cx: &mut App) -> Arc<Self> {
        let fake = Arc::new(Self {
            frames,
            failure: None,
            opened: Default::default(),
            senders: Default::default(),
        });
        cx.set_global(Video(fake.clone()));
        fake
    }

    /// Installs a source whose every playback fails before its first frame,
    /// as a 404 or a missing decoder does.
    pub fn install_failing(cx: &mut App) -> Arc<Self> {
        let fake = Arc::new(Self {
            frames: Vec::new(),
            failure: Some("not found".into()),
            opened: Default::default(),
            senders: Default::default(),
        });
        cx.set_global(Video(fake.clone()));
        fake
    }

    pub fn opened(&self) -> Vec<String> {
        self.opened.lock().map(|v| v.clone()).unwrap_or_default()
    }

    /// How many playbacks a view still holds (the others hung up, as a
    /// decoder thread would notice).
    pub fn listening(&self) -> usize {
        let Ok(senders) = self.senders.lock() else { return 0 };
        // A spare `Ended` is harmless to a playback that already ended.
        senders.iter().filter(|tx| tx.try_send(VideoEvent::Ended).is_ok()).count()
    }
}

#[cfg(test)]
impl VideoSource for FakeVideo {
    fn open(&self, url: &str, _: Repeat) -> Receiver<VideoEvent> {
        if let Ok(mut opened) = self.opened.lock() {
            opened.push(url.to_owned());
        }
        // Room for the probes of `listening` too.
        let (tx, rx) = sync_channel(self.frames.len() + 64);
        for &time in &self.frames {
            let image = Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(2, 2))]));
            let _ = tx.send(VideoEvent::Frame { time, image });
        }
        let _ = tx.send(match &self.failure {
            Some(why) => VideoEvent::Failed(why.clone()),
            None => VideoEvent::Ended,
        });
        if let Ok(mut senders) = self.senders.lock() {
            senders.push(tx);
        }
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `passes` passes of three 100 ms frames, then the end.
    struct Clip {
        passes: usize,
        next: u32,
        rewinds: usize,
    }

    impl Frames for Clip {
        fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
            if self.rewinds >= self.passes || self.next == 3 {
                return Ok(None);
            }
            let k = self.next;
            self.next += 1;
            Ok(Some(VideoFrame {
                time: Duration::from_millis(100) * k,
                duration: Duration::from_millis(100),
                width: 1,
                height: 1,
                bgra: vec![0, 0, 0, 255],
            }))
        }

        fn rewind(&mut self) -> Result<(), MediaError> {
            self.next = 0;
            self.rewinds += 1;
            Ok(())
        }
    }

    fn run(clip: &mut Clip, repeat: Repeat, cap: usize) -> Vec<String> {
        let (tx, rx) = sync_channel(cap);
        pump(clip, repeat, &tx);
        drop(tx);
        rx.iter()
            .map(|e| match e {
                VideoEvent::Frame { time, .. } => format!("{}", time.as_millis()),
                VideoEvent::Ended => "end".into(),
                VideoEvent::Failed(why) => format!("failed: {why}"),
            })
            .collect()
    }

    #[test]
    fn a_video_plays_once_then_ends() {
        let mut clip = Clip { passes: 5, next: 0, rewinds: 0 };
        assert_eq!(run(&mut clip, Repeat::Once, 16), ["0", "100", "200", "end"]);
        assert_eq!(clip.rewinds, 0);
    }

    #[test]
    fn a_loop_keeps_the_clock_going() {
        // Rewinds until the clip has no frames left (the third pass).
        let mut clip = Clip { passes: 2, next: 0, rewinds: 0 };
        assert_eq!(
            run(&mut clip, Repeat::Loop, 16),
            ["0", "100", "200", "300", "400", "500", "end"],
            "the second pass follows the first's last frame"
        );
    }

    #[test]
    fn hanging_up_stops_the_decoder() {
        let (tx, rx) = sync_channel(16);
        drop(rx);
        let mut clip = Clip { passes: usize::MAX, next: 0, rewinds: 0 };
        pump(&mut clip, Repeat::Loop, &tx);
        assert_eq!((clip.next, clip.rewinds), (1, 0), "stopped at the first send");
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
