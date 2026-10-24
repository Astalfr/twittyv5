//! Video decoding for Twitty.
//!
//! [`Decoder`] reads a video file or URL and hands out its frames as BGRA
//! pixels, the layout GPUI's `RenderImage` takes. On Windows it uses Media
//! Foundation (the `mf` module, the only unsafe code in the workspace); on
//! other platforms [`Decoder::open`] returns [`MediaError::Unsupported`].
//!
//! A decoder belongs to the thread that opened it (it holds COM objects), so
//! it is not `Send`: open it on the thread that reads from it.

#![cfg_attr(not(windows), forbid(unsafe_code))]

use std::time::Duration;

#[cfg(windows)]
mod mf;

/// One decoded picture.
#[derive(Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// When the frame is shown, from the start of the video.
    pub time: Duration,
    /// How long it stays on screen (zero when the file doesn't say).
    pub duration: Duration,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height` pixels, top row first, as B, G, R, A bytes with
    /// opaque alpha.
    pub bgra: Vec<u8>,
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("time", &self.time)
            .field("duration", &self.duration)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// Why a video can't be played.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MediaError {
    /// This platform has no decoder.
    #[error("video playback is not supported on this platform")]
    Unsupported,
    /// The system's media stack is missing or failed to start (e.g. Windows
    /// N editions without the Media Feature Pack).
    #[error("the system media stack is unavailable: {0}")]
    Unavailable(String),
    /// The file or URL couldn't be opened, or has no video track.
    #[error("could not open the video: {0}")]
    Open(String),
    /// Reading or converting a frame failed.
    #[error("could not decode the video: {0}")]
    Decode(String),
}

/// Which tracks a [`Decoder`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tracks {
    /// The picture only (GIFs, which have no sound).
    Video,
    /// The picture and, when the file has one, the sound.
    VideoAndAudio,
}

/// The shape of decoded sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    /// Frames (one sample per channel) per second.
    pub sample_rate: u32,
    /// Interleaved channels.
    pub channels: u16,
}

impl AudioFormat {
    /// How long `samples` interleaved samples last.
    pub fn duration_of(self, samples: usize) -> Duration {
        let frames = samples / usize::from(self.channels.max(1));
        Duration::from_secs_f64(frames as f64 / f64::from(self.sample_rate.max(1)))
    }
}

/// A run of decoded sound.
#[derive(Clone, PartialEq)]
pub struct AudioChunk {
    /// When its first frame plays, from the start of the video.
    pub time: Duration,
    /// Interleaved 32-bit float samples in the decoder's [`AudioFormat`].
    pub samples: Vec<f32>,
}

impl std::fmt::Debug for AudioChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioChunk")
            .field("time", &self.time)
            .field("samples", &self.samples.len())
            .finish()
    }
}

/// What the decoder read next, in file order.
#[derive(Debug, Clone, PartialEq)]
pub enum Packet {
    Video(VideoFrame),
    Audio(AudioChunk),
}

/// Reads the frames (and sound) of one video, in order.
pub struct Decoder {
    #[cfg(windows)]
    inner: mf::Decoder,
    #[cfg(not(windows))]
    _never: std::convert::Infallible,
}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder")
            .field("duration", &self.duration())
            .field("audio", &self.audio_format())
            .finish_non_exhaustive()
    }
}

impl Decoder {
    /// Opens `url`: an `https://` URL or a local file path. Blocks while the
    /// source connects and reads the file header. Fails when the file has no
    /// video track; a missing or unreadable sound track is just left out.
    pub fn open(url: &str, tracks: Tracks) -> Result<Self, MediaError> {
        #[cfg(windows)]
        {
            mf::Decoder::open(url, tracks).map(|inner| Self { inner })
        }
        #[cfg(not(windows))]
        {
            let _ = (url, tracks);
            Err(MediaError::Unsupported)
        }
    }

    /// The decoded sound's shape, when the sound is read.
    pub fn audio_format(&self) -> Option<AudioFormat> {
        #[cfg(windows)]
        {
            self.inner.audio_format()
        }
        #[cfg(not(windows))]
        {
            match self._never {}
        }
    }

    /// The next frame or run of sound, or `None` once every track ended.
    /// Blocks while it decodes (and downloads, for a URL).
    pub fn next_packet(&mut self) -> Result<Option<Packet>, MediaError> {
        #[cfg(windows)]
        {
            self.inner.next_packet()
        }
        #[cfg(not(windows))]
        {
            match self._never {}
        }
    }

    /// The next frame, skipping any sound, or `None` at the end.
    pub fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        loop {
            match self.next_packet()? {
                Some(Packet::Video(frame)) => return Ok(Some(frame)),
                Some(Packet::Audio(_)) => {}
                None => return Ok(None),
            }
        }
    }

    /// Continues from `to`: the next frame is the one on screen at `to`, and
    /// the sound starts exactly there.
    pub fn seek(&mut self, to: Duration) -> Result<(), MediaError> {
        #[cfg(windows)]
        {
            self.inner.seek(to)
        }
        #[cfg(not(windows))]
        {
            let _ = to;
            match self._never {}
        }
    }

    /// Goes back to the first frame.
    pub fn rewind(&mut self) -> Result<(), MediaError> {
        self.seek(Duration::ZERO)
    }

    /// The length of the video, when the file says.
    pub fn duration(&self) -> Option<Duration> {
        #[cfg(windows)]
        {
            self.inner.duration()
        }
        #[cfg(not(windows))]
        {
            match self._never {}
        }
    }
}

