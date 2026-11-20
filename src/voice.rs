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
use parking_lot::Mutex;
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
pub struct Jitter {
    buf: VecDeque<f32>,
    primed: bool,
    prebuffer: usize,
    max: usize,
}

impl Jitter {
    pub fn new(prebuffer: usize, max: usize) -> Self {
        Self { buf: VecDeque::with_capacity(max), primed: false, prebuffer, max }
    }

    pub fn push(&mut self, samples: &[f32]) {
        self.buf.extend(samples.iter().copied());
        if self.buf.len() > self.max {
            // Fell behind: keep only the newest prebuffer's worth.
            let excess = self.buf.len() - self.prebuffer;
            self.buf.drain(..excess);
        }
    }

    /// Fills `out` completely, with silence where there is no audio.
    pub fn pull(&mut self, out: &mut [f32]) {
        if !self.primed {
            if self.buf.len() < self.prebuffer {
                out.fill(0.0);
                return;
            }
            self.primed = true;
        }
        let n = out.len().min(self.buf.len());
        for (slot, sample) in out.iter_mut().zip(self.buf.drain(..n)) {
            *slot = sample;
        }
        if n < out.len() {
            out[n..].fill(0.0);
            self.primed = false;
        }
    }

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

struct Shared {
    /// Mono microphone samples at the input device's rate.
    mic: Mutex<Jitter>,
    /// Interleaved stereo at the output device's rate.
    out: Mutex<Jitter>,
    resampler: Mutex<StereoResampler>,
    muted: AtomicBool,
    deafened: AtomicBool,
    /// Linear RMS threshold, stored as f32 bits.
    threshold: AtomicU32,
    /// When the microphone last heard something above the threshold.
    last_loud: Mutex<Option<Instant>>,
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

/// Serves microphone audio to songbird as an endless raw f32 stream. Reads
/// never block: songbird's mixer thread calls this every tick.
struct MicSource {
    shared: Arc<Shared>,
    pending: VecDeque<u8>,
    scratch: Vec<f32>,
}

impl Read for MicSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pending.len() < buf.len() {
            let samples = (buf.len() - self.pending.len()).div_ceil(4);
            self.scratch.resize(samples, 0.0);
            self.shared.mic.lock().pull(&mut self.scratch);
            for s in &self.scratch {
                self.pending.extend(s.to_le_bytes());
            }
        }
        let len = buf.len();
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
                for (ssrc, data) in &tick.speaking {
                    let Some(pcm) = &data.decoded_voice else { continue };
                    let energy = pcm.iter().map(|s| (*s as f32).powi(2)).sum::<f32>() / pcm.len().max(1) as f32;
                    if energy.sqrt() > SPEAKING_RMS {
                        shared.heard.lock().insert(*ssrc, now);
                    }
                    if !deaf {
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
                shared.out.lock().push(&resampled);
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
                mono.clear();
                mono.extend(data.chunks(channels.max(1)).map(|frame| {
                    frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / frame.len() as f32
                }));
                let rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len().max(1) as f32).sqrt();
                if rms >= shared.threshold() {
                    *shared.last_loud.lock() = Some(Instant::now());
                }
                shared.mic.lock().push(&mono);
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
            // Microphone: wait for 40 ms of audio, keep at most 300 ms (at up to 48 kHz mono).
            mic: Mutex::new(Jitter::new(VOICE_RATE as usize / 25, VOICE_RATE as usize * 3 / 10)),
            out: Mutex::new(Jitter::new(0, 0)),
            resampler: Mutex::new(StereoResampler::new(VOICE_RATE, VOICE_RATE)),
            muted: AtomicBool::new(muted),
            deafened: AtomicBool::new(deafened),
            threshold: AtomicU32::new(db_to_linear(options.sensitivity_db).to_bits()),
            last_loud: Mutex::new(None),
            has_mic: AtomicBool::new(false),
            ssrc_users: Mutex::new(HashMap::new()),
            heard: Mutex::new(HashMap::new()),
            events: events.clone(),
        });

        let output = match open_output(shared.clone(), options.output_device.as_deref()) {
            Ok((stream, rate)) => {
                // 60 ms of prebuffer, at most 400 ms of backlog, stereo.
                let per_ms = rate as usize * 2 / 1000;
                *shared.out.lock() = Jitter::new(per_ms * 60, per_ms * 400);
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
                shared.has_mic.store(true, Ordering::Relaxed);
                let source = MicSource { shared: shared.clone(), pending: VecDeque::new(), scratch: Vec::new() };
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
    let mut last_reported: HashSet<Id> = HashSet::new();
    loop {
        interval.tick().await;
        let transmit = shared.transmitting();
        if transmit != sending {
            sending = transmit;
            driver.lock().await.mute(!transmit);
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

    #[test]
    fn mic_source_always_fills_reads() {
        let (tx, _rx) = async_channel::unbounded();
        let shared = Arc::new(Shared {
            mic: Mutex::new(Jitter::new(0, 1000)),
            out: Mutex::new(Jitter::new(0, 0)),
            resampler: Mutex::new(StereoResampler::new(1, 1)),
            muted: AtomicBool::new(false),
            deafened: AtomicBool::new(false),
            threshold: AtomicU32::new(0f32.to_bits()),
            last_loud: Mutex::new(None),
            has_mic: AtomicBool::new(true),
            ssrc_users: Mutex::new(HashMap::new()),
            heard: Mutex::new(HashMap::new()),
            events: tx,
        });
        shared.mic.lock().push(&[0.25, -0.5]);
        let mut source = MicSource { shared, pending: VecDeque::new(), scratch: Vec::new() };
        let mut buf = [0u8; 6];
        assert_eq!(source.read(&mut buf).unwrap(), 6);
        assert_eq!(f32::from_le_bytes(buf[..4].try_into().unwrap()), 0.25);
        let mut rest = [0u8; 6];
        assert_eq!(source.read(&mut rest).unwrap(), 6);
        // The odd bytes carried over complete the second sample.
        let second = [buf[4], buf[5], rest[0], rest[1]];
        assert_eq!(f32::from_le_bytes(second), -0.5);
    }

    #[test]
    fn decibels_convert() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_linear(-20.0) - 0.1).abs() < 1e-6);
    }
}
