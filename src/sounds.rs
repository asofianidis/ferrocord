//! Short interface sounds for voice: joining and leaving, other people coming
//! and going, and muting or deafening.
//!
//! The chimes are synthesized here (no sound files), and each plays on its
//! own short-lived output stream, so a sound still finishes after a voice
//! connection has been torn down (the leave sound, for one).

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use std::{collections::VecDeque, sync::Arc, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sound {
    Join,
    Leave,
    /// Someone else joined the voice channel we are in.
    OtherJoin,
    OtherLeave,
    Mute,
    Unmute,
    Deafen,
    Undeafen,
}

/// A note: frequency in Hz, start and length in seconds, relative loudness.
struct Note(f32, f32, f32, f32);

const E5: f32 = 659.26;
const A5: f32 = 880.0;
const D5: f32 = 587.33;
const A4: f32 = 440.0;
const E4: f32 = 329.63;
const A3: f32 = 220.0;

impl Sound {
    fn notes(self) -> Vec<Note> {
        match self {
            Sound::Join => vec![Note(E5, 0.0, 0.16, 1.0), Note(A5, 0.09, 0.24, 1.0)],
            Sound::Leave => vec![Note(A5, 0.0, 0.16, 1.0), Note(E5, 0.09, 0.24, 1.0)],
            Sound::OtherJoin => vec![Note(E5, 0.0, 0.12, 0.6), Note(A5, 0.07, 0.18, 0.6)],
            Sound::OtherLeave => vec![Note(A5, 0.0, 0.12, 0.6), Note(E5, 0.07, 0.18, 0.6)],
            Sound::Mute => vec![Note(D5, 0.0, 0.08, 0.8), Note(A4, 0.06, 0.12, 0.8)],
            Sound::Unmute => vec![Note(A4, 0.0, 0.08, 0.8), Note(D5, 0.06, 0.12, 0.8)],
            Sound::Deafen => vec![Note(E4, 0.0, 0.1, 0.9), Note(A3, 0.07, 0.16, 0.9)],
            Sound::Undeafen => vec![Note(A3, 0.0, 0.1, 0.9), Note(E4, 0.07, 0.16, 0.9)],
        }
    }
}

/// Renders a sound as mono samples at `rate`, scaled by `volume`.
pub fn render(sound: Sound, rate: u32, volume: f32) -> Vec<f32> {
    let notes = sound.notes();
    let end = notes.iter().map(|n| n.1 + n.2).fold(0.0, f32::max);
    let len = (end * rate as f32).ceil() as usize;
    let mut out = vec![0.0f32; len];
    let attack = 0.005;
    for Note(freq, start, length, gain) in notes {
        let first = (start * rate as f32) as usize;
        let count = (length * rate as f32) as usize;
        for i in 0..count {
            let t = i as f32 / rate as f32;
            // A quick fade in, then a smooth decay that reaches silence exactly
            // at the end, so neither edge clicks.
            let rise = (t / attack).min(1.0);
            let fall = (1.0 - t / length).max(0.0).powi(2);
            let phase = std::f32::consts::TAU * freq * t;
            // A touch of the octave makes it a chime rather than a beep.
            let tone = phase.sin() + 0.25 * (2.0 * phase).sin();
            if let Some(slot) = out.get_mut(first + i) {
                *slot += tone * rise * fall * gain;
            }
        }
    }
    let level = 0.22 * volume;
    for s in &mut out {
        *s = (*s * level).clamp(-1.0, 1.0);
    }
    out
}

/// Plays a sound on the chosen output device, unless sounds are turned off.
/// Returns at once; the sound plays on its own thread.
pub fn play(sound: Sound) {
    let settings = crate::settings::Settings::load();
    if !settings.voice_sounds {
        return;
    }
    let device_name = settings.output_device;
    let volume = settings.output_volume;
    std::thread::Builder::new()
        .name("ui-sound".into())
        .spawn(move || {
            if let Err(e) = play_blocking(sound, device_name.as_deref(), volume) {
                log::warn!("could not play a sound: {e}");
            }
        })
        .ok();
}

fn play_blocking(sound: Sound, wanted: Option<&str>, volume: f32) -> Result<(), String> {
    let host = cpal::default_host();
    let device = wanted
        .and_then(|name| host.output_devices().ok()?.find(|d| d.name().is_ok_and(|n| n == name)))
        .or_else(|| host.default_output_device())
        .ok_or("no output device")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let samples = render(sound, rate, volume);
    let duration = Duration::from_secs_f32(samples.len() as f32 / rate as f32);
    let queue = Arc::new(Mutex::new(VecDeque::from(samples)));

    fn build<T: cpal::SizedSample + cpal::FromSample<f32>>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        channels: usize,
        queue: Arc<Mutex<VecDeque<f32>>>,
    ) -> Result<cpal::Stream, cpal::BuildStreamError> {
        device.build_output_stream(
            config,
            move |data: &mut [T], _| {
                let mut queue = queue.lock();
                for frame in data.chunks_mut(channels.max(1)) {
                    let value = T::from_sample(queue.pop_front().unwrap_or(0.0));
                    frame.fill(value);
                }
            },
            |e| log::warn!("sound stream error: {e}"),
            None,
        )
    }

    let stream_config = config.config();
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, &stream_config, channels, queue),
        cpal::SampleFormat::I16 => build::<i16>(&device, &stream_config, channels, queue),
        cpal::SampleFormat::U16 => build::<u16>(&device, &stream_config, channels, queue),
        cpal::SampleFormat::I32 => build::<i32>(&device, &stream_config, channels, queue),
        other => return Err(format!("unsupported output format {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    // Let it finish, plus the device's own buffering, before closing.
    std::thread::sleep(duration + Duration::from_millis(250));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_are_short_quiet_and_click_free() {
        for sound in [
            Sound::Join,
            Sound::Leave,
            Sound::OtherJoin,
            Sound::OtherLeave,
            Sound::Mute,
            Sound::Unmute,
            Sound::Deafen,
            Sound::Undeafen,
        ] {
            let samples = render(sound, 48_000, 1.0);
            let seconds = samples.len() as f32 / 48_000.0;
            assert!((0.1..0.5).contains(&seconds), "{sound:?} lasts {seconds}s");
            assert!(samples.iter().all(|s| s.abs() <= 1.0));
            assert!(samples[0].abs() < 0.01 && samples.last().unwrap().abs() < 0.01, "{sound:?} starts and ends silent");
            assert!(samples.iter().any(|s| s.abs() > 0.05), "{sound:?} is audible");
        }
    }

    #[test]
    fn volume_scales_sounds() {
        let full = render(Sound::Join, 48_000, 1.0);
        let half = render(Sound::Join, 48_000, 0.5);
        let peak = |v: &[f32]| v.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak(&half) - peak(&full) / 2.0).abs() < 1e-4);
        assert!(render(Sound::Join, 48_000, 0.0).iter().all(|s| *s == 0.0));
    }
}
