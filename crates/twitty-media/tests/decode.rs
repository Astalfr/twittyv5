//! Decodes `fixtures/bars.mp4` (see `examples/make_fixture.rs`) with the
//! system decoder.
//!
//! GitHub's Windows Server runners don't have the Media Foundation feature,
//! so there each test says so and checks nothing more (opening still has to
//! fail cleanly, with `Unavailable`). Every Windows desktop has it.

#![cfg(windows)]
#![expect(clippy::unwrap_used, clippy::print_stdout, reason = "test helpers outside #[test] fns")]

use std::time::Duration;

use twitty_media::{Decoder, MediaError, VideoFrame};

const W: u32 = 128;
const H: u32 = 90;
/// B, G, R of the top half of frame `k % 4`: red, green, blue, yellow.
const COLOURS: [[u8; 3]; 4] = [[0, 0, 255], [0, 255, 0], [255, 0, 0], [0, 255, 255]];

fn fixture() -> String {
    format!("{}/tests/fixtures/bars.mp4", env!("CARGO_MANIFEST_DIR"))
}

fn open() -> Decoder {
    Decoder::open(&fixture()).unwrap()
}

/// Whether this machine can decode; says why not when it can't.
fn available() -> bool {
    match Decoder::open(&fixture()) {
        Err(MediaError::Unavailable(why)) => {
            println!("skipped: no Media Foundation here ({why})");
            false
        }
        _ => true,
    }
}

/// The B, G, R of pixel (x, y).
fn pixel(frame: &VideoFrame, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * frame.width + x) * 4) as usize;
    [frame.bgra[i], frame.bgra[i + 1], frame.bgra[i + 2]]
}

/// H.264 is lossy and goes through YUV 4:2:0: compare loosely.
fn close(actual: [u8; 3], expected: [u8; 3]) -> bool {
    actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 24)
}

fn all_frames(decoder: &mut Decoder) -> Vec<VideoFrame> {
    std::iter::from_fn(|| decoder.next_frame().unwrap()).collect()
}

#[test]
fn every_frame_comes_out_in_order_at_its_time() {
    if !available() {
        return;
    }
    let frames = all_frames(&mut open());
    assert_eq!(frames.len(), 8);
    for (k, frame) in frames.iter().enumerate() {
        assert_eq!(frame.time, Duration::from_millis(100) * k as u32, "frame {k}");
        assert_eq!(frame.duration, Duration::from_millis(100), "frame {k}");
    }
}

#[test]
fn frames_are_cropped_to_the_picture_not_the_coded_size() {
    if !available() {
        return;
    }
    let frame = open().next_frame().unwrap().unwrap();
    assert_eq!((frame.width, frame.height), (W, H), "not the 96 rows H.264 codes");
    assert_eq!(frame.bgra.len(), (W * H * 4) as usize);
}

#[test]
fn pixels_are_upright_bgra_and_opaque() {
    if !available() {
        return;
    }
    for (k, frame) in all_frames(&mut open()).iter().enumerate() {
        let top = pixel(frame, W / 2, 10);
        let bottom = pixel(frame, W / 2, H - 10);
        assert!(close(top, COLOURS[k % 4]), "frame {k} top is {top:?}");
        assert!(close(bottom, [255, 255, 255]), "frame {k} bottom is {bottom:?}");
    }
    let frame = open().next_frame().unwrap().unwrap();
    assert!(frame.bgra.as_chunks::<4>().0.iter().all(|px| px[3] == 255), "alpha is forced opaque");
}

#[test]
fn rewinding_starts_over() {
    if !available() {
        return;
    }
    let mut decoder = open();
    let first = all_frames(&mut decoder);
    assert_eq!(decoder.next_frame().unwrap(), None, "still at the end");
    decoder.rewind().unwrap();
    let again = decoder.next_frame().unwrap().unwrap();
    assert_eq!(again.time, Duration::ZERO);
    assert!(close(pixel(&again, 1, 1), pixel(&first[0], 1, 1)));
}

#[test]
fn the_duration_is_read_from_the_file() {
    if !available() {
        return;
    }
    assert_eq!(open().duration(), Some(Duration::from_millis(800)));
}

#[test]
fn a_missing_file_or_garbage_is_an_open_error() {
    if !available() {
        return;
    }
    let missing = format!("{}/tests/fixtures/none.mp4", env!("CARGO_MANIFEST_DIR"));
    assert!(matches!(Decoder::open(&missing), Err(MediaError::Open(_))));
    let garbage = format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR"));
    assert!(matches!(Decoder::open(&garbage), Err(MediaError::Open(_))));
}

/// An HTTP server on localhost that answers every request with a 404.
fn not_found_server() -> String {
    use std::io::{Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/clip.mp4", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = stream.read(&mut [0; 4096]);
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    url
}

#[test]
fn a_failed_download_leaves_later_playbacks_working() {
    if !available() {
        return;
    }
    // Used to crash the process (access violation) on the second open.
    let url = not_found_server();
    for _ in 0..3 {
        assert!(matches!(Decoder::open(&url), Err(MediaError::Open(_))));
    }
    let other = std::thread::spawn(move || matches!(Decoder::open(&url), Err(MediaError::Open(_))));
    assert!(other.join().unwrap(), "on another thread too");
    assert_eq!(all_frames(&mut open()).len(), 8, "and files still play");
}

#[test]
fn decoders_work_on_other_threads() {
    if !available() {
        return;
    }
    let handles: Vec<_> =
        (0..2).map(|_| std::thread::spawn(|| all_frames(&mut open()).len())).collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), 8);
    }
}