/// Whether a frame shown at `time` for `duration` is over before `target`
/// (a seek lands on the keyframe before its target; those frames are skipped).
#[cfg_attr(all(not(windows), not(test)), expect(dead_code, reason = "used by the Windows decoder"))]
fn frame_before(time: Duration, duration: Duration, target: Duration) -> bool {
    time < target && time + duration <= target
}

/// Cuts the part of `chunk` that plays before `target`. Whole frames are cut
/// (a multiple of the channel count), or the channels would swap for the rest
/// of the video. `None` when nothing is left.
#[cfg_attr(all(not(windows), not(test)), expect(dead_code, reason = "used by the Windows decoder"))]
fn trim_audio(chunk: AudioChunk, target: Duration, format: AudioFormat) -> Option<AudioChunk> {
    if chunk.time >= target {
        return Some(chunk);
    }
    let channels = usize::from(format.channels.max(1));
    let late = (target - chunk.time).as_secs_f64() * f64::from(format.sample_rate);
    let cut = (late.ceil() as usize).saturating_mul(channels);
    if cut >= chunk.samples.len() {
        return None;
    }
    let frames = cut / channels;
    let time = chunk.time + format.duration_of(frames * channels);
    Some(AudioChunk { time, samples: chunk.samples[cut..].to_vec() })
}

/// Copies the visible `crop` of a 32-bit RGB picture out of `data` into
/// tightly packed BGRA rows, top row first, with opaque alpha.
///
/// `stride` is the byte distance between rows; negative means the rows are
/// stored bottom-up (the last row first), as Windows bitmaps can be.
/// `rows` is how many rows `data` holds. Fails instead of reading out of
/// bounds when `data` is shorter than the geometry says.
#[cfg_attr(
    all(not(windows), not(test)),
    expect(dead_code, reason = "only the Windows decoder copies frames")
)]
fn copy_bgra(data: &[u8], stride: i32, rows: u32, crop: Crop) -> Option<Vec<u8>> {
    let pitch = stride.unsigned_abs() as usize;
    let (x, w) = (crop.x as usize, crop.width as usize);
    if crop.y.checked_add(crop.height)? > rows || (x + w) * 4 > pitch {
        return None;
    }
    let mut out = Vec::with_capacity(w * crop.height as usize * 4);
    for y in crop.y..crop.y + crop.height {
        let row = if stride >= 0 { y } else { rows - 1 - y } as usize;
        let start = row * pitch + x * 4;
        let src = data.get(start..start + w * 4)?;
        for [b, g, r, _] in src.as_chunks::<4>().0 {
            out.extend_from_slice(&[*b, *g, *r, 0xff]);
        }
    }
    Some(out)
}

