//! Voice chat. Songbird speaks Discord's voice protocol (including DAVE
//! end-to-end encryption); cpal captures the microphone and plays what other
//! people say. Everything here runs off the UI thread and reports back through
//! [`VoiceEvent`]s.

use crate::discord::Id;
use async_trait::async_trait;
use cpal::{
    FromSample, Sample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use parking_lot::{Condvar, Mutex};
use songbird::{
    Config, ConnectionInfo, CoreEvent, Driver, Event as SongbirdEvent, EventContext, EventHandler,
    driver::{Channels, DecodeConfig, DecodeMode, SampleRate},
    id::{ChannelId, GuildId, UserId},
    input::{Input, RawAdapter},
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{self, Read, Seek, SeekFrom},
    num::NonZeroU64,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

/// Discord voice runs at 48 kHz; one tick carries 20 ms of stereo audio.
const VOICE_RATE: u32 = 48_000;
/// How long the microphone stays open after the last loud sample.
const GATE_HANGOVER: Duration = Duration::from_millis(400);
/// How long a remote user counts as speaking after their last loud frame.
const SPEAKING_HOLD: Duration = Duration::from_millis(300);
/// Received frames quieter than this (RMS of i16 samples) are not "speaking".
const SPEAKING_RMS: f32 = 300.0;
/// How long the microphone fades in or out when the voice gate opens or closes.
/// Switching instantly would cut the waveform mid-swing, which clicks.
const GATE_FADE: Duration = Duration::from_millis(10);
/// How long after the gate closes songbird stops sending. Long enough for the
/// faded-out audio still buffered to go out first.
const MUTE_DELAY: Duration = Duration::from_millis(150);
/// Length of the fade applied around playback underruns, in samples.
const UNDERRUN_FADE: usize = 96;

#[derive(Debug, Clone)]
pub enum VoiceEvent {
    Connected,
    Disconnected(String),
    /// Users currently speaking, including ourselves.
    Speaking(HashSet<Id>),
    /// An audio device could not be used. Voice keeps working without it.
    DeviceProblem(String),
}

#[derive(Debug, Clone, Default)]
pub struct VoiceOptions {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    /// Voice activity threshold in dBFS; quieter input is not sent.
    pub sensitivity_db: f32,
    /// Linear gains: 1.0 leaves the audio as it is.
    pub input_volume: f32,
    pub output_volume: f32,
}

/// Loudness in dBFS of samples' RMS, floored at -100.
pub fn rms_db(samples: &[f32]) -> f32 {
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt();
    (20.0 * rms.max(1e-5).log10()).max(-100.0)
}

/// Captures the microphone only to measure how loud it is, for the level
/// meter in settings. Drop it to close the device.
pub struct MicTest {
    _stream: cpal::Stream,
    level: Arc<AtomicU32>,
    gain: Arc<AtomicU32>,
}

impl MicTest {
    pub fn start(device: Option<&str>, volume: f32) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .input_devices()
            .ok()
            .and_then(|list| pick_device(list, device))
            .or_else(|| host.default_input_device())
            .ok_or("No microphone was found.")?;
        let config = device.default_input_config().map_err(|e| format!("Microphone unavailable: {e}"))?;
        let channels = config.channels() as usize;
        let level = Arc::new(AtomicU32::new((-100f32).to_bits()));
        let gain = Arc::new(AtomicU32::new(volume.to_bits()));

        fn build<T>(
            device: &cpal::Device,
            config: &cpal::StreamConfig,
            channels: usize,
            level: Arc<AtomicU32>,
            gain: Arc<AtomicU32>,
        ) -> Result<cpal::Stream, cpal::BuildStreamError>
        where
            T: SizedSample,
            f32: FromSample<T>,
        {
            let mut mono = Vec::new();
            device.build_input_stream(
                config,
                move |data: &[T], _| {
                    let g = f32::from_bits(gain.load(Ordering::Relaxed));
                    mono.clear();
                    mono.extend(data.chunks(channels.max(1)).map(|frame| {
                        g * frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / frame.len() as f32
                    }));
                    level.store(rms_db(&mono).to_bits(), Ordering::Relaxed);
                },
                |e| log::warn!("microphone test error: {e}"),
                None,
            )
        }

        let stream_config = config.config();
        let (l, g) = (level.clone(), gain.clone());
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, &stream_config, channels, l, g),
            cpal::SampleFormat::I16 => build::<i16>(&device, &stream_config, channels, l, g),
            cpal::SampleFormat::U16 => build::<u16>(&device, &stream_config, channels, l, g),
            cpal::SampleFormat::I32 => build::<i32>(&device, &stream_config, channels, l, g),
            other => return Err(format!("Unsupported microphone format {other:?}.")),
        }
        .map_err(|e| format!("Could not open the microphone: {e}"))?;
        stream.play().map_err(|e| format!("Could not start the microphone: {e}"))?;
        Ok(Self { _stream: stream, level, gain })
    }

    /// The latest loudness, in dBFS.
    pub fn level_db(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn set_volume(&self, volume: f32) {
        self.gain.store(volume.to_bits(), Ordering::Relaxed);
    }
}

