//! The Media Foundation decoder behind [`crate::Decoder`].
//!
//! An `IMFSourceReader` opens the file or URL, decodes the first video track
//! (and, when asked, the first audio track) and converts them: the picture to
//! 32-bit RGB with advanced video processing on, the sound to 32-bit float.
//! Each sample is copied out of its locked buffer into a [`VideoFrame`] or an
//! [`AudioChunk`].
//!
//! Media Foundation's DLLs are loaded at run time ([`Api`]), not linked:
//! Windows N editions without the Media Feature Pack and Windows Server
//! without its Media Foundation feature don't have them, and a linked import
//! would stop the whole app (and every test binary) from starting there.
//! COM methods are vtable calls and need no import.
//!
//! Media Foundation is started once per process and never shut down. Shutting
//! it down after a failed download and starting it again crashed the process
//! (an access violation in the network source's leftover work).

#![expect(unsafe_code, reason = "Media Foundation is a COM API: every call is unsafe")]

use std::{ffi::c_void, marker::PhantomData, sync::OnceLock, time::Duration};

use windows::{
    Win32::{
        Foundation::{E_POINTER, HMODULE, RPC_E_CHANGED_MODE},
        Media::MediaFoundation::{
            IMFAttributes, IMFMediaBuffer, IMFMediaType, IMFSample, IMFSourceReader,
            MF_E_INVALIDSTREAMNUMBER, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
            MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE, MF_MT_GEOMETRIC_APERTURE, MF_MT_MAJOR_TYPE,
            MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_SUBTYPE, MF_PD_DURATION,
            MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_ANY_STREAM,
            MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, MF_SOURCE_READER_MEDIASOURCE,
            MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED, MF_SOURCE_READERF_ENDOFSTREAM,
            MF_SOURCE_READERF_ERROR, MF_VERSION, MFAudioFormat_Float, MFMediaType_Audio,
            MFMediaType_Video, MFSTARTUP_FULL, MFVideoFormat_RGB32,
        },
        System::{
            Com::{
                COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize,
                StructuredStorage::PROPVARIANT,
            },
            LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
            Variant::{VT_I8, VT_UI8},
        },
    },
    core::{GUID, HRESULT, HSTRING, Interface as _, PCSTR, PCWSTR, s, w},
};

use crate::{
    AudioChunk, AudioFormat, Crop, MediaError, Packet, Tracks, VideoFrame, copy_bgra, frame_before,
    trim_audio,
};

type StartupFn = unsafe extern "system" fn(u32, u32) -> HRESULT;
type CreateAttributesFn = unsafe extern "system" fn(*mut *mut c_void, u32) -> HRESULT;
type CreateMediaTypeFn = unsafe extern "system" fn(*mut *mut c_void) -> HRESULT;
type CreateSourceReaderFromUrlFn =
    unsafe extern "system" fn(PCWSTR, *mut c_void, *mut *mut c_void) -> HRESULT;
type Export = unsafe extern "system" fn() -> isize;

/// The Media Foundation functions the decoder calls, from `mfplat.dll` and
/// `mfreadwrite.dll`. Loaded once per process and never unloaded, so the
/// pointers stay valid.
struct Api {
    startup: StartupFn,
    create_attributes: CreateAttributesFn,
    create_media_type: CreateMediaTypeFn,
    create_source_reader_from_url: CreateSourceReaderFromUrlFn,
}

/// The loaded and started API, or why it isn't there (cached: it won't
/// appear later). Call it with COM initialised on the thread.
fn api() -> Result<&'static Api, MediaError> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| {
        let api = Api::load()?;
        // SAFETY: plain call. Never balanced by `MFShutdown`: Media
        // Foundation stays up until the process ends (see the module docs).
        unsafe { (api.startup)(MF_VERSION, MFSTARTUP_FULL) }.ok().map_err(|e| e.to_string())?;
        Ok(api)
    })
    .as_ref()
    .map_err(|e| MediaError::Unavailable(e.clone()))
}

