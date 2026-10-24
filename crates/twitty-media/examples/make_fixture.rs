//! Writes the synthetic videos the decoder tests read, H.264 in MP4,
//! 128 × 90 (coded as 128 × 96, so the crop is exercised), 10 fps. Frame `k`'s
//! top half is `COLOURS[k % 4]` and its bottom half is white, so a test sees
//! both order and orientation.
//!
//! - `tests/fixtures/bars.mp4`: 8 frames, no sound.
//! - `tests/fixtures/tone.mp4`: 10 frames (1 s) and AAC stereo sound at
//!   44.1 kHz: a 440 Hz tone on the left, silence on the right, so a test
//!   sees that the channels keep their order.
//!
//! ```sh
//! cargo run -p twitty-media --example make_fixture
//! ```

#![expect(unsafe_code, reason = "Media Foundation is a COM API: every call is unsafe")]
#![expect(clippy::print_stdout, reason = "a developer tool that reports what it wrote")]

#[cfg(windows)]
mod write {
    use windows::{
        Win32::Media::MediaFoundation::{
            IMFSinkWriter, MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE,
            MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
            MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
            MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
            MFAudioFormat_AAC, MFAudioFormat_PCM, MFCreateMediaType, MFCreateMemoryBuffer,
            MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Audio, MFMediaType_Video,
            MFVideoFormat_H264, MFVideoFormat_RGB32, MFVideoInterlace_Progressive,
        },
        core::{GUID, HSTRING, Result},
    };

    const W: u32 = 128;
    const H: u32 = 90;
    /// 100 ns units per frame at 10 fps.
    const STEP: i64 = 1_000_000;
    /// B, G, R bytes: red, green, blue, yellow.
    const COLOURS: [[u8; 3]; 4] = [[0, 0, 255], [0, 255, 0], [255, 0, 0], [0, 255, 255]];
    const RATE: u32 = 44_100;
    const CHANNELS: u32 = 2;

    /// Writes `frames` frames to `name`, with the tone when `sound`.
    ///
    /// # Safety
    /// COM and Media Foundation must be started on this thread.
    pub unsafe fn video(name: &str, frames: i64, sound: bool) -> Result<()> {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        // SAFETY: every pointer passed is a live local (see the caller).
        unsafe {
            let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, None)?;
            let video_type = |subtype: &GUID| -> Result<_> {
                let ty = MFCreateMediaType()?;
                ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                ty.SetGUID(&MF_MT_SUBTYPE, subtype)?;
                ty.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
                ty.SetUINT64(&MF_MT_FRAME_SIZE, (u64::from(W) << 32) | u64::from(H))?;
                ty.SetUINT64(&MF_MT_FRAME_RATE, (10 << 32) | 1)?;
                ty.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
                Ok(ty)
            };
            let output = video_type(&MFVideoFormat_H264)?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, 800_000)?;
            let picture = writer.AddStream(&output)?;
            let input = video_type(&MFVideoFormat_RGB32)?;
            // Positive: the rows below are written top-down.
            input.SetUINT32(&MF_MT_DEFAULT_STRIDE, W * 4)?;
            writer.SetInputMediaType(picture, &input, None)?;

            let audio_type = |subtype: &GUID, bytes_per_second: u32| -> Result<_> {
                let ty = MFCreateMediaType()?;
                ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                ty.SetGUID(&MF_MT_SUBTYPE, subtype)?;
                ty.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)?;
                ty.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
                ty.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                ty.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, bytes_per_second)?;
                Ok(ty)
            };
            let sound_stream = if sound {
                // The AAC encoder only takes these byte rates.
                let stream = writer.AddStream(&audio_type(&MFAudioFormat_AAC, 16_000)?)?;
                let pcm = audio_type(&MFAudioFormat_PCM, RATE * CHANNELS * 2)?;
                pcm.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, CHANNELS * 2)?;
                writer.SetInputMediaType(stream, &pcm, None)?;
                Some(stream)
            } else {
                None
            };
            writer.BeginWriting()?;

            for k in 0..frames {
                if let Some(stream) = sound_stream {
                    write(&writer, stream, &tone(k), k * STEP)?;
                }
                write(&writer, picture, &picture_bytes(k), k * STEP)?;
            }
            writer.Finalize()?;
        }
        println!("wrote {}", path.display());
        Ok(())
    }

    fn picture_bytes(k: i64) -> Vec<u8> {
        let colour = COLOURS[k as usize % COLOURS.len()];
        let mut pixels = Vec::with_capacity((W * H * 4) as usize);
        for y in 0..H {
            let [b, g, r] = if y < H / 2 { colour } else { [255, 255, 255] };
            for _ in 0..W {
                pixels.extend_from_slice(&[b, g, r, 255]);
            }
        }
        pixels
    }

    /// Frame `k`'s 100 ms of sound: 16-bit stereo, the tone on the left.
    fn tone(k: i64) -> Vec<u8> {
        let per_frame = RATE / 10;
        let mut bytes = Vec::with_capacity((per_frame * CHANNELS * 2) as usize);
        for i in 0..per_frame {
            let t = f64::from(k as u32 * per_frame + i) / f64::from(RATE);
            let left = ((t * 440.0 * std::f64::consts::TAU).sin() * 16_000.0) as i16;
            bytes.extend_from_slice(&left.to_le_bytes());
            bytes.extend_from_slice(&0i16.to_le_bytes());
        }
        bytes
    }

    /// Writes one sample of `bytes` at `time`, lasting a frame.
    ///
    /// # Safety
    /// As [`video`].
    unsafe fn write(writer: &IMFSinkWriter, stream: u32, bytes: &[u8], time: i64) -> Result<()> {
        // SAFETY: the buffer is locked while the bytes are copied into it,
        // and holds at least `bytes.len()` bytes.
        unsafe {
            let buffer = MFCreateMemoryBuffer(bytes.len() as u32)?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(time)?;
            sample.SetSampleDuration(STEP)?;
            writer.WriteSample(stream, &sample)
        }
    }
}

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    use windows::Win32::{
        Media::MediaFoundation::{MF_VERSION, MFSTARTUP_FULL, MFShutdown, MFStartup},
        System::Com::{COINIT_MULTITHREADED, CoInitializeEx},
    };
    // SAFETY: a tool's main thread; COM and MF stay up until the end.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        write::video("bars.mp4", 8, false)?;
        write::video("tone.mp4", 10, true)?;
        MFShutdown()
    }
}

#[cfg(not(windows))]
fn main() {
    println!("the fixtures are written with Media Foundation: run this on Windows");
}
