//! Writes `tests/fixtures/bars.mp4`, the synthetic video the decoder tests
//! read: H.264 in MP4, 128 × 90 (coded as 128 × 96, so the crop is
//! exercised), 10 fps, 8 frames. Frame `k`'s top half is `COLOURS[k % 4]` and
//! its bottom half is white, so a test sees both order and orientation.
//!
//! ```sh
//! cargo run -p twitty-media --example make_fixture
//! ```

#![expect(unsafe_code, reason = "Media Foundation is a COM API: every call is unsafe")]
#![expect(clippy::print_stdout, reason = "a developer tool that reports what it wrote")]

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    use windows::{
        Win32::{
            Media::MediaFoundation::{
                MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
                MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
                MF_VERSION, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
                MFCreateSinkWriterFromURL, MFMediaType_Video, MFSTARTUP_FULL, MFShutdown,
                MFStartup, MFVideoFormat_H264, MFVideoFormat_RGB32, MFVideoInterlace_Progressive,
            },
            System::Com::{COINIT_MULTITHREADED, CoInitializeEx},
        },
        core::HSTRING,
    };

    const W: u32 = 128;
    const H: u32 = 90;
    const FRAMES: i64 = 8;
    /// 100 ns units per frame at 10 fps.
    const STEP: i64 = 1_000_000;
    /// B, G, R bytes: red, green, blue, yellow.
    const COLOURS: [[u8; 3]; 4] = [[0, 0, 255], [0, 255, 0], [255, 0, 0], [0, 255, 255]];

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bars.mp4");
    // SAFETY: a tool's main thread; COM and MF stay up until the process
    // ends. Every pointer passed is a live local.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, None)?;
        let video_type = |subtype| -> windows::core::Result<_> {
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
        let stream = writer.AddStream(&output)?;
        let input = video_type(&MFVideoFormat_RGB32)?;
        // Positive: the rows below are written top-down.
        input.SetUINT32(&MF_MT_DEFAULT_STRIDE, W * 4)?;
        writer.SetInputMediaType(stream, &input, None)?;
        writer.BeginWriting()?;

        for k in 0..FRAMES {
            let colour = COLOURS[k as usize % COLOURS.len()];
            let mut pixels = Vec::with_capacity((W * H * 4) as usize);
            for y in 0..H {
                let [b, g, r] = if y < H / 2 { colour } else { [255, 255, 255] };
                for _ in 0..W {
                    pixels.extend_from_slice(&[b, g, r, 255]);
                }
            }
            let buffer = MFCreateMemoryBuffer(pixels.len() as u32)?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), data, pixels.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(pixels.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(k * STEP)?;
            sample.SetSampleDuration(STEP)?;
            writer.WriteSample(stream, &sample)?;
        }
        writer.Finalize()?;
        drop(writer);
        MFShutdown()?;
    }
    println!("wrote {}", path.display());
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    println!("the fixture is written with Media Foundation: run this on Windows");
}