impl Api {
    fn load() -> Result<Self, String> {
        let mfplat = library(w!("mfplat.dll"), "mfplat.dll")?;
        let mfreadwrite = library(w!("mfreadwrite.dll"), "mfreadwrite.dll")?;
        let startup = export(mfplat, s!("MFStartup"))?;
        let create_attributes = export(mfplat, s!("MFCreateAttributes"))?;
        let create_media_type = export(mfplat, s!("MFCreateMediaType"))?;
        let create_source_reader_from_url = export(mfreadwrite, s!("MFCreateSourceReaderFromURL"))?;
        // SAFETY: each pointer is the export of that name, and each type is
        // that function's C signature (mfapi.h, mfreadwrite.h). Function
        // pointers all have the same size.
        unsafe {
            Ok(Self {
                startup: std::mem::transmute::<Export, StartupFn>(startup),
                create_attributes: std::mem::transmute::<Export, CreateAttributesFn>(
                    create_attributes,
                ),
                create_media_type: std::mem::transmute::<Export, CreateMediaTypeFn>(
                    create_media_type,
                ),
                create_source_reader_from_url: std::mem::transmute::<
                    Export,
                    CreateSourceReaderFromUrlFn,
                >(create_source_reader_from_url),
            })
        }
    }

    fn create_attributes(&self) -> windows::core::Result<IMFAttributes> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: a valid out pointer; on success it holds a new reference
        // that `owned` takes over.
        unsafe {
            (self.create_attributes)(&mut raw, 1).ok()?;
            owned(raw)
        }
    }

    fn create_media_type(&self) -> windows::core::Result<IMFMediaType> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: as in `create_attributes`.
        unsafe {
            (self.create_media_type)(&mut raw).ok()?;
            owned(raw)
        }
    }

    fn create_source_reader(
        &self,
        url: &str,
        attributes: &IMFAttributes,
    ) -> windows::core::Result<IMFSourceReader> {
        let url = HSTRING::from(url);
        let mut raw = std::ptr::null_mut();
        // SAFETY: `url` is NUL-terminated and outlives the call; the
        // attributes are only borrowed (the callee adds its own reference);
        // the out pointer is valid, and on success holds a new reference.
        unsafe {
            (self.create_source_reader_from_url)(
                PCWSTR(url.as_ptr()),
                attributes.as_raw(),
                &mut raw,
            )
            .ok()?;
            owned(raw)
        }
    }
}

/// Takes over the reference in `raw`, a COM object of type `T` returned by a
/// successful create call.
///
/// # Safety
/// `raw` must be null or an owned reference to a `T`.
unsafe fn owned<T: windows::core::Interface>(raw: *mut c_void) -> windows::core::Result<T> {
    if raw.is_null() {
        return Err(E_POINTER.into());
    }
    // SAFETY: non-null, and owned per the caller's contract.
    Ok(unsafe { T::from_raw(raw) })
}

/// Loads a system DLL (from System32 only, never the app's folder).
fn library(name: PCWSTR, display: &str) -> Result<HMODULE, String> {
    // SAFETY: a static NUL-terminated name. The module is never freed.
    unsafe { LoadLibraryExW(name, None, LOAD_LIBRARY_SEARCH_SYSTEM32) }
        .map_err(|e| format!("{display}: {e}"))
}

fn export(module: HMODULE, name: PCSTR) -> Result<Export, String> {
    // SAFETY: a loaded module and a static NUL-terminated name.
    unsafe { GetProcAddress(module, name) }.ok_or_else(|| {
        // SAFETY: the same static NUL-terminated name.
        format!("{} is missing", unsafe { name.display() })
    })
}

/// COM, initialised on the current thread for as long as this lives. Not
/// `Send`: it must be dropped on the thread that made it.
struct Com {
    /// Whether this guard initialised COM and so must uninitialise it.
    owned: bool,
    _thread_bound: PhantomData<*const ()>,
}