/// Where to connect, as told by `VOICE_STATE_UPDATE` and `VOICE_SERVER_UPDATE`.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub channel: Id,
    /// The server, or the call's own channel for direct-message calls.
    pub guild_or_channel: Id,
    pub user: Id,
    pub session_id: String,
    pub token: String,
    pub endpoint: String,
}

pub struct Devices {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

/// Lists audio devices. Slow on some systems; call it off the UI thread.
pub fn list_devices() -> Devices {
    let host = cpal::default_host();
    let names = |devices: Result<_, _>| -> Vec<String> {
        match devices {
            Ok(list) => {
                let list: Box<dyn Iterator<Item = cpal::Device>> = list;
                list.filter_map(|d| d.name().ok()).collect()
            }
            Err(_) => Vec::new(),
        }
    };
    Devices {
        inputs: names(host.input_devices().map(|d| Box::new(d) as Box<dyn Iterator<Item = cpal::Device>>)),
        outputs: names(host.output_devices().map(|d| Box::new(d) as Box<dyn Iterator<Item = cpal::Device>>)),
    }
}

pub fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

// ------------------------------------------------------------------ buffers

/// A FIFO that waits for a little audio before playing, so that bursty
/// producers do not cause constant underruns, and that sheds old audio when
/// it falls too far behind.
///
/// Running dry fades the last samples out, and starting again fades in, so
/// an underrun is a short dip rather than a click. Drift between the
/// producer's and consumer's clocks is corrected only while the audio is
/// quiet, where adding or dropping a little silence cannot be heard.
pub struct Jitter {
    buf: VecDeque<f32>,
    primed: bool,
    prebuffer: usize,
    max: usize,
    fade: usize,
    /// Samples of fade-in still to apply after (re)starting.
    fading_in: usize,
}

impl Jitter {
    pub fn new(prebuffer: usize, max: usize) -> Self {
        Self { buf: VecDeque::with_capacity(max), primed: false, prebuffer, max, fade: 0, fading_in: 0 }
    }

    /// Fades over `samples` around underruns.
    pub fn with_fade(mut self, samples: usize) -> Self {
        self.fade = samples;
        self
    }

    /// Adds audio. `quiet` audio may be dropped, or padded with extra silence,
    /// to steer the backlog back towards the prebuffer level.
    pub fn push_steered(&mut self, samples: &[f32], quiet: bool) {
        if quiet && self.primed {
            if self.buf.len() > self.prebuffer * 2 {
                return; // Running behind: skip some silence.
            }
            if self.buf.len() < self.prebuffer / 2 {
                // Running dry: add a little extra silence.
                self.buf.extend(std::iter::repeat_n(0.0, samples.len()));
            }
        }
        self.push(samples);
    }

    pub fn push(&mut self, samples: &[f32]) {
        self.buf.extend(samples.iter().copied());
        if self.buf.len() > self.max {
            // Badly behind: keep only the newest prebuffer's worth.
            let excess = self.buf.len() - self.prebuffer;
            self.buf.drain(..excess);
        }
    }

    /// Fills `out` completely, with silence where there is no audio.
    pub fn pull(&mut self, out: &mut [f32]) {
        if !self.primed {
            if self.buf.len() < self.prebuffer || self.buf.is_empty() {
                out.fill(0.0);
                return;
            }
            self.primed = true;
            self.fading_in = self.fade;
        }
        let n = out.len().min(self.buf.len());
        for (slot, sample) in out.iter_mut().zip(self.buf.drain(..n)) {
            *slot = sample;
        }
        let mut i = 0;
        while self.fading_in > 0 && i < n {
            *out.get_mut(i).unwrap() *= 1.0 - self.fading_in as f32 / (self.fade + 1) as f32;
            self.fading_in -= 1;
            i += 1;
        }
        if n < out.len() {
            // Underrun: ease the last real samples down to silence.
            let k = n.min(self.fade);
            for j in 0..k {
                out[n - k + j] *= (k - j) as f32 / (k + 1) as f32;
            }
            out[n..].fill(0.0);
            self.primed = false;
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.buf.len()
    }
}

/// The microphone's buffer between the capture device and songbird.
///
/// Songbird reads through symphonia, which asks for large blocks (up to
/// 170 ms) at a time. Handing back only the audio that is really there,
/// minus a small reserve, keeps it from being padded with silence.
pub struct MicBuffer {
    buf: VecDeque<f32>,
    primed: bool,
    /// Audio gathered before the first read, and the level drift is steered to.
    target: usize,
    /// Kept back between reads to ride out uneven capture callbacks.
    reserve: usize,
    /// Hard limit; beyond it the oldest audio is dropped.
    max: usize,
}

impl MicBuffer {
    pub fn new(rate: u32) -> Self {
        let ms = |n: usize| rate as usize * n / 1000;
        Self { buf: VecDeque::new(), primed: false, target: ms(40), reserve: ms(20), max: ms(1000) }
    }

