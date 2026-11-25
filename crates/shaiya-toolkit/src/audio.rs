//! Playing a decoded sound in the preview of the Archive tab.
//!
//! One [`Player`] holds one track. The sound is decoded on a worker (see `preview.rs`); the output stream of the system's default device
//! reads it through a callback that resamples linearly to the device's rate and maps mono or stereo to the device's channels. Nothing here
//! plays by itself: a track is loaded, and only [`Player::play`] starts a stream. Having no output device, or a Toolkit built without the
//! `audio` feature, are ordinary states ([`Unavailable`]) that the preview shows in words.
//!
//! The output is behind the [`Backend`] trait so that the tests use a fake one and never open a device.

use shaiya_clientdata::wav::Pcm;
use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Why there is no sound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// The system has no default output device.
    NoDevice,
    /// The Toolkit was built without the `audio` feature.
    BuiltWithout,
    /// The device refused (the reason in the system's words).
    Failed(String),
}

impl Unavailable {
    /// The line the preview shows.
    pub fn message(&self) -> String {
        match self {
            Unavailable::NoDevice => "No audio output device".to_string(),
            Unavailable::BuiltWithout => "Built without audio: this Toolkit does not play sounds".to_string(),
            Unavailable::Failed(e) => format!("The audio output failed: {e}"),
        }
    }
}

/// What the output device wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub rate: u32,
    pub channels: u16,
}

/// Fills one buffer of interleaved `f32` samples for the device.
pub type Fill = Box<dyn FnMut(&mut [f32]) + Send + 'static>;

/// An output: the default device of the system (or a fake in the tests).
pub trait Backend {
    /// The device's rate and channel count, or why there is none. Opens no stream.
    fn output(&mut self) -> Result<Format, Unavailable>;

    /// Starts a stream that calls `fill` for each buffer; the stream stops when the returned guard is dropped. `failed` is set when the
    /// device reports an error while it runs.
    fn start(&mut self, fill: Fill, failed: Arc<AtomicBool>) -> Result<Box<dyn Any>, Unavailable>;
}

/// The backend of a build without the `audio` feature.
pub struct NoAudio;

impl Backend for NoAudio {
    fn output(&mut self) -> Result<Format, Unavailable> {
        Err(Unavailable::BuiltWithout)
    }

    fn start(&mut self, _fill: Fill, _failed: Arc<AtomicBool>) -> Result<Box<dyn Any>, Unavailable> {
        Err(Unavailable::BuiltWithout)
    }
}

/// The system's default output through cpal (WASAPI, CoreAudio or ALSA).
#[cfg(feature = "audio")]
pub struct Cpal;

#[cfg(feature = "audio")]
impl Backend for Cpal {
    fn output(&mut self) -> Result<Format, Unavailable> {
        use cpal::traits::{DeviceTrait, HostTrait};
        let device = cpal::default_host().default_output_device().ok_or(Unavailable::NoDevice)?;
        let cfg = device.default_output_config().map_err(|e| Unavailable::Failed(e.to_string()))?;
        Ok(Format { rate: cfg.sample_rate(), channels: cfg.channels() })
    }

    fn start(&mut self, mut fill: Fill, failed: Arc<AtomicBool>) -> Result<Box<dyn Any>, Unavailable> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let device = cpal::default_host().default_output_device().ok_or(Unavailable::NoDevice)?;
        let supported = device.default_output_config().map_err(|e| Unavailable::Failed(e.to_string()))?;
        let format = supported.sample_format();
        let config = supported.config();
        let on_error = move |_e: cpal::Error| failed.store(true, Ordering::Relaxed);
        // room for a large buffer up front: the callback resizes within it and does not allocate on the audio thread
        let mut scratch: Vec<f32> = Vec::with_capacity(1 << 16);
        let stream = match format {
            cpal::SampleFormat::F32 => device.build_output_stream(config, move |data: &mut [f32], _: &cpal::OutputCallbackInfo| fill(data), on_error, None),
            cpal::SampleFormat::I16 => device.build_output_stream(
                config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    scratch.resize(data.len(), 0.0);
                    fill(&mut scratch);
                    for (o, s) in data.iter_mut().zip(&scratch) {
                        *o = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                    }
                },
                on_error,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_output_stream(
                config,
                move |data: &mut [u16], _: &cpal::OutputCallbackInfo| {
                    scratch.resize(data.len(), 0.0);
                    fill(&mut scratch);
                    for (o, s) in data.iter_mut().zip(&scratch) {
                        *o = ((s.clamp(-1.0, 1.0) * 0.5 + 0.5) * 65535.0) as u16;
                    }
                },
                on_error,
                None,
            ),
            other => return Err(Unavailable::Failed(format!("the device wants {other} samples, which are not supported"))),
        }
        .map_err(|e| Unavailable::Failed(e.to_string()))?;
        stream.play().map_err(|e| Unavailable::Failed(e.to_string()))?;
        Ok(Box::new(stream))
    }
}