/// The visible part of a decoded picture, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    all(not(windows), not(test)),
    expect(dead_code, reason = "only the Windows decoder crops frames")
)]
struct Crop {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3 × 2 picture with a padding byte per row; pixel (x, y) is
    /// `[x, y, 9, 0]`.
    fn picture() -> Vec<u8> {
        let mut data = Vec::new();
        for y in 0..2u8 {
            for x in 0..3u8 {
                data.extend_from_slice(&[x, y, 9, 0]);
            }
            data.push(0xee);
        }
        data
    }

    const ALL: Crop = Crop { x: 0, y: 0, width: 3, height: 2 };

    #[test]
    fn top_down_rows_are_copied_with_opaque_alpha() {
        let out = copy_bgra(&picture(), 13, 2, ALL).unwrap();
        assert_eq!(&out[..8], &[0, 0, 9, 255, 1, 0, 9, 255]);
        assert_eq!(&out[12..16], &[0, 1, 9, 255], "second row starts after the padding");
        assert_eq!(out.len(), 3 * 2 * 4);
    }

    #[test]
    fn bottom_up_rows_are_flipped() {
        let out = copy_bgra(&picture(), -13, 2, ALL).unwrap();
        assert_eq!(&out[..4], &[0, 1, 9, 255], "the last stored row is the top");
    }

    #[test]
    fn the_crop_is_cut_out() {
        let out = copy_bgra(&picture(), 13, 2, Crop { x: 1, y: 1, width: 2, height: 1 }).unwrap();
        assert_eq!(out, vec![1, 1, 9, 255, 2, 1, 9, 255]);
    }

    const STEREO: AudioFormat = AudioFormat { sample_rate: 10, channels: 2 };

    fn chunk(time_ms: u64, frames: usize) -> AudioChunk {
        // Left samples are the frame number, right ones its negative.
        let samples = (0..frames).flat_map(|f| [f as f32, -(f as f32)]).collect();
        AudioChunk { time: Duration::from_millis(time_ms), samples }
    }

    #[test]
    fn frames_before_a_seek_target_are_skipped() {
        let ms = Duration::from_millis;
        assert!(frame_before(ms(0), ms(100), ms(150)));
        assert!(!frame_before(ms(100), ms(100), ms(150)), "on screen at the target");
        assert!(!frame_before(ms(150), ms(100), ms(150)));
        assert!(frame_before(ms(100), ms(0), ms(150)), "no duration: before is before");
        assert!(!frame_before(ms(150), ms(0), ms(150)));
    }

    #[test]
    fn audio_is_trimmed_to_the_seek_target_in_whole_frames() {
        // 10 frames per second: frame k plays at k * 100 ms.
        let out = trim_audio(chunk(0, 10), Duration::from_millis(250), STEREO).unwrap();
        assert_eq!(out.time, Duration::from_millis(300), "the first whole frame after it");
        assert_eq!(&out.samples[..2], &[3.0, -3.0], "left stays left");
        assert_eq!(out.samples.len(), 14);
    }

    #[test]
    fn audio_at_or_after_the_target_is_kept_and_before_it_dropped() {
        let target = Duration::from_millis(500);
        assert_eq!(trim_audio(chunk(600, 3), target, STEREO), Some(chunk(600, 3)));
        assert_eq!(trim_audio(chunk(0, 5), target, STEREO), None, "ends at the target");
        assert_eq!(trim_audio(chunk(0, 2), target, STEREO), None);
    }

    #[test]
    fn audio_durations_count_frames_not_samples() {
        assert_eq!(STEREO.duration_of(20), Duration::from_secs(1));
    }

    #[test]
    fn short_or_inconsistent_buffers_are_refused() {
        let data = picture();
        assert_eq!(copy_bgra(&data[..20], 13, 2, ALL), None, "buffer too short");
        assert_eq!(copy_bgra(&data, 13, 3, Crop { height: 3, ..ALL }), None, "too few rows");
        assert_eq!(copy_bgra(&data, 8, 2, ALL), None, "rows narrower than the crop");
        assert_eq!(copy_bgra(&data, 13, 2, Crop { y: u32::MAX, ..ALL }), None, "overflow");
    }
}