    pub fn push(&mut self, samples: &[f32], quiet: bool) {
        // The device runs ahead of songbird: drop some silence to catch up.
        if quiet && self.buf.len() > self.target * 3 {
            return;
        }
        self.buf.extend(samples.iter().copied());
        if self.buf.len() > self.max {
            let excess = self.buf.len() - self.target;
            self.buf.drain(..excess);
        }
    }

    /// Whether a read would have to dig into the reserve.
    pub fn low(&self) -> bool {
        !self.primed || self.buf.len() <= self.reserve
    }

    /// Moves up to `want` samples into `out`, keeping the reserve when it can.
    /// Adds nothing if there is no audio to give.
    pub fn take(&mut self, want: usize, out: &mut Vec<f32>) {
        if !self.primed {
            if self.buf.len() < self.target {
                return;
            }
            self.primed = true;
        }
        let spare = self.buf.len().saturating_sub(self.reserve);
        let n = if spare > 0 { want.min(spare) } else { want.min(self.buf.len()) };
        out.extend(self.buf.drain(..n));
        if self.buf.is_empty() {
            self.primed = false;
        }
    }

    /// Brings the backlog back to the target level. Only for when nobody can
    /// hear the result, because it skips or repeats nothing smoothly.
    pub fn rebalance(&mut self) {
        if self.buf.len() > self.target * 2 {
            let excess = self.buf.len() - self.target;
            self.buf.drain(..excess);
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.buf.len()
    }
}

/// Streaming linear resampler for interleaved stereo.
pub struct StereoResampler {
    step: f64,
    pos: f64,
    prev: [f32; 2],
}

impl StereoResampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self { step: from as f64 / to.max(1) as f64, pos: 0.0, prev: [0.0; 2] }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let frames = input.len() / 2;
        if frames == 0 {
            return;
        }
        if (self.step - 1.0).abs() < f64::EPSILON {
            out.extend_from_slice(&input[..frames * 2]);
            return;
        }
        // Index 0 is the last frame of the previous call, 1.. are this call's frames.
        let frame = |i: usize| -> [f32; 2] {
            if i == 0 { self.prev } else { [input[(i - 1) * 2], input[(i - 1) * 2 + 1]] }
        };
        while self.pos < frames as f64 {
            let i = self.pos.floor() as usize;
            let t = (self.pos - i as f64) as f32;
            let (a, b) = (frame(i), frame(i + 1));
            out.push(a[0] + (b[0] - a[0]) * t);
            out.push(a[1] + (b[1] - a[1]) * t);
            self.pos += self.step;
        }
        self.pos -= frames as f64;
        self.prev = frame(frames);
    }
}

// ------------------------------------------------------------------ shared state

/// What happened to our audio over the last second, for diagnosing "they
/// can't hear me": was the mic loud enough, did the gate open, and did
/// songbird actually take audio from us.
#[derive(Default)]
struct Stats {
    /// Loudest capture callback, in dBFS (after input volume).
    mic_peak_db: Option<f32>,
    captured: usize,
    /// Samples songbird read, and how many of those were real (not filler).
    read: usize,
    read_real: usize,
}

struct Shared {
    /// Mono microphone samples at the input device's rate.
    mic: Mutex<MicBuffer>,
    /// Signalled whenever the microphone delivers audio.
    mic_ready: Condvar,
    /// Interleaved stereo at the output device's rate.
    out: Mutex<Jitter>,
    resampler: Mutex<StereoResampler>,
    muted: AtomicBool,
    deafened: AtomicBool,
    /// Linear RMS threshold, stored as f32 bits.
    threshold: AtomicU32,
    /// Linear microphone and playback gains, stored as f32 bits.
    input_gain: AtomicU32,
    output_gain: AtomicU32,
    /// When the microphone last heard something above the threshold.
    last_loud: Mutex<Option<Instant>>,
    /// Counters for the once-a-second voice log line.
    stats: Mutex<Stats>,
    has_mic: AtomicBool,
    ssrc_users: Mutex<HashMap<u32, u64>>,
    /// When each SSRC last carried audible voice.
    heard: Mutex<HashMap<u32, Instant>>,
    events: async_channel::Sender<VoiceEvent>,
}

impl Shared {
    fn threshold(&self) -> f32 {
        f32::from_bits(self.threshold.load(Ordering::Relaxed))
    }

