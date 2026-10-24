//! Decodes the sound of `fixtures/tone.mp4` (see `examples/make_fixture.rs`):
//! 1 s of 10 fps video with AAC stereo at 44.1 kHz, a 440 Hz tone on the left
//! and silence on the right. Skipped, saying so, without Media Foundation
//! (see `decode.rs`).

#![cfg(windows)]
#![expect(clippy::unwrap_used, clippy::print_stdout, reason = "test helpers outside #[test] fns")]

use std::time::Duration;

use twitty_media::{AudioChunk, AudioFormat, Decoder, MediaError, Packet, Tracks, VideoFrame};

fn fixture() -> String {
    format!("{}/tests/fixtures/tone.mp4", env!("CARGO_MANIFEST_DIR"))
}

fn open() -> Decoder {
    Decoder::open(&fixture(), Tracks::VideoAndAudio).unwrap()
}

fn available() -> bool {
    match Decoder::open(&fixture(), Tracks::VideoAndAudio) {
        Err(MediaError::Unavailable(why)) => {
            println!("skipped: no Media Foundation here ({why})");
            false
        }
        _ => true,
    }
}

fn packets(decoder: &mut Decoder) -> (Vec<VideoFrame>, Vec<AudioChunk>) {
    let (mut video, mut audio) = (Vec::new(), Vec::new());
    while let Some(packet) = decoder.next_packet().unwrap() {
        match packet {
            Packet::Video(frame) => video.push(frame),
            Packet::Audio(chunk) => audio.push(chunk),
        }
    }
    (video, audio)
}

/// Root mean square of one channel of interleaved stereo.
fn rms(samples: &[f32], channel: usize) -> f32 {
    let channel: Vec<f32> = samples.iter().skip(channel).step_by(2).copied().collect();
    (channel.iter().map(|s| s * s).sum::<f32>() / channel.len().max(1) as f32).sqrt()
}

#[test]
fn the_sound_is_float_stereo_at_the_file_rate() {
    if !available() {
        return;
    }
    assert_eq!(open().audio_format(), Some(AudioFormat { sample_rate: 44_100, channels: 2 }));
}

#[test]
fn video_and_sound_come_out_together_in_time_order() {
    if !available() {
        return;
    }
    let (video, audio) = packets(&mut open());
    assert_eq!(video.len(), 10);
    assert!(!audio.is_empty());
    assert!(audio.windows(2).all(|w| w[0].time <= w[1].time), "chunks in order");
    let samples: usize = audio.iter().map(|c| c.samples.len()).sum();
    let seconds = samples as f64 / 2.0 / 44_100.0;
    // AAC adds priming and pads the last block.
    assert!((0.9..1.2).contains(&seconds), "about a second of sound: {seconds}");
}

#[test]
fn the_tone_is_on_the_left_and_the_right_is_silent() {
    if !available() {
        return;
    }
    let (_, audio) = packets(&mut open());
    let all: Vec<f32> = audio.into_iter().flat_map(|c| c.samples).collect();
    assert!(rms(&all, 0) > 0.2, "left: {}", rms(&all, 0));
    assert!(rms(&all, 1) < 0.02, "right: {}", rms(&all, 1));
}

#[test]
fn a_seek_starts_both_tracks_at_the_target_and_keeps_the_channels() {
    if !available() {
        return;
    }
    let mut decoder = open();
    let target = Duration::from_millis(450);
    decoder.seek(target).unwrap();
    let (video, audio) = packets(&mut decoder);
    let first = video.first().unwrap();
    assert!(first.time <= target && target < first.time + first.duration, "on screen then");
    assert_eq!(video.len(), 6, "frames 4 to 9");
    let start = audio.first().unwrap().time;
    assert!(start >= target && start < target + Duration::from_millis(1), "starts at {start:?}");
    let all: Vec<f32> = audio.into_iter().flat_map(|c| c.samples).collect();
    assert!(rms(&all, 0) > 0.2 && rms(&all, 1) < 0.02, "left still left");
}

#[test]
fn a_video_only_decoder_skips_the_sound() {
    if !available() {
        return;
    }
    let mut decoder = Decoder::open(&fixture(), Tracks::Video).unwrap();
    assert_eq!(decoder.audio_format(), None);
    let (video, audio) = packets(&mut decoder);
    assert_eq!((video.len(), audio.len()), (10, 0));
}

#[test]
fn a_silent_file_has_no_sound_track() {
    if !available() {
        return;
    }
    let bars = format!("{}/tests/fixtures/bars.mp4", env!("CARGO_MANIFEST_DIR"));
    let decoder = Decoder::open(&bars, Tracks::VideoAndAudio).unwrap();
    assert_eq!(decoder.audio_format(), None);
}
