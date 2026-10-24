//! The notification sound. Each play decodes and plays on its own thread,
//! so the UI thread never waits on the audio device.

use std::{io::Cursor, sync::Arc};

use anyhow::Context as _;
use gpui_kit::{App, Global};

use crate::state::deck::Volume;

/// The sound columns play when new posts arrive.
const NOTIFICATION: &[u8] = include_bytes!("../resources/notification.mp3");

/// Something that plays the notification sound.
pub trait SoundPlayer: Send + Sync {
    fn play(&self, volume: Volume);
}

/// The app's sound player, as a GPUI global. Without one (tests, by default)
/// nothing plays.
#[derive(Clone)]
pub struct Sound(pub Arc<dyn SoundPlayer>);

impl Global for Sound {}

impl std::fmt::Debug for Sound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sound").finish_non_exhaustive()
    }
}

/// Plays the notification sound at `volume`, if a player is installed.
pub fn play(volume: Volume, cx: &App) {
    if volume.percent() == 0 {
        return;
    }
    if let Some(sound) = cx.try_global::<Sound>() {
        sound.0.play(volume);
    }
}

/// Plays through the default audio output device.
#[derive(Debug, Default)]
pub struct SystemSound;

impl SoundPlayer for SystemSound {
    fn play(&self, volume: Volume) {
        let spawned =
            std::thread::Builder::new().name("notification-sound".into()).spawn(move || {
                if let Err(e) = play_blocking(volume) {
                    tracing::warn!("could not play the notification sound: {e:#}");
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("could not start the sound thread: {e}");
        }
    }
}

/// Plays the sound to the end. The device sink must outlive the playback, or
/// the sound stops as soon as it is dropped.
fn play_blocking(volume: Volume) -> anyhow::Result<()> {
    let mut sink =
        rodio::DeviceSinkBuilder::open_default_sink().context("no audio output device")?;
    // Rodio otherwise prints to stderr when the sink is dropped.
    sink.log_on_drop(false);
    let player = rodio::play(sink.mixer(), Cursor::new(NOTIFICATION))
        .context("could not decode the sound")?;
    player.set_volume(volume.gain());
    player.sleep_until_end();
    Ok(())
}

/// A player that records what it was asked to play, for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct RecordingSound(std::sync::Mutex<Vec<Volume>>);

#[cfg(test)]
impl RecordingSound {
    /// Installs a recorder as the app's sound player and returns it.
    pub fn install(cx: &mut App) -> Arc<Self> {
        let recorder = Arc::new(Self::default());
        cx.set_global(Sound(recorder.clone()));
        recorder
    }

    /// The volumes played so far.
    pub fn played(&self) -> Vec<Volume> {
        self.0.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
impl SoundPlayer for RecordingSound {
    fn play(&self, volume: Volume) {
        if let Ok(mut played) = self.0.lock() {
            played.push(volume);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sound_decodes() {
        let decoder = rodio::Decoder::new(Cursor::new(NOTIFICATION)).unwrap();
        let duration = rodio::Source::total_duration(&decoder);
        assert!(duration.is_none_or(|d| d.as_secs() < 10), "a short chime");
    }

    #[gpui_kit::test]
    fn muted_sounds_are_skipped(cx: &mut gpui_kit::TestAppContext) {
        let recorder = cx.update(RecordingSound::install);
        cx.update(|cx| {
            play(Volume::from(0), cx);
            play(Volume::from(40), cx);
        });
        assert_eq!(recorder.played(), [Volume::from(40)]);
    }
}