    /// Whether our microphone should be transmitting right now.
    fn transmitting(&self) -> bool {
        self.has_mic.load(Ordering::Relaxed)
            && !self.muted.load(Ordering::Relaxed)
            && !self.deafened.load(Ordering::Relaxed)
            && self.last_loud.lock().is_some_and(|t| t.elapsed() < GATE_HANGOVER)
    }
}

/// Serves microphone audio to songbird as an endless raw f32 stream.
///
/// Reads return only the audio that has been captured, so short reads are
/// normal. Songbird's mixer thread makes these reads, so they wait at most a
/// few milliseconds; if the microphone has fallen silent, a little silence
/// is returned instead. The voice gate is applied here as a gain that fades
/// rather than switching instantly.
struct MicSource {
    shared: Arc<Shared>,
    pending: VecDeque<u8>,
    scratch: Vec<f32>,
    /// Current gate gain, 0 (closed) to 1 (open).
    gain: f32,
    /// Gain change per sample while fading.
    gain_step: f32,
    /// Samples of silence to return when there is nothing to send (10 ms).
    silence: usize,
}

impl MicSource {
    fn new(shared: Arc<Shared>, rate: u32) -> Self {
        let fade_samples = (rate as f32 * GATE_FADE.as_secs_f32()).max(1.0);
        Self {
            shared,
            pending: VecDeque::new(),
            scratch: Vec::new(),
            gain: 0.0,
            gain_step: 1.0 / fade_samples,
            silence: (rate as usize / 100).max(1),
        }
    }

    fn refill(&mut self, want: usize) {
        let open = self.shared.transmitting();
        self.scratch.clear();
        {
            let mut mic = self.shared.mic.lock();
            if !open && self.gain == 0.0 {
                // Nobody hears us right now, so drift can be corrected freely.
                mic.rebalance();
            }
            if mic.low() {
                self.shared.mic_ready.wait_for(&mut mic, Duration::from_millis(5));
            }
            mic.take(want, &mut self.scratch);
        }
        let real = self.scratch.len();
        if self.scratch.is_empty() {
            self.scratch.resize(want.min(self.silence), 0.0);
        }
        {
            let mut stats = self.shared.stats.lock();
            stats.read += self.scratch.len();
            stats.read_real += real;
        }
        let target = if open { 1.0 } else { 0.0 };
        for sample in &mut self.scratch {
            if self.gain < target {
                self.gain = (self.gain + self.gain_step).min(1.0);
            } else if self.gain > target {
                self.gain = (self.gain - self.gain_step).max(0.0);
            }
            *sample *= self.gain;
        }
        for s in &self.scratch {
            self.pending.extend(s.to_le_bytes());
        }
    }
}

impl Read for MicSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.pending.is_empty() {
            self.refill(buf.len().div_ceil(4));
        }
        let len = buf.len().min(self.pending.len());
        for (slot, byte) in buf.iter_mut().zip(self.pending.drain(..len)) {
            *slot = byte;
        }
        Ok(len)
    }
}

impl Seek for MicSource {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

impl symphonia_core::io::MediaSource for MicSource {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

struct Receiver {
    shared: Arc<Shared>,
}

#[async_trait]
impl EventHandler for Receiver {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<SongbirdEvent> {
        let shared = &self.shared;
        match ctx {
            EventContext::SpeakingStateUpdate(speaking) => {
                if let Some(user) = speaking.user_id {
                    shared.ssrc_users.lock().insert(speaking.ssrc, user.0);
                }
            }
            EventContext::VoiceTick(tick) => {
                let deaf = shared.deafened.load(Ordering::Relaxed);
                let mut mix = vec![0f32; (VOICE_RATE as usize / 50) * 2];
                let now = Instant::now();
                let mut quiet = true;
                for (ssrc, data) in &tick.speaking {
                    let Some(pcm) = &data.decoded_voice else { continue };
                    let energy = pcm.iter().map(|s| (*s as f32).powi(2)).sum::<f32>() / pcm.len().max(1) as f32;
                    if energy.sqrt() > SPEAKING_RMS {
                        shared.heard.lock().insert(*ssrc, now);
                    }
                    if !deaf {
                        quiet = false;
                        for (slot, s) in mix.iter_mut().zip(pcm) {
                            *slot += *s as f32 / 32768.0;
                        }
                    }
                }
                for s in &mut mix {
                    *s = s.clamp(-1.0, 1.0);
                }
                let mut resampled = Vec::with_capacity(mix.len() + 16);
                shared.resampler.lock().process(&mix, &mut resampled);
                shared.out.lock().push_steered(&resampled, quiet);
            }
            EventContext::DriverConnect(_) | EventContext::DriverReconnect(_) => {
                let _ = shared.events.try_send(VoiceEvent::Connected);
            }
            EventContext::DriverDisconnect(data) => {
                let reason = match data.reason {
                    Some(reason) => format!("{reason:?}"),
                    None => "Left the channel.".to_string(),
                };
                let _ = shared.events.try_send(VoiceEvent::Disconnected(reason));
            }
            _ => {}
        }
        None
    }
}

// ------------------------------------------------------------------ devices

fn pick_device(devices: impl Iterator<Item = cpal::Device>, wanted: Option<&str>) -> Option<cpal::Device> {
    let wanted = wanted?;
    devices.into_iter().find(|d| d.name().is_ok_and(|n| n == wanted))
}

fn open_input(shared: Arc<Shared>, wanted: Option<&str>) -> Result<(cpal::Stream, u32), String> {
    let host = cpal::default_host();
    let device = host
        .input_devices()
        .ok()
        .and_then(|list| pick_device(list, wanted))
        .or_else(|| host.default_input_device())
        .ok_or("No microphone was found.")?;
    let config = device.default_input_config().map_err(|e| format!("Microphone unavailable: {e}"))?;
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let stream_config = config.config();

    fn build<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        channels: usize,
        shared: Arc<Shared>,
    ) -> Result<cpal::Stream, cpal::BuildStreamError>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let mut mono = Vec::new();
        device.build_input_stream(
            config,
            move |data: &[T], _| {
                let gain = f32::from_bits(shared.input_gain.load(Ordering::Relaxed));
                mono.clear();
                mono.extend(data.chunks(channels.max(1)).map(|frame| {
                    (gain * frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / frame.len() as f32).clamp(-1.0, 1.0)
                }));
                let rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len().max(1) as f32).sqrt();
                let loud = rms >= shared.threshold();
                {
                    let mut stats = shared.stats.lock();
                    let db = rms_db(&mono);
                    stats.mic_peak_db = Some(stats.mic_peak_db.map_or(db, |p| p.max(db)));
                    stats.captured += mono.len();
                }
                if loud {
                    *shared.last_loud.lock() = Some(Instant::now());
                }
                shared.mic.lock().push(&mono, !loud);
                shared.mic_ready.notify_one();
            },
            |e| log::warn!("microphone stream error: {e}"),
            None,
        )
    }

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, &stream_config, channels, shared),
        cpal::SampleFormat::I16 => build::<i16>(&device, &stream_config, channels, shared),
        cpal::SampleFormat::U16 => build::<u16>(&device, &stream_config, channels, shared),
        cpal::SampleFormat::I32 => build::<i32>(&device, &stream_config, channels, shared),
        other => return Err(format!("Unsupported microphone format {other:?}.")),
    }
    .map_err(|e| format!("Could not open the microphone: {e}"))?;
    stream.play().map_err(|e| format!("Could not start the microphone: {e}"))?;
    Ok((stream, rate))
}