/// The backend of this build.
pub fn default_backend() -> Box<dyn Backend> {
    #[cfg(feature = "audio")]
    {
        Box::new(Cpal)
    }
    #[cfg(not(feature = "audio"))]
    {
        Box::new(NoAudio)
    }
}

const NO_SEEK: u64 = u64::MAX;

/// What the output callback and the player share.
struct Shared {
    playing: AtomicBool,
    finished: AtomicBool,
    failed: Arc<AtomicBool>,
    /// The volume (0 to 1) as `f32` bits.
    volume: AtomicU32,
    /// The play position in source frames as `f64` bits (written by the callback).
    pos: AtomicU64,
    /// A position the callback must jump to (`f64` bits), or [`NO_SEEK`].
    seek: AtomicU64,
}

/// The playing state of the track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Playing,
    Paused,
}

/// One track and its stream.
pub struct Player {
    backend: Box<dyn Backend>,
    track: Option<(String, Arc<Pcm>)>,
    shared: Arc<Shared>,
    stream: Option<Box<dyn Any>>,
    output: Option<Result<Format, Unavailable>>,
    error: Option<String>,
}

impl Default for Player {
    fn default() -> Self {
        Player::new(default_backend())
    }
}

/// The callback of one stream: reads `pcm` from `start` and writes buffers for `out`.
fn make_fill(pcm: Arc<Pcm>, out: Format, shared: Arc<Shared>) -> Fill {
    let mut pos = f64::from_bits(shared.pos.load(Ordering::Relaxed));
    let step = pcm.rate as f64 / out.rate.max(1) as f64;
    let (ch_in, ch_out) = (pcm.channels.max(1) as usize, out.channels.max(1) as usize);
    let frames = pcm.frames() as f64;
    Box::new(move |buf: &mut [f32]| {
        let s = shared.seek.swap(NO_SEEK, Ordering::Relaxed);
        if s != NO_SEEK {
            pos = f64::from_bits(s);
        }
        if !shared.playing.load(Ordering::Relaxed) {
            buf.fill(0.0);
            return;
        }
        let vol = f32::from_bits(shared.volume.load(Ordering::Relaxed)).clamp(0.0, 1.0);
        let mut frame = [0f32; 2];
        for o in buf.chunks_mut(ch_out) {
            // a buffer that is not a whole number of frames (a device never gives one): its tail is silence, never an out-of-range index
            if o.len() < ch_out {
                o.fill(0.0);
                continue;
            }
            if pos >= frames {
                o.fill(0.0);
                shared.finished.store(true, Ordering::Relaxed);
                continue;
            }
            pcm.lerp_frame(pos, &mut frame[..ch_in.min(2)]);
            let (l, r) = if ch_in == 1 { (frame[0], frame[0]) } else { (frame[0], frame[1]) };
            if ch_out == 1 {
                o[0] = (l + r) * 0.5 * vol;
            } else {
                o[0] = l * vol;
                o[1] = r * vol;
                o[2..].fill(0.0);
            }
            pos += step;
        }
        shared.pos.store(pos.min(frames).to_bits(), Ordering::Relaxed);
    })
}

impl Player {
    pub fn new(backend: Box<dyn Backend>) -> Player {
        Player {
            backend,
            track: None,
            shared: Arc::new(Shared {
                playing: AtomicBool::new(false),
                finished: AtomicBool::new(false),
                failed: Arc::new(AtomicBool::new(false)),
                volume: AtomicU32::new(0.8f32.to_bits()),
                pos: AtomicU64::new(0f64.to_bits()),
                seek: AtomicU64::new(NO_SEEK),
            }),
            stream: None,
            output: None,
            error: None,
        }
    }

    /// Replaces the backend (the tests give a fake one).
    pub fn set_backend(&mut self, backend: Box<dyn Backend>) {
        self.stop();
        self.backend = backend;
        self.output = None;
    }

    /// Takes a decoded sound as the track, stopped at its start. Nothing plays.
    pub fn load(&mut self, key: &str, pcm: Arc<Pcm>) {
        self.stop();
        self.track = Some((key.to_string(), pcm));
        // the device is asked once and the answer kept for the next track (asking opens the system's device, which can take a moment on the
        // UI thread); a failed answer is asked again
        if self.output.as_ref().is_some_and(Result::is_err) {
            self.output = None;
        }
        self.error = None;
    }