impl Com {
    fn new() -> Result<Self, MediaError> {
        // SAFETY: no reserved pointer. A success (S_OK or S_FALSE) is
        // balanced by `CoUninitialize` in `Drop`, on this thread since `Com`
        // is not `Send`.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // A thread already in a single-threaded apartment keeps it; Media
        // Foundation works there too, but then COM isn't ours to close.
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return Err(MediaError::Unavailable(windows::core::Error::from(hr).to_string()));
        }
        Ok(Self { owned: hr.is_ok(), _thread_bound: PhantomData })
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: balances the successful `CoInitializeEx` on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// Media Foundation, usable on the current thread (inside COM) for as long
/// as this lives.
struct Started {
    api: &'static Api,
    _com: Com,
}

impl Started {
    fn new() -> Result<Self, MediaError> {
        let com = Com::new()?;
        Ok(Self { api: api()?, _com: com })
    }
}

/// The shape of the reader's current output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Format {
    /// Byte distance between rows; negative for bottom-up rows.
    stride: i32,
    /// Rows in a decoded buffer (the coded height, which can be padded).
    rows: u32,
    /// The visible picture.
    crop: Crop,
}

/// The sound track being read.
#[derive(Debug, Clone, Copy)]
struct Audio {
    stream: u32,
    format: AudioFormat,
}

pub(crate) struct Decoder {
    // Field order matters: the reader must be released before COM is.
    reader: IMFSourceReader,
    /// The video track's stream index.
    video: u32,
    format: Format,
    audio: Option<Audio>,
    duration: Option<Duration>,
    /// Tracks that reported their end since the last seek.
    video_ended: bool,
    audio_ended: bool,
    /// After a seek, packets before this time are skipped or trimmed.
    seek_target: Option<Duration>,
    _started: Started,
}