fn open_output(shared: Arc<Shared>, wanted: Option<&str>) -> Result<(cpal::Stream, u32), String> {
    let host = cpal::default_host();
    let device = host
        .output_devices()
        .ok()
        .and_then(|list| pick_device(list, wanted))
        .or_else(|| host.default_output_device())
        .ok_or("No speakers or headphones were found.")?;
    let config = device.default_output_config().map_err(|e| format!("Output device unavailable: {e}"))?;
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let stream_config = config.config();

    fn build<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        channels: usize,
        shared: Arc<Shared>,
    ) -> Result<cpal::Stream, cpal::BuildStreamError>
    where
        T: SizedSample + FromSample<f32>,
    {
        let mut stereo = Vec::new();
        device.build_output_stream(
            config,
            move |data: &mut [T], _| {
                let frames = data.len() / channels.max(1);
                stereo.resize(frames * 2, 0.0);
                shared.out.lock().pull(&mut stereo);
                let gain = f32::from_bits(shared.output_gain.load(Ordering::Relaxed));
                if gain != 1.0 {
                    for s in stereo.iter_mut() {
                        *s = (*s * gain).clamp(-1.0, 1.0);
                    }
                }
                for (frame, lr) in data.chunks_mut(channels.max(1)).zip(stereo.chunks(2)) {
                    for (ch, slot) in frame.iter_mut().enumerate() {
                        let value = match (channels, ch) {
                            (1, _) => (lr[0] + lr[1]) * 0.5,
                            (_, 0) => lr[0],
                            (_, 1) => lr[1],
                            _ => 0.0,
                        };
                        *slot = T::from_sample(value);
                    }
                }
            },
            |e| log::warn!("audio output stream error: {e}"),
            None,
        )
    }

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, &stream_config, channels, shared),
        cpal::SampleFormat::I16 => build::<i16>(&device, &stream_config, channels, shared),
        cpal::SampleFormat::U16 => build::<u16>(&device, &stream_config, channels, shared),
        cpal::SampleFormat::I32 => build::<i32>(&device, &stream_config, channels, shared),
        other => return Err(format!("Unsupported output format {other:?}.")),
    }
    .map_err(|e| format!("Could not open the audio output: {e}"))?;
    stream.play().map_err(|e| format!("Could not start the audio output: {e}"))?;
    Ok((stream, rate))
}

// ------------------------------------------------------------------ connection

/// One voice session: audio devices plus the songbird driver.
///
/// Holds cpal streams, which must stay on the thread that created them, so
/// this lives on the UI thread inside the store.
pub struct VoiceConnection {
    runtime: tokio::runtime::Handle,
    driver: Arc<tokio::sync::Mutex<Driver>>,
    shared: Arc<Shared>,
    _input: Option<cpal::Stream>,
    _output: Option<cpal::Stream>,
    pump: tokio::task::JoinHandle<()>,
}