    /// Stops the stream and forgets the track (another row was selected, or the archive was closed).
    pub fn unload(&mut self) {
        self.stop();
        self.track = None;
        self.error = None;
    }

    /// The key the track was loaded with.
    pub fn track_key(&self) -> Option<&str> {
        self.track.as_ref().map(|(k, _)| k.as_str())
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.shared.volume.load(Ordering::Relaxed))
    }

    pub fn set_volume(&mut self, v: f32) {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.8 };
        self.shared.volume.store(v.to_bits(), Ordering::Relaxed);
    }

    pub fn state(&self) -> PlayState {
        match (&self.stream, self.shared.playing.load(Ordering::Relaxed)) {
            (None, _) => PlayState::Stopped,
            (Some(_), true) => PlayState::Playing,
            (Some(_), false) => PlayState::Paused,
        }
    }

    /// Why no sound can be played, asked of the backend once per track (opens no stream); `None` when a device is there.
    pub fn unavailable(&mut self) -> Option<Unavailable> {
        if self.output.is_none() {
            self.output = Some(self.backend.output());
        }
        self.output.as_ref().and_then(|o| o.as_ref().err().cloned())
    }

    /// The last error of a running stream, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn duration_s(&self) -> f64 {
        self.track.as_ref().map(|(_, p)| p.duration_s()).unwrap_or(0.0)
    }

    pub fn position_s(&self) -> f64 {
        let Some((_, p)) = &self.track else { return 0.0 };
        f64::from_bits(self.shared.pos.load(Ordering::Relaxed)) / p.rate.max(1) as f64
    }

    /// Starts (or resumes) the track. Without a track or without a device nothing happens and `unavailable` says why.
    pub fn play(&mut self) {
        let Some((_, pcm)) = self.track.clone() else { return };
        self.error = None;
        if self.stream.is_some() {
            self.shared.playing.store(true, Ordering::Relaxed);
            return;
        }
        // the format is asked fresh when a stream starts (the default device may have changed since the kept answer)
        self.output = Some(self.backend.output());
        let fmt = match &self.output {
            Some(Ok(f)) => *f,
            _ => return,
        };
        if f64::from_bits(self.shared.pos.load(Ordering::Relaxed)) >= pcm.frames() as f64 {
            self.shared.pos.store(0f64.to_bits(), Ordering::Relaxed);
        }
        self.shared.finished.store(false, Ordering::Relaxed);
        self.shared.failed.store(false, Ordering::Relaxed);
        self.shared.seek.store(NO_SEEK, Ordering::Relaxed);
        self.shared.playing.store(true, Ordering::Relaxed);
        match self.backend.start(make_fill(pcm, fmt, self.shared.clone()), self.shared.failed.clone()) {
            Ok(guard) => self.stream = Some(guard),
            Err(e) => {
                self.shared.playing.store(false, Ordering::Relaxed);
                self.output = Some(Err(e));
            }
        }
    }

    /// Pauses the sound where it is (the stream stays open, playing silence).
    pub fn pause(&mut self) {
        self.shared.playing.store(false, Ordering::Relaxed);
    }

    /// Stops the stream and goes back to the start.
    pub fn stop(&mut self) {
        self.shared.playing.store(false, Ordering::Relaxed);
        self.stream = None;
        self.shared.pos.store(0f64.to_bits(), Ordering::Relaxed);
        self.shared.seek.store(NO_SEEK, Ordering::Relaxed);
        self.shared.finished.store(false, Ordering::Relaxed);
    }

    /// Jumps to `secs` (clamped to the track).
    pub fn seek(&mut self, secs: f64) {
        let Some((_, p)) = &self.track else { return };
        let frame = (secs.max(0.0) * p.rate as f64).min(p.frames() as f64);
        self.shared.pos.store(frame.to_bits(), Ordering::Relaxed);
        if self.stream.is_some() {
            self.shared.seek.store(frame.to_bits(), Ordering::Relaxed);
        }
    }

    /// Called every frame: a track that ended goes back to the start and stops; a device error is kept.
    pub fn poll(&mut self) {
        if self.stream.is_some() && self.shared.failed.load(Ordering::Relaxed) {
            self.stop();
            self.error = Some("The audio device reported an error; playing stopped".to_string());
        } else if self.stream.is_some() && self.shared.finished.load(Ordering::Relaxed) {
            self.stop();
        }
    }
}