impl Decoder {
    pub(crate) fn open(url: &str, tracks: Tracks) -> Result<Self, MediaError> {
        let started = Started::new()?;
        let api = started.api;
        let open = |e: windows::core::Error| MediaError::Open(e.to_string());
        let attributes = api.create_attributes().map_err(open)?;
        // SAFETY: the key is a valid GUID; the value is a plain integer.
        unsafe { attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1) }
            .map_err(open)?;
        let reader = api.create_source_reader(url, &attributes).map_err(open)?;
        let (video, audio) = first_streams(&reader).map_err(open)?;
        let video = video.ok_or_else(|| MediaError::Open("the file has no video track".into()))?;
        // SAFETY: plain calls on a live reader with its own stream indices.
        unsafe {
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(open)?;
            reader.SetStreamSelection(video, true).map_err(open)?;
        }
        let wanted = output_type(api, &MFMediaType_Video, &MFVideoFormat_RGB32).map_err(open)?;
        // SAFETY: a live reader and media type; no reserved pointer.
        unsafe { reader.SetCurrentMediaType(video, None, &wanted) }.map_err(open)?;
        let format = current_format(&reader, video).map_err(MediaError::Open)?;
        let audio = match (tracks, audio) {
            (Tracks::VideoAndAudio, Some(stream)) => select_audio(api, &reader, stream),
            _ => None,
        };
        let duration = media_duration(&reader);
        Ok(Self {
            reader,
            video,
            format,
            audio,
            duration,
            video_ended: false,
            audio_ended: audio.is_none(),
            seek_target: None,
            _started: started,
        })
    }

    pub(crate) fn audio_format(&self) -> Option<AudioFormat> {
        self.audio.map(|a| a.format)
    }

    pub(crate) fn next_packet(&mut self) -> Result<Option<Packet>, MediaError> {
        let decode = |e: windows::core::Error| MediaError::Decode(e.to_string());
        loop {
            if self.video_ended && self.audio_ended {
                return Ok(None);
            }
            let mut stream = 0u32;
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            // SAFETY: the out pointers are valid locals for the whole call
            // (the reader is synchronous: no callback is set).
            unsafe {
                self.reader.ReadSample(
                    MF_SOURCE_READER_ANY_STREAM.0 as u32,
                    0,
                    Some(&mut stream),
                    Some(&mut flags),
                    None,
                    Some(&mut sample),
                )
            }
            .map_err(decode)?;
            let has = |flag: i32| flags & flag as u32 != 0;
            if has(MF_SOURCE_READERF_ERROR.0) {
                return Err(MediaError::Decode("the stream reported an error".into()));
            }
            let audio = self.audio.filter(|a| a.stream == stream);
            if has(MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0) {
                if stream == self.video {
                    self.format =
                        current_format(&self.reader, self.video).map_err(MediaError::Decode)?;
                } else if let Some(a) = audio {
                    let format = audio_format(&self.reader, a.stream).map_err(decode)?;
                    self.audio = Some(Audio { format, ..a });
                }
            }
            if has(MF_SOURCE_READERF_ENDOFSTREAM.0) {
                if stream == self.video {
                    self.video_ended = true;
                } else if audio.is_some() {
                    self.audio_ended = true;
                }
                continue;
            }
            // No sample (a stream tick or a gap): read on.
            let Some(sample) = sample else { continue };
            if stream == self.video {
                let frame = frame(&sample, self.format)?;
                if self.seek_target.is_some_and(|t| frame_before(frame.time, frame.duration, t)) {
                    continue;
                }
                return Ok(Some(Packet::Video(frame)));
            }
            let Some(audio) = self.audio.filter(|a| a.stream == stream) else { continue };
            let chunk = audio_chunk(&sample)?;
            let chunk = match self.seek_target {
                Some(target) => match trim_audio(chunk, target, audio.format) {
                    Some(chunk) => chunk,
                    None => continue,
                },
                None => chunk,
            };
            return Ok(Some(Packet::Audio(chunk)));
        }
    }

    pub(crate) fn seek(&mut self, to: Duration) -> Result<(), MediaError> {
        let mut start = PROPVARIANT::default();
        let hns = i64::try_from(to.as_nanos() / 100).unwrap_or(i64::MAX);
        // SAFETY: sets the tag and the matching 64-bit member of a zeroed
        // PROPVARIANT. VT_I8 owns no memory, so nothing needs clearing.
        unsafe {
            let value = &mut *start.Anonymous.Anonymous;
            value.vt = VT_I8;
            value.Anonymous.hVal = hns;
        }
        // SAFETY: GUID_NULL means 100 ns units; both pointers are valid locals.
        unsafe { self.reader.SetCurrentPosition(&GUID::zeroed(), &start) }
            .map_err(|e| MediaError::Decode(e.to_string()))?;
        self.video_ended = false;
        self.audio_ended = self.audio.is_none();
        self.seek_target = (to > Duration::ZERO).then_some(to);
        Ok(())
    }

    pub(crate) fn duration(&self) -> Option<Duration> {
        self.duration
    }
}

/// The indices of the first video and the first audio stream.
fn first_streams(reader: &IMFSourceReader) -> windows::core::Result<(Option<u32>, Option<u32>)> {
    let (mut video, mut audio) = (None, None);
    for index in 0u32.. {
        // SAFETY: a plain getter; an index past the last stream is an error.
        let ty = match unsafe { reader.GetNativeMediaType(index, 0) } {
            Ok(ty) => ty,
            Err(e) if e.code() == MF_E_INVALIDSTREAMNUMBER => break,
            Err(e) => return Err(e),
        };
        // SAFETY: a plain getter with a valid GUID key.
        let major = unsafe { ty.GetGUID(&MF_MT_MAJOR_TYPE) }?;
        if major == MFMediaType_Video && video.is_none() {
            video = Some(index);
        } else if major == MFMediaType_Audio && audio.is_none() {
            audio = Some(index);
        }
    }
    Ok((video, audio))
}