impl VoiceConnection {
    pub fn start(
        runtime: tokio::runtime::Handle,
        options: &VoiceOptions,
        self_id: Id,
        muted: bool,
        deafened: bool,
        events: async_channel::Sender<VoiceEvent>,
    ) -> Self {
        let shared = Arc::new(Shared {
            // Resized for the real device rate once the microphone opens.
            mic: Mutex::new(MicBuffer::new(VOICE_RATE)),
            mic_ready: Condvar::new(),
            out: Mutex::new(Jitter::new(0, 0)),
            resampler: Mutex::new(StereoResampler::new(VOICE_RATE, VOICE_RATE)),
            muted: AtomicBool::new(muted),
            deafened: AtomicBool::new(deafened),
            threshold: AtomicU32::new(db_to_linear(options.sensitivity_db).to_bits()),
            input_gain: AtomicU32::new(options.input_volume.to_bits()),
            output_gain: AtomicU32::new(options.output_volume.to_bits()),
            last_loud: Mutex::new(None),
            stats: Mutex::new(Stats::default()),
            has_mic: AtomicBool::new(false),
            ssrc_users: Mutex::new(HashMap::new()),
            heard: Mutex::new(HashMap::new()),
            events: events.clone(),
        });

        let output = match open_output(shared.clone(), options.output_device.as_deref()) {
            Ok((stream, rate)) => {
                // 60 ms of prebuffer, at most 400 ms of backlog, stereo.
                let per_ms = rate as usize * 2 / 1000;
                *shared.out.lock() = Jitter::new(per_ms * 60, per_ms * 400).with_fade(UNDERRUN_FADE);
                *shared.resampler.lock() = StereoResampler::new(VOICE_RATE, rate);
                Some(stream)
            }
            Err(problem) => {
                let _ = events.try_send(VoiceEvent::DeviceProblem(problem));
                None
            }
        };

        let config = Config::default()
            .decode_mode(DecodeMode::Decode(DecodeConfig::new(Channels::Stereo, SampleRate::Hz48000)));
        let mut driver = {
            let _guard = runtime.enter();
            Driver::new(config)
        };
        let receiver = Arc::new(Receiver { shared: shared.clone() });
        for event in [
            CoreEvent::SpeakingStateUpdate,
            CoreEvent::VoiceTick,
            CoreEvent::DriverConnect,
            CoreEvent::DriverReconnect,
            CoreEvent::DriverDisconnect,
        ] {
            driver.add_global_event(event.into(), ReceiverRef(receiver.clone()));
        }
        // Start muted; the pump un-mutes while the voice gate is open.
        driver.mute(true);

        let input = match open_input(shared.clone(), options.input_device.as_deref()) {
            Ok((stream, rate)) => {
                *shared.mic.lock() = MicBuffer::new(rate);
                shared.has_mic.store(true, Ordering::Relaxed);
                let source = MicSource::new(shared.clone(), rate);
                let input: Input = RawAdapter::new(source, rate, 1).into();
                let _guard = runtime.enter();
                driver.play_input(input);
                Some(stream)
            }
            Err(problem) => {
                let _ = events.try_send(VoiceEvent::DeviceProblem(problem));
                None
            }
        };

        let driver = Arc::new(tokio::sync::Mutex::new(driver));
        let pump = runtime.spawn(pump(driver.clone(), shared.clone(), self_id));
        Self { runtime, driver, shared, _input: input, _output: output, pump }
    }

    /// Connects (or moves) to a voice server.
    pub fn connect(&self, info: ServerInfo) {
        let nz = |id: Id| NonZeroU64::new(id.0);
        let (Some(channel), Some(guild), Some(user)) = (nz(info.channel), nz(info.guild_or_channel), nz(info.user))
        else {
            return;
        };
        let connection = ConnectionInfo {
            channel_id: ChannelId(channel),
            endpoint: info.endpoint,
            guild_id: GuildId(guild),
            session_id: info.session_id,
            token: info.token,
            user_id: UserId(user),
        };
        let driver = self.driver.clone();
        let events = self.shared.events.clone();
        self.runtime.spawn(async move {
            let connecting = driver.lock().await.connect(connection);
            if let Err(e) = connecting.await {
                let _ = events.send(VoiceEvent::Disconnected(format!("Could not connect: {e}"))).await;
            }
        });
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.shared.deafened.store(deafened, Ordering::Relaxed);
    }

    pub fn set_sensitivity(&self, db: f32) {
        self.shared.threshold.store(db_to_linear(db).to_bits(), Ordering::Relaxed);
    }

    pub fn set_input_volume(&self, volume: f32) {
        self.shared.input_gain.store(volume.to_bits(), Ordering::Relaxed);
    }

    pub fn set_output_volume(&self, volume: f32) {
        self.shared.output_gain.store(volume.to_bits(), Ordering::Relaxed);
    }
}