/// `m:ss` for a time in seconds.
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A backend that opens nothing: it hands the stream callback to the test and records when the stream is dropped.
    #[derive(Clone, Default)]
    pub struct Fake {
        pub device: Arc<Mutex<Option<Format>>>,
        pub fill: Arc<Mutex<Option<Fill>>>,
        pub alive: Arc<AtomicBool>,
        pub starts: Arc<AtomicU32>,
        pub probes: Arc<AtomicU32>,
    }

    struct Guard(Arc<AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    impl Fake {
        pub fn with_device(rate: u32, channels: u16) -> Fake {
            let f = Fake::default();
            *f.device.lock().unwrap() = Some(Format { rate, channels });
            f
        }
        fn render(&self, frames: usize, channels: usize) -> Vec<f32> {
            let mut buf = vec![9.0f32; frames * channels];
            (self.fill.lock().unwrap().as_mut().expect("a stream is running"))(&mut buf);
            buf
        }
    }

    impl Backend for Fake {
        fn output(&mut self) -> Result<Format, Unavailable> {
            self.probes.fetch_add(1, Ordering::SeqCst);
            self.device.lock().unwrap().ok_or(Unavailable::NoDevice)
        }
        fn start(&mut self, fill: Fill, _failed: Arc<AtomicBool>) -> Result<Box<dyn Any>, Unavailable> {
            self.device.lock().unwrap().ok_or(Unavailable::NoDevice)?;
            *self.fill.lock().unwrap() = Some(fill);
            self.alive.store(true, Ordering::SeqCst);
            self.starts.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Guard(self.alive.clone())))
        }
    }

    fn tone(rate: u32, channels: u16, frames: usize) -> Arc<Pcm> {
        Arc::new(Pcm { rate, channels, samples: (0..frames * channels as usize).map(|i| (i / channels as usize) as f32 / frames as f32).collect() })
    }

    #[test]
    fn no_device_is_a_normal_state() {
        let mut p = Player::new(Box::new(Fake::default()));
        p.load("a.wav", tone(22050, 1, 100));
        assert_eq!(p.unavailable(), Some(Unavailable::NoDevice));
        assert_eq!(Unavailable::NoDevice.message(), "No audio output device");
        p.play();
        assert_eq!(p.state(), PlayState::Stopped, "play without a device does nothing");
        assert!(p.error().is_none());
        // a build without the audio feature is the other normal state
        let mut q = Player::new(Box::new(NoAudio));
        q.load("a.wav", tone(22050, 1, 100));
        assert_eq!(q.unavailable(), Some(Unavailable::BuiltWithout));
        assert!(Unavailable::BuiltWithout.message().starts_with("Built without audio"));
        q.play();
        assert_eq!(q.state(), PlayState::Stopped);
        // the settings of the player still work
        q.set_volume(0.25);
        assert_eq!(q.volume(), 0.25);
        q.set_volume(f32::NAN);
        assert_eq!(q.volume(), 0.8);
        q.set_volume(7.0);
        assert_eq!(q.volume(), 1.0);
    }

    #[test]
    fn a_loaded_track_does_not_play_by_itself() {
        let fake = Fake::with_device(48000, 2);
        let mut p = Player::new(Box::new(fake.clone()));
        p.load("a.wav", tone(22050, 1, 100));
        p.poll();
        assert_eq!((p.state(), fake.starts.load(Ordering::SeqCst), p.unavailable()), (PlayState::Stopped, 0, None));
        assert_eq!(p.track_key(), Some("a.wav"));
    }

    #[test]
    fn the_device_is_asked_once_for_many_tracks_and_again_when_a_stream_starts() {
        let fake = Fake::with_device(48000, 2);
        let mut p = Player::new(Box::new(fake.clone()));
        for key in ["a.wav", "b.wav", "c.wav"] {
            p.load(key, tone(22050, 1, 100));
            assert_eq!(p.unavailable(), None);
        }
        assert_eq!(fake.probes.load(Ordering::SeqCst), 1, "three tracks, one probe");
        p.play();
        assert_eq!(fake.probes.load(Ordering::SeqCst), 2, "a starting stream asks for the device's format again");
        // an answer of no device is asked again for the next track
        let none = Fake::default();
        let mut q = Player::new(Box::new(none.clone()));
        q.load("a.wav", tone(22050, 1, 100));
        assert_eq!(q.unavailable(), Some(Unavailable::NoDevice));
        q.load("b.wav", tone(22050, 1, 100));
        assert_eq!(q.unavailable(), Some(Unavailable::NoDevice));
        assert_eq!(none.probes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_buffer_that_is_not_a_whole_number_of_frames_is_filled_not_indexed_out_of_range() {
        let fake = Fake::with_device(22050, 2);
        let mut p = Player::new(Box::new(fake.clone()));
        p.load("a.wav", tone(22050, 1, 1000));
        p.play();
        let mut buf = vec![9.0f32; 7];
        (fake.fill.lock().unwrap().as_mut().expect("a stream is running"))(&mut buf);
        assert_eq!(buf[6], 0.0, "the tail that holds no whole frame is silence");
    }

    #[test]
    fn selecting_another_row_stops_the_track() {
        let fake = Fake::with_device(48000, 2);
        let mut p = Player::new(Box::new(fake.clone()));
        p.load("a.wav", tone(22050, 1, 1000));
        p.play();
        assert_eq!(p.state(), PlayState::Playing);
        assert!(fake.alive.load(Ordering::SeqCst));
        // what the Archive tab does when a preview of another row starts
        p.unload();
        assert_eq!(p.state(), PlayState::Stopped);
        assert!(!fake.alive.load(Ordering::SeqCst), "the stream is dropped");
        assert_eq!(p.track_key(), None);
        // loading a new track over a playing one stops the old one too
        p.load("b.wav", tone(22050, 1, 1000));
        p.play();
        p.load("c.wav", tone(22050, 1, 1000));
        assert_eq!((p.state(), fake.alive.load(Ordering::SeqCst)), (PlayState::Stopped, false));
    }

    #[test]
    fn the_stream_resamples_maps_channels_and_applies_the_volume() {
        // 1000 Hz mono ramp 0, 0.25, 0.5, 0.75 played on a 2000 Hz stereo device at half volume: two output frames per source frame
        let pcm = Arc::new(Pcm { rate: 1000, channels: 1, samples: vec![0.0, 0.25, 0.5, 0.75] });
        let fake = Fake::with_device(2000, 2);
        let mut p = Player::new(Box::new(fake.clone()));
        p.set_volume(0.5);
        p.load("a.wav", pcm);
        p.play();
        let out = fake.render(4, 2);
        assert_eq!(out, vec![0.0, 0.0, 0.0625, 0.0625, 0.125, 0.125, 0.1875, 0.1875]);
        // stereo onto a mono device averages the channels; a third channel is silent
        let st = Arc::new(Pcm { rate: 1000, channels: 2, samples: vec![0.5, 1.0, 0.5, 1.0] });
        let mono = Fake::with_device(1000, 1);
        let mut q = Player::new(Box::new(mono.clone()));
        q.set_volume(1.0);
        q.load("s.wav", st.clone());
        q.play();
        assert_eq!(mono.render(2, 1), vec![0.75, 0.75]);
        let six = Fake::with_device(1000, 3);
        let mut r = Player::new(Box::new(six.clone()));
        r.set_volume(1.0);
        r.load("s.wav", st);
        r.play();
        assert_eq!(six.render(1, 3), vec![0.5, 1.0, 0.0]);
    }

    #[test]
    fn pause_seek_and_the_end_of_the_track() {
        let fake = Fake::with_device(1000, 1);
        let mut p = Player::new(Box::new(fake.clone()));
        p.set_volume(1.0);
        p.load("a.wav", tone(1000, 1, 1000));
        p.play();
        fake.render(100, 1);
        assert!((p.position_s() - 0.1).abs() < 1e-9);
        p.pause();
        assert_eq!(p.state(), PlayState::Paused);
        assert!(fake.render(50, 1).iter().all(|&s| s == 0.0), "paused plays silence");
        assert!((p.position_s() - 0.1).abs() < 1e-9, "and keeps the position");
        p.seek(0.5);
        assert!((p.position_s() - 0.5).abs() < 1e-9);
        p.play();
        let after = fake.render(1, 1);
        assert!((after[0] - 0.5).abs() < 1e-6, "{after:?}");
        // run past the end: silence, then poll stops the stream and goes back to the start
        fake.render(600, 1);
        p.poll();
        assert_eq!((p.state(), p.position_s()), (PlayState::Stopped, 0.0));
        assert!(!fake.alive.load(Ordering::SeqCst));
        // a second play starts again from the start
        p.play();
        assert_eq!(fake.starts.load(Ordering::SeqCst), 2);
        assert!((fake.render(1, 1)[0] - 0.0).abs() < 1e-9);
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(125.4), "2:05");
    }
}