/// Selects the sound track as 32-bit float. A track that can't be decoded
/// is left out: the video still plays, silent.
fn select_audio(api: &Api, reader: &IMFSourceReader, stream: u32) -> Option<Audio> {
    let wanted = output_type(api, &MFMediaType_Audio, &MFAudioFormat_Float).ok()?;
    // SAFETY: plain calls on a live reader and media type.
    let selected = unsafe {
        reader
            .SetStreamSelection(stream, true)
            .and_then(|()| reader.SetCurrentMediaType(stream, None, &wanted))
    };
    match selected.and_then(|()| audio_format(reader, stream)) {
        Ok(format) if format.channels > 0 && format.sample_rate > 0 => {
            Some(Audio { stream, format })
        }
        _ => {
            // SAFETY: as above.
            let _ = unsafe { reader.SetStreamSelection(stream, false) };
            None
        }
    }
}

/// The sound's shape as the reader outputs it (read back, not assumed).
fn audio_format(reader: &IMFSourceReader, stream: u32) -> windows::core::Result<AudioFormat> {
    // SAFETY: plain getters on live objects with valid GUID keys.
    unsafe {
        let ty = reader.GetCurrentMediaType(stream)?;
        Ok(AudioFormat {
            sample_rate: ty.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)?,
            channels: u16::try_from(ty.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)?).unwrap_or(0),
        })
    }
}

fn audio_chunk(sample: &IMFSample) -> Result<AudioChunk, MediaError> {
    let decode = |e: windows::core::Error| MediaError::Decode(e.to_string());
    // SAFETY: plain getters on a live sample.
    let (time, buffer) = unsafe {
        (
            sample.GetSampleTime().map_err(decode)?,
            sample.ConvertToContiguousBuffer().map_err(decode)?,
        )
    };
    let locked = Locked::new(&buffer).map_err(decode)?;
    let samples =
        locked.bytes().as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
    Ok(AudioChunk { time: hns(time), samples })
}

/// A type for the reader to output: 32-bit RGB (B, G, R, X in memory) for
/// video, 32-bit float for audio.
fn output_type(api: &Api, major: &GUID, subtype: &GUID) -> windows::core::Result<IMFMediaType> {
    let ty = api.create_media_type()?;
    // SAFETY: plain setters; the keys and values are valid GUIDs.
    unsafe {
        ty.SetGUID(&MF_MT_MAJOR_TYPE, major)?;
        ty.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        Ok(ty)
    }
}