impl Drop for VoiceConnection {
    fn drop(&mut self) {
        self.pump.abort();
        let driver = self.driver.clone();
        self.runtime.spawn(async move { driver.lock().await.leave() });
    }
}

/// Lets one handler instance serve several event registrations.
#[derive(Clone)]
struct ReceiverRef(Arc<Receiver>);

#[async_trait]
impl EventHandler for ReceiverRef {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<SongbirdEvent> {
        self.0.act(ctx).await
    }
}

/// Every 20 ms: opens or closes our transmission to follow the voice gate,
/// and reports who is speaking.
async fn pump(driver: Arc<tokio::sync::Mutex<Driver>>, shared: Arc<Shared>, self_id: Id) {
    let mut interval = tokio::time::interval(Duration::from_millis(20));
    let mut sending = false;
    let mut closed_at: Option<Instant> = None;
    let mut last_reported: HashSet<Id> = HashSet::new();
    let (mut ticks, mut open_ticks, mut sending_ticks) = (0u32, 0u32, 0u32);
    loop {
        interval.tick().await;
        let transmit = shared.transmitting();
        ticks += 1;
        open_ticks += transmit as u32;
        sending_ticks += sending as u32;
        if ticks == 50 {
            let stats = std::mem::take(&mut *shared.stats.lock());
            log::info!(
                "voice: mic peak {} (sensitivity {:.0} dB), captured {}, songbird read {} ({} real), gate open {}%, sending {}%{}{}",
                stats.mic_peak_db.map_or("none".to_string(), |db| format!("{db:.0} dB")),
                20.0 * shared.threshold().max(1e-5).log10(),
                stats.captured,
                stats.read,
                stats.read_real,
                open_ticks * 2,
                sending_ticks * 2,
                if shared.muted.load(Ordering::Relaxed) { ", muted" } else { "" },
                if shared.has_mic.load(Ordering::Relaxed) { "" } else { ", no microphone" },
            );
            (ticks, open_ticks, sending_ticks) = (0, 0, 0);
        }
        // Unmute as soon as the gate opens (the microphone fades in), but mute
        // only once the fade-out has been sent, so speech never stops mid-wave.
        let send = if transmit {
            closed_at = None;
            true
        } else {
            let since = *closed_at.get_or_insert_with(Instant::now);
            sending && since.elapsed() < MUTE_DELAY
        };
        if send != sending {
            sending = send;
            driver.lock().await.mute(!send);
        }

        let mut speaking: HashSet<Id> = {
            let users = shared.ssrc_users.lock();
            let mut heard = shared.heard.lock();
            heard.retain(|_, at| at.elapsed() < SPEAKING_HOLD);
            heard.keys().filter_map(|ssrc| users.get(ssrc)).map(|u| Id(*u)).collect()
        };
        if transmit {
            speaking.insert(self_id);
        }
        if speaking != last_reported {
            last_reported = speaking.clone();
            if shared.events.send(VoiceEvent::Speaking(speaking)).await.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_waits_for_prebuffer_then_drains() {
        let mut j = Jitter::new(4, 100);
        let mut out = [9.0f32; 3];
        j.push(&[1.0, 2.0]);
        j.pull(&mut out);
        assert_eq!(out, [0.0; 3], "not primed yet");
        j.push(&[3.0, 4.0, 5.0]);
        j.pull(&mut out);
        assert_eq!(out, [1.0, 2.0, 3.0]);
        j.pull(&mut out);
        assert_eq!(out, [4.0, 5.0, 0.0], "underrun pads with silence");
        assert_eq!(j.len(), 0);
    }

    #[test]
    fn jitter_fades_around_underruns() {
        let mut j = Jitter::new(4, 100).with_fade(2);
        j.push(&[1.0; 6]);
        let mut out = [0.0f32; 4];
        j.pull(&mut out);
        assert!(out[0] < out[1] && out[1] < 1.0 && out[2] == 1.0, "fades in: {out:?}");
        j.pull(&mut out);
        assert!(out[0] > out[1] && out[1] > 0.0 && out[2] == 0.0, "fades out at underrun: {out:?}");
    }

    #[test]
    fn jitter_steers_only_during_silence() {
        let mut j = Jitter::new(4, 100);
        j.push(&[1.0; 4]);
        let mut out = [0.0f32; 1];
        j.pull(&mut out); // primed
        j.push_steered(&[0.5; 8], false);
        assert_eq!(j.len(), 11, "speech is never dropped");
        j.push_steered(&[0.0; 8], true);
        assert_eq!(j.len(), 11, "silence is skipped while running behind");
    }

    #[test]
    fn mic_buffer_keeps_a_reserve() {
        let mut m = MicBuffer::new(1000); // 1 sample per ms: target 40, reserve 20
        let mut out = Vec::new();
        m.push(&[0.1; 30], false);
        m.take(100, &mut out);
        assert!(out.is_empty(), "waits for the target level first");
        m.push(&[0.1; 30], false);
        m.take(100, &mut out);
        assert_eq!((out.len(), m.len()), (40, 20), "keeps the reserve");
        out.clear();
        m.take(100, &mut out);
        assert_eq!(out.len(), 20, "digs into the reserve rather than send nothing");
        assert!(m.low());
    }

    #[test]
    fn mic_buffer_drops_silence_when_behind() {
        let mut m = MicBuffer::new(1000);
        m.push(&[0.1; 130], false);
        m.push(&[0.0; 10], true);
        assert_eq!(m.len(), 130, "quiet audio skipped while far behind");
        m.push(&[0.1; 10], false);
        assert_eq!(m.len(), 140, "speech kept");
    }

    #[test]
    fn jitter_sheds_backlog() {
        let mut j = Jitter::new(2, 6);
        j.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        assert_eq!(j.len(), 2);
        let mut out = [0.0f32; 2];
        j.pull(&mut out);
        assert_eq!(out, [6.0, 7.0], "keeps the newest audio");
    }

    #[test]
    fn resampler_preserves_duration() {
        let mut r = StereoResampler::new(48_000, 44_100);
        let frame = vec![0.5f32; 960 * 2];
        let mut out = Vec::new();
        for _ in 0..50 {
            r.process(&frame, &mut out);
        }
        // One second in should be (almost exactly) one second out.
        let frames = out.len() / 2;
        assert!((frames as i64 - 44_100).abs() <= 2, "{frames}");
        assert!(out.iter().skip(2).all(|s| (*s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn resampler_is_identity_at_equal_rates() {
        let mut r = StereoResampler::new(48_000, 48_000);
        let input: Vec<f32> = (0..20).map(|i| i as f32).collect();
        let mut out = Vec::new();
        r.process(&input, &mut out);
        assert_eq!(out, input);
    }

    fn test_shared(loud: bool) -> Arc<Shared> {
        let (tx, _rx) = async_channel::unbounded();
        Arc::new(Shared {
            mic: Mutex::new(MicBuffer::new(1000)),
            mic_ready: Condvar::new(),
            out: Mutex::new(Jitter::new(0, 0)),
            resampler: Mutex::new(StereoResampler::new(1, 1)),
            muted: AtomicBool::new(false),
            deafened: AtomicBool::new(false),
            threshold: AtomicU32::new(0f32.to_bits()),
            input_gain: AtomicU32::new(1f32.to_bits()),
            output_gain: AtomicU32::new(1f32.to_bits()),
            last_loud: Mutex::new(loud.then(Instant::now)),
            stats: Mutex::new(Stats::default()),
            has_mic: AtomicBool::new(true),
            ssrc_users: Mutex::new(HashMap::new()),
            heard: Mutex::new(HashMap::new()),
            events: tx,
        })
    }

    fn read_samples(source: &mut MicSource, bytes: usize) -> Vec<f32> {
        let mut buf = vec![0u8; bytes];
        let n = source.read(&mut buf).unwrap();
        assert!(n > 0, "reads never signal the end of the stream");
        buf[..n - n % 4].chunks(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
    }

    #[test]
    fn mic_source_never_pads_real_audio_with_silence() {
        let shared = test_shared(true);
        shared.mic.lock().push(&[0.5; 60], false);
        let mut source = MicSource::new(shared.clone(), 1000);
        // A huge read, like symphonia's 32 KB blocks, gets only real audio.
        let got = read_samples(&mut source, 32 * 1024);
        assert_eq!(got.len(), 40, "only what is there, minus the reserve");
        assert!(got.iter().skip(10).all(|s| *s == 0.5), "fully open after the fade-in");
        assert!(got[0] < got[5] && got[5] < 0.5, "the gate fades in");
    }

    #[test]
    fn mic_source_fades_out_when_the_gate_closes() {
        let shared = test_shared(true);
        shared.mic.lock().push(&[0.5; 100], false);
        let mut source = MicSource::new(shared.clone(), 1000);
        read_samples(&mut source, 20 * 4);
        *shared.last_loud.lock() = None;
        let got = read_samples(&mut source, 20 * 4);
        assert!(got[0] > 0.0 && got[0] < 0.5 && got[12] == 0.0, "fades out: {got:?}");
    }

    #[test]
    fn mic_source_returns_silence_without_audio() {
        let mut source = MicSource::new(test_shared(false), 1000);
        let got = read_samples(&mut source, 4096);
        assert_eq!(got, vec![0.0; 10], "10 ms of silence");
    }

    #[test]
    fn rms_in_decibels() {
        assert!((rms_db(&[1.0, -1.0]) - 0.0).abs() < 1e-4);
        assert!((rms_db(&[0.1; 8]) + 20.0).abs() < 1e-3);
        assert_eq!(rms_db(&[0.0; 8]), -100.0);
        assert_eq!(rms_db(&[]), -100.0);
    }

    #[test]
    fn decibels_convert() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_linear(-20.0) - 0.1).abs() < 1e-6);
    }
}
