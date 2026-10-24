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

/// Reads the frames of one video, in order.
pub struct Decoder {
    #[cfg(windows)]
    inner: mf::Decoder,
    #[cfg(not(windows))]
    _never: std::convert::Infallible,
}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder").field("duration", &self.duration()).finish_non_exhaustive()
    }
}

impl Decoder {
    /// Opens `url`: an `https://` URL or a local file path. Blocks while the
    /// source connects and reads the file header.
    pub fn open(url: &str) -> Result<Self, MediaError> {
        #[cfg(windows)]
        {
            mf::Decoder::open(url).map(|inner| Self { inner })
        }
        #[cfg(not(windows))]
        {
            let _ = url;
            Err(MediaError::Unsupported)
        }
    }

    /// The next frame, or `None` at the end of the video. Blocks while it
    /// decodes (and downloads, for a URL).
    pub fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        #[cfg(windows)]
        {
            self.inner.next_frame()
        }
        #[cfg(not(windows))]
        {
            match self._never {}
        }
    }

    /// Goes back to the first frame.
    pub fn rewind(&mut self) -> Result<(), MediaError> {
        #[cfg(windows)]
        {
            self.inner.rewind()
        }
        #[cfg(not(windows))]
        {
            match self._never {}
        }
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

    #[test]
    fn short_or_inconsistent_buffers_are_refused() {
        let data = picture();
        assert_eq!(copy_bgra(&data[..20], 13, 2, ALL), None, "buffer too short");
        assert_eq!(copy_bgra(&data, 13, 3, Crop { height: 3, ..ALL }), None, "too few rows");
        assert_eq!(copy_bgra(&data, 8, 2, ALL), None, "rows narrower than the crop");
        assert_eq!(copy_bgra(&data, 13, 2, Crop { y: u32::MAX, ..ALL }), None, "overflow");
    }
}