/// Reads the size, stride and visible area of the reader's output, which can
/// differ from the file's: decoders pad the coded height (1080 → 1088).
fn current_format(reader: &IMFSourceReader, video: u32) -> Result<Format, String> {
    // SAFETY: plain getters on live objects with valid GUID keys.
    let (ty, size) = unsafe {
        let ty = reader.GetCurrentMediaType(video).map_err(|e| e.to_string())?;
        let size = ty.GetUINT64(&MF_MT_FRAME_SIZE).map_err(|e| e.to_string())?;
        (ty, size)
    };
    let (width, rows) = ((size >> 32) as u32, size as u32);
    if width == 0 || rows == 0 {
        return Err(format!("empty frame size {width}x{rows}"));
    }
    // SAFETY: as above. The stride is stored as a UINT32 holding an i32.
    let stride = unsafe { ty.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
        .map(|s| s as i32)
        .unwrap_or(width.saturating_mul(4) as i32);
    let full = Crop { x: 0, y: 0, width, height: rows };
    let crop = [MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_GEOMETRIC_APERTURE]
        .iter()
        .find_map(|key| aperture(&ty, key))
        .filter(|c| c.x + c.width <= width && c.y + c.height <= rows && c.width > 0 && c.height > 0)
        .unwrap_or(full);
    Ok(Format { stride, rows, crop })
}

/// An `MFVideoArea` blob: two `MFOffset { fract: u16, value: i16 }` then a
/// `SIZE { cx: i32, cy: i32 }`. Parsed by hand to avoid reinterpreting bytes.
fn aperture(ty: &IMFMediaType, key: &GUID) -> Option<Crop> {
    let mut blob = [0u8; 16];
    let mut len = 0u32;
    // SAFETY: the buffer is a valid slice; `len` is a valid out pointer.
    unsafe { ty.GetBlob(key, &mut blob, Some(&mut len)) }.ok()?;
    if len != 16 {
        return None;
    }
    let i16_at = |i: usize| i16::from_le_bytes([blob[i], blob[i + 1]]);
    let i32_at = |i: usize| i32::from_le_bytes([blob[i], blob[i + 1], blob[i + 2], blob[i + 3]]);
    Some(Crop {
        x: u32::try_from(i16_at(2)).ok()?,
        y: u32::try_from(i16_at(6)).ok()?,
        width: u32::try_from(i32_at(8)).ok()?,
        height: u32::try_from(i32_at(12)).ok()?,
    })
}

fn media_duration(reader: &IMFSourceReader) -> Option<Duration> {
    // SAFETY: a plain getter; the returned PROPVARIANT is read only through
    // the member its tag names, and a VT_UI8 owns no memory to clear.
    unsafe {
        let value = reader
            .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
            .ok()?;
        let inner = &value.Anonymous.Anonymous;
        (inner.vt == VT_UI8).then(|| hns(inner.Anonymous.uhVal as i64))
    }
}

/// A Media Foundation time (100 ns units) as a duration; negative is zero.
fn hns(t: i64) -> Duration {
    Duration::from_nanos(u64::try_from(t).unwrap_or(0).saturating_mul(100))
}

fn frame(sample: &IMFSample, format: Format) -> Result<VideoFrame, MediaError> {
    let decode = |e: windows::core::Error| MediaError::Decode(e.to_string());
    // SAFETY: plain getters on a live sample.
    let (time, duration, buffer) = unsafe {
        (
            sample.GetSampleTime().map_err(decode)?,
            sample.GetSampleDuration().unwrap_or(0),
            sample.ConvertToContiguousBuffer().map_err(decode)?,
        )
    };
    let locked = Locked::new(&buffer).map_err(decode)?;
    let bgra =
        copy_bgra(locked.bytes(), format.stride, format.rows, format.crop).ok_or_else(|| {
            MediaError::Decode(format!(
                "a {}-byte buffer is too small for {format:?}",
                locked.bytes().len()
            ))
        })?;
    Ok(VideoFrame {
        time: hns(time),
        duration: hns(duration),
        width: format.crop.width,
        height: format.crop.height,
        bgra,
    })
}

/// A media buffer locked for reading; unlocked on drop.
struct Locked<'a> {
    buffer: &'a IMFMediaBuffer,
    data: *const u8,
    len: usize,
}

impl<'a> Locked<'a> {
    fn new(buffer: &'a IMFMediaBuffer) -> windows::core::Result<Self> {
        let mut data = std::ptr::null_mut();
        let mut len = 0u32;
        // SAFETY: valid out pointers; balanced by `Unlock` in `Drop`.
        unsafe { buffer.Lock(&mut data, None, Some(&mut len)) }?;
        Ok(Self { buffer, data, len: len as usize })
    }

    fn bytes(&self) -> &[u8] {
        if self.data.is_null() || self.len == 0 {
            return &[];
        }
        // SAFETY: while locked, Media Foundation guarantees `len` readable
        // bytes at `data`, and nothing writes to the buffer. The slice
        // borrows `self`, so it can't outlive the lock.
        unsafe { std::slice::from_raw_parts(self.data, self.len) }
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        // SAFETY: balances the successful `Lock` in `new`.
        let _ = unsafe { self.buffer.Unlock() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_foundation_times_are_100_ns() {
        assert_eq!(hns(10_000_000), Duration::from_secs(1));
        assert_eq!(hns(-5), Duration::ZERO);
    }

    #[test]
    fn com_can_start_twice_on_one_thread() {
        let (a, b) = (Com::new().unwrap(), Com::new().unwrap());
        assert!(a.owned && b.owned, "S_FALSE must be balanced too");
    }
}
