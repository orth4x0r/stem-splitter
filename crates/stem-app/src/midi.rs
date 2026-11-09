//! Audio to MIDI for a stem (or the selected part of one): the transcription job, the
//! audition synth, and `.mid` export. Drawing lives in `views/midi.rs`.
//!
//! The model (Basic Pitch) runs once per sheet; its posteriorgrams are kept, so moving a
//! threshold slider only re-runs note creation, which takes milliseconds.

use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;
use stem_audio::AudioBuffer;
use stem_audio::analysis::{BeatGrid, Key};
use stem_models::transcribe::{
    self, BASIC_PITCH, Beats, MidiOptions, Note, NoteOptions, Posteriorgrams, Quantize,
    QuantizeGrid,
};
use stem_models::{Device, ModelManager, StemKind};

use crate::backend::guard;

// ---- settings ------------------------------------------------------------------------------

/// Audition voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wave {
    Sine,
    Saw,
}

/// The sheet's knobs, kept between sheets for the session of the app.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiPrefs {
    /// Onset threshold in percent (higher = fewer notes).
    pub onset_pct: f32,
    /// Frame threshold in percent (higher = shorter notes).
    pub frame_pct: f32,
    pub min_note_ms: f32,
    /// Pick up notes without a clear attack (basic-pitch's melodia trick).
    pub soft_notes: bool,
    pub pitch_bends: bool,
    /// `None` = off.
    pub quantize: Option<QuantizeGrid>,
    pub strength_pct: f32,
    pub wave: Wave,
    /// Play the original stem under the synth.
    pub with_stem: bool,
    /// Run the model on the GPU (DirectML) instead of the CPU.
    pub gpu: bool,
}

impl Default for MidiPrefs {
    fn default() -> Self {
        let d = NoteOptions::default();
        Self {
            onset_pct: d.onset_threshold * 100.0,
            frame_pct: d.frame_threshold * 100.0,
            min_note_ms: 128.0,
            soft_notes: d.melodia_trick,
            pitch_bends: false,
            quantize: None,
            strength_pct: 100.0,
            wave: Wave::Saw,
            with_stem: false,
            gpu: false,
        }
    }
}

/// Default pitch range for a stem: keeps octave ghosts and bleed out of bass / vocal lines.
pub fn default_range(kind: StemKind) -> (u8, u8) {
    match kind {
        StemKind::Bass => (24, 67),
        StemKind::Vocals | StemKind::LeadVocals | StemKind::BackingVocals => (40, 88),
        StemKind::Dialogue => (36, 84),
        _ => (21, 108),
    }
}

/// General MIDI program written into the file (0-based).
pub fn gm_program(kind: StemKind) -> u8 {
    match kind {
        StemKind::Bass => 33,   // Electric Bass (finger)
        StemKind::Guitar => 27, // Electric Guitar (clean)
        StemKind::Piano => 0,   // Acoustic Grand
        StemKind::Vocals | StemKind::LeadVocals | StemKind::BackingVocals => 53, // Voice Oohs
        _ => 4,                 // Electric Piano 1, as basic-pitch
    }
}

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// "C4" for 60.
pub fn note_name(pitch: u8) -> String {
    format!("{}{}", NAMES[pitch as usize % 12], pitch as i32 / 12 - 1)
}

// ---- the sheet's state ---------------------------------------------------------------------

/// What is being transcribed.
#[derive(Clone)]
pub struct MidiSource {
    pub title: String,
    pub out_dir: PathBuf,
    pub kind: StemKind,
    pub stems: Arc<Vec<AudioBuffer>>,
    pub track: usize,
    /// Seconds into the stem; `end` is exclusive.
    pub start: f64,
    pub end: f64,
    /// True when it is a selection rather than the whole stem.
    pub is_selection: bool,
    pub bpm: Option<f32>,
    pub key: Option<Key>,
    pub grid: Option<BeatGrid>,
}

impl MidiSource {
    pub fn audio(&self) -> Option<AudioBuffer> {
        let a = self.stems.get(self.track)?;
        let sr = a.sample_rate as f64;
        let f0 = ((self.start * sr) as usize).min(a.frames());
        let f1 = ((self.end * sr) as usize).clamp(f0, a.frames());
        Some(if f0 == 0 && f1 == a.frames() {
            a.clone()
        } else {
            a.slice(f0, f1)
        })
    }

    pub fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }

    pub fn beats(&self) -> Beats<'_> {
        Beats {
            bpm: self.bpm.unwrap_or(120.0),
            grid: self.grid.as_ref(),
            offset: self.start,
        }
    }

    /// "Track - Bass - 90bpm Am" (+ " - 0m10s670" for a selection), sanitised.
    pub fn file_stem(&self) -> String {
        if self.is_selection {
            return crate::analysis::sample_file_name(
                &self.title,
                self.kind.label(),
                self.bpm,
                self.key.as_ref(),
                self.start,
                None,
            );
        }
        let title: String = self.title.trim().chars().take(60).collect();
        let mut parts = vec![title.trim().to_owned(), self.kind.label().to_owned()];
        let musical = [
            self.bpm
                .map(|b| format!("{}bpm", crate::analysis::bpm_label(b))),
            self.key.as_ref().map(Key::short),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
        if !musical.is_empty() {
            parts.push(musical);
        }
        crate::paths::sanitize_file_name(&parts.join(" - "))
    }

    pub fn midi_path(&self) -> PathBuf {
        self.out_dir
            .join("MIDI")
            .join(format!("{}.mid", self.file_stem()))
    }
}

pub enum Status {
    Running { msg: String, frac: Option<f32> },
    Ready,
    Failed(String),
}

enum Msg {
    Progress(String, Option<f32>),
    Done(Result<(Posteriorgrams, Device), String>),
}

pub struct MidiSheet {
    pub src: MidiSource,
    pub status: Status,
    pub prefs: MidiPrefs,
    /// Inclusive MIDI pitch range kept.
    pub range: (u8, u8),
    post: Option<Posteriorgrams>,
    /// Raw notes from the current thresholds, sorted by start.
    pub raw: Vec<Note>,
    /// What gets exported and drawn: `raw` after quantisation.
    pub notes: Vec<Note>,
    pub device: Option<Device>,
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    /// Inputs `notes` was computed from.
    computed: Option<(MidiPrefs, (u8, u8))>,
    pub preview: Option<Preview>,
    /// Visible time window of the piano roll (seconds into the source).
    pub view_start: f64,
    pub view_span: f64,
}

impl MidiSheet {
    /// Open the sheet and start transcribing. `blocking` runs the model on this thread (for
    /// screenshots).
    pub fn open(
        src: MidiSource,
        prefs: MidiPrefs,
        manager: ModelManager,
        demo: bool,
        ctx: &egui::Context,
        blocking: bool,
    ) -> Self {
        let (tx, rx) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = Job {
            audio: src.audio(),
            kind: src.kind,
            start: src.start,
            manager,
            demo,
            device: if prefs.gpu { Device::Auto } else { Device::Cpu },
        };
        let range = default_range(src.kind);
        let view_span = if src.duration() <= 40.0 {
            src.duration()
        } else {
            24.0
        };
        let mut me = Self {
            src,
            status: Status::Running {
                msg: "Starting…".into(),
                frac: None,
            },
            prefs,
            range,
            post: None,
            raw: Vec::new(),
            notes: Vec::new(),
            device: None,
            rx,
            cancel: cancel.clone(),
            computed: None,
            preview: None,
            view_start: 0.0,
            view_span,
        };
        let send = {
            let ctx = ctx.clone();
            move |tx: &Sender<Msg>, m| {
                let _ = tx.send(m);
                ctx.request_repaint();
            }
        };
        if blocking {
            let r = job.run(&cancel, &mut |m, f| send(&tx, Msg::Progress(m, f)));
            send(&tx, Msg::Done(r));
            me.poll();
        } else {
            std::thread::Builder::new()
                .name("midi".into())
                .spawn(move || {
                    let r = guard("transcription", || {
                        job.run(&cancel, &mut |m, f| send(&tx, Msg::Progress(m, f)))
                    })
                    .and_then(|r| r);
                    send(&tx, Msg::Done(r));
                })
                .expect("spawn midi thread");
        }
        me
    }

    /// Pull job messages and keep `notes` in step with the knobs. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Progress(msg, frac) => self.status = Status::Running { msg, frac },
                Msg::Done(Ok((post, device))) => {
                    self.post = Some(post);
                    self.device = Some(device);
                    self.status = Status::Ready;
                }
                Msg::Done(Err(e)) => self.status = Status::Failed(e),
            }
        }
        let key = (self.prefs.clone(), self.range);
        if self.post.is_some() && self.computed.as_ref() != Some(&key) {
            self.recompute();
            self.computed = Some(key);
        }
    }

    pub fn note_options(&self) -> NoteOptions {
        NoteOptions {
            onset_threshold: (self.prefs.onset_pct / 100.0).clamp(0.01, 0.99),
            frame_threshold: (self.prefs.frame_pct / 100.0).clamp(0.01, 0.99),
            min_note_ms: self.prefs.min_note_ms,
            min_pitch: Some(self.range.0.min(self.range.1)),
            max_pitch: Some(self.range.1.max(self.range.0)),
            infer_onsets: true,
            melodia_trick: self.prefs.soft_notes,
            pitch_bends: self.prefs.pitch_bends,
        }
    }

    pub fn quantize(&self) -> Option<Quantize> {
        self.prefs.quantize.map(|grid| Quantize {
            grid,
            strength: self.prefs.strength_pct / 100.0,
        })
    }

    fn recompute(&mut self) {
        let Some(post) = &self.post else { return };
        let mut raw = transcribe::notes(post, &self.note_options());
        raw.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
        self.notes = match self.quantize() {
            Some(q) => transcribe::quantize(&raw, &q, &self.src.beats()),
            None => raw.clone(),
        };
        self.raw = raw;
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status, Status::Running { .. })
    }

    /// The `.mid` bytes for the current notes.
    pub fn midi_bytes(&self) -> Vec<u8> {
        transcribe::write_midi(
            &self.notes,
            &MidiOptions {
                bpm: self.src.bpm.unwrap_or(120.0),
                track_name: format!("{} - {}", self.src.title, self.src.kind.label()),
                program: gm_program(self.src.kind),
                pitch_bends: self.prefs.pitch_bends && self.prefs.quantize.is_none(),
            },
        )
    }

    /// Write the `.mid` next to the stems (`<out>/MIDI/`). Returns the path.
    pub fn export(&self) -> Result<PathBuf, String> {
        let path = self.src.midi_path();
        write_file(&path, &self.midi_bytes())?;
        Ok(path)
    }

    // ---- audition

    pub fn is_previewing(&self) -> bool {
        self.preview.as_ref().is_some_and(Preview::is_playing)
    }

    /// Playhead of the audition in sheet time (seconds from the start of the source).
    pub fn preview_pos(&self) -> Option<f64> {
        self.preview
            .as_ref()
            .filter(|p| p.is_playing())
            .map(Preview::position)
    }

    pub fn stop_preview(&mut self) {
        if let Some(p) = self.preview.take() {
            p.stop();
        }
    }

    /// Render the notes with the synth (plus the stem, if asked) and play it from `from`.
    pub fn start_preview(&mut self, player: &mut crate::player::Player, from: f64) -> bool {
        self.stop_preview();
        let sr = self
            .src
            .stems
            .get(self.src.track)
            .map_or(44_100, |a| a.sample_rate);
        let mut buf = render(&self.notes, self.prefs.wave, sr, self.src.duration());
        if self.prefs.with_stem
            && let Some(stem) = self.src.audio()
        {
            mix_under(&mut buf, &stem, 0.6);
        }
        let start = ((from.max(0.0) * sr as f64) as u64).min(buf.frames() as u64);
        let (preview, source) = Preview::new(Arc::new(buf), start);
        let Some(mixer) = player.mixer() else {
            return false;
        };
        player.set_playing(false);
        mixer.add(source);
        self.preview = Some(preview);
        true
    }
}

impl Drop for MidiSheet {
    fn drop(&mut self) {
        self.cancel.store(true, Relaxed);
        self.stop_preview();
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

// ---- the job -------------------------------------------------------------------------------

struct Job {
    audio: Option<AudioBuffer>,
    kind: StemKind,
    start: f64,
    manager: ModelManager,
    demo: bool,
    device: Device,
}

impl Job {
    fn run(
        self,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(String, Option<f32>),
    ) -> Result<(Posteriorgrams, Device), String> {
        let audio = self.audio.ok_or("no audio for this stem")?;
        // Developer override (also used for demo screenshots with the real model).
        let model_override = std::env::var_os("STEM_MIDI_MODEL").map(PathBuf::from);
        let real = !self.demo
            || (model_override.is_some() && std::env::var_os("ORT_DYLIB_PATH").is_some());
        if !real {
            progress("Listening for notes…".into(), Some(0.5));
            return Ok((
                demo_posteriorgrams(self.kind, self.start, &audio),
                Device::Cpu,
            ));
        }
        let path = match model_override {
            Some(p) => p,
            None => {
                progress("Downloading Basic Pitch (230 KB)…".into(), None);
                self.manager
                    .download_transcriber(
                        &BASIC_PITCH,
                        &mut |s| progress("Downloading Basic Pitch…".into(), s.fraction()),
                        cancel,
                    )
                    .map_err(|e| e.to_string())?
            }
        };
        progress("Loading ONNX Runtime…".into(), None);
        self.manager
            .ensure_runtime(&mut |_| progress("Downloading ONNX Runtime…".into(), None))
            .map_err(|e| e.to_string())?;
        let mut model =
            transcribe::BasicPitch::load(&path, self.device).map_err(|e| e.to_string())?;
        let device = model.active_device();
        progress("Listening for notes…".into(), Some(0.0));
        let post = model
            .transcribe(
                &audio,
                &mut |f| progress("Listening for notes…".into(), Some(f)),
                cancel,
            )
            .map_err(|e| e.to_string())?;
        Ok((post, device))
    }
}

/// Fake posteriorgrams for demo mode without a model: the demo composition's own notes,
/// smeared and with a little noise, so note creation and every knob still behave for real.
fn demo_posteriorgrams(kind: StemKind, start: f64, audio: &AudioBuffer) -> Posteriorgrams {
    use transcribe::{N_CONTOUR_BINS, N_NOTES};
    let n = (audio.duration_secs() * transcribe::ANNOTATIONS_FPS as f64) as usize;
    let mut post = Posteriorgrams {
        n_frames: n,
        note: vec![0.0; n * N_NOTES],
        onset: vec![0.0; n * N_NOTES],
        contour: vec![0.0; n * N_CONTOUR_BINS],
    };
    let mut seed = 0x2545_F491_u32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed >> 8) as f32 / (1u32 << 24) as f32
    };
    for v in post.note.iter_mut() {
        *v = rnd() * rnd() * 0.25;
    }
    let fps = transcribe::ANNOTATIONS_FPS as f64;
    for (t0, dur, pitch, vel) in crate::demo::notes(kind) {
        let (t0, dur) = (t0 as f64 - start, dur as f64);
        if t0 + dur < 0.0 || !(21..=108).contains(&pitch) {
            continue;
        }
        let f = (pitch - 21) as usize;
        let s = (t0 * fps).round().max(0.0) as usize;
        let e = (((t0 + dur) * fps).round() as usize).min(n);
        for t in s..e {
            let fade = 1.0 - 0.35 * (t - s) as f32 / (e - s).max(1) as f32;
            let v = (vel * fade * (0.9 + 0.1 * rnd())).min(0.98);
            post.note[t * N_NOTES + f] = post.note[t * N_NOTES + f].max(v);
            post.contour[t * N_CONTOUR_BINS + 3 * f] = v;
        }
        if s < n && t0 >= 0.0 {
            post.onset[s * N_NOTES + f] = 0.6 + 0.35 * vel;
            if s + 1 < n {
                post.onset[(s + 1) * N_NOTES + f] = 0.3;
            }
        }
    }
    post
}

// ---- synth ---------------------------------------------------------------------------------

/// Render `notes` as a simple synth voice (stereo, `duration` seconds at `sr`).
pub fn render(notes: &[Note], wave: Wave, sr: u32, duration: f64) -> AudioBuffer {
    let srf = sr as f32;
    let release = 0.08f32;
    let frames = ((duration.max(0.0) + release as f64) * sr as f64) as usize;
    let mut out = vec![0.0f32; frames];
    for n in notes {
        let freq = 440.0 * 2f32.powf((n.pitch as f32 - 69.0) / 12.0);
        let gain = 0.22 * (n.velocity() as f32 / 127.0).powf(1.3);
        let s0 = (n.start.max(0.0) * sr as f64) as usize;
        let held = ((n.end - n.start).max(0.0) * sr as f64) as usize;
        let len = held + (release * srf) as usize;
        let dt = freq / srf;
        let mut phase = 0.0f32;
        let mut lp = 0.0f32;
        // Brighter for louder notes; one-pole low-pass tames the raw saw.
        let cutoff = (freq * (3.0 + 6.0 * n.amplitude)).min(9000.0);
        let a = 1.0 - (-std::f32::consts::TAU * cutoff / srf).exp();
        for k in 0..len {
            let i = s0 + k;
            if i >= frames {
                break;
            }
            let t = k as f32 / srf;
            let env = if k < held {
                (t / 0.006).min(1.0) * (0.75 + 0.25 * (-t * 6.0).exp())
            } else {
                let held_t = held as f32 / srf;
                let at_release = (held_t / 0.006).min(1.0) * (0.75 + 0.25 * (-held_t * 6.0).exp());
                at_release * (1.0 - (k - held) as f32 / (release * srf)).max(0.0)
            };
            let x = match wave {
                Wave::Sine => {
                    (std::f32::consts::TAU * phase).sin()
                        + 0.15 * (2.0 * std::f32::consts::TAU * phase).sin()
                }
                Wave::Saw => {
                    // PolyBLEP saw.
                    let mut v = 2.0 * phase - 1.0;
                    if phase < dt {
                        let p = phase / dt;
                        v -= p + p - p * p - 1.0;
                    } else if phase > 1.0 - dt {
                        let p = (phase - 1.0) / dt;
                        v -= p * p + p + p + 1.0;
                    }
                    lp += a * (v - lp);
                    lp * 0.8
                }
            };
            out[i] += x * env * gain;
            phase += dt;
            if phase >= 1.0 {
                phase -= 1.0;
            }
        }
    }
    let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak > 0.95 {
        let g = 0.95 / peak;
        out.iter_mut().for_each(|v| *v *= g);
    }
    AudioBuffer::new(sr, vec![out.clone(), out])
}

/// Add `stem` (same rate) under `buf` at `gain`, then keep the peak below full scale.
fn mix_under(buf: &mut AudioBuffer, stem: &AudioBuffer, gain: f32) {
    let n = buf.frames().min(stem.frames());
    let sc = stem.n_channels();
    if sc == 0 {
        return;
    }
    for (c, ch) in buf.channels.iter_mut().enumerate() {
        let src = &stem.channels[c.min(sc - 1)];
        for i in 0..n {
            ch[i] = ch[i] * 0.8 + src[i] * gain;
        }
    }
    let peak = buf.peak();
    if peak > 0.98 {
        let g = 0.98 / peak;
        for ch in &mut buf.channels {
            ch.iter_mut().for_each(|v| *v *= g);
        }
    }
}

// ---- audition playback ---------------------------------------------------------------------

struct PreviewShared {
    pos: AtomicU64,
    alive: AtomicBool,
}

/// Handle to a playing audition.
pub struct Preview {
    shared: Arc<PreviewShared>,
    rate: u32,
}

impl Preview {
    fn new(buf: Arc<AudioBuffer>, start: u64) -> (Self, PreviewSource) {
        let shared = Arc::new(PreviewShared {
            pos: AtomicU64::new(start),
            alive: AtomicBool::new(true),
        });
        let rate = buf.sample_rate.max(1);
        (
            Self {
                shared: shared.clone(),
                rate,
            },
            PreviewSource {
                buf,
                shared,
                frame: start as usize,
                right: None,
                rate: NonZero::new(rate).unwrap_or(NonZero::<u32>::MIN),
            },
        )
    }

    pub fn is_playing(&self) -> bool {
        self.shared.alive.load(Relaxed)
    }

    pub fn position(&self) -> f64 {
        self.shared.pos.load(Relaxed) as f64 / self.rate as f64
    }

    pub fn stop(&self) {
        self.shared.alive.store(false, Relaxed);
    }
}

/// One-shot stereo buffer source for rodio; ends when stopped or done.
pub struct PreviewSource {
    buf: Arc<AudioBuffer>,
    shared: Arc<PreviewShared>,
    frame: usize,
    right: Option<f32>,
    rate: NonZero<u32>,
}

impl Iterator for PreviewSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if let Some(r) = self.right.take() {
            return Some(r);
        }
        if !self.shared.alive.load(Relaxed) || self.frame >= self.buf.frames() {
            self.shared.alive.store(false, Relaxed);
            return None;
        }
        let ch = &self.buf.channels;
        let l = ch[0][self.frame];
        self.right = Some(ch[1.min(ch.len() - 1)][self.frame]);
        self.frame += 1;
        self.shared.pos.store(self.frame as u64, Relaxed);
        Some(l)
    }
}

impl rodio::Source for PreviewSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        NonZero::new(2).unwrap_or(NonZero::<u16>::MIN)
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: f64, end: f64, pitch: u8) -> Note {
        Note {
            start,
            end,
            pitch,
            amplitude: 0.7,
            bends: None,
        }
    }

    #[test]
    fn names() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(21), "A0");
        assert_eq!(note_name(33), "A1");
        assert_eq!(note_name(108), "C8");
    }

    #[test]
    fn synth_renders_notes_where_they_are() {
        let sr = 8000;
        let buf = render(&[note(0.5, 1.0, 69)], Wave::Saw, sr, 2.0);
        assert_eq!(buf.n_channels(), 2);
        let rms = |a: f64, b: f64| {
            let s = &buf.channels[0][(a * sr as f64) as usize..(b * sr as f64) as usize];
            (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt()
        };
        assert!(rms(0.0, 0.45) < 1e-6);
        assert!(rms(0.6, 0.9) > 0.02);
        assert!(rms(1.2, 2.0) < 1e-6);
        assert!(buf.peak() <= 0.95 + 1e-6);
        let sine = render(&[note(0.0, 0.5, 69)], Wave::Sine, sr, 0.5);
        assert!(sine.peak() > 0.05);
    }

    #[test]
    fn file_names() {
        let src = MidiSource {
            title: "Midnight: Drive".into(),
            out_dir: PathBuf::from("/out"),
            kind: StemKind::Bass,
            stems: Arc::new(Vec::new()),
            track: 0,
            start: 0.0,
            end: 30.0,
            is_selection: false,
            bpm: Some(90.0),
            key: Some(Key {
                tonic: 9,
                mode: stem_audio::analysis::Mode::Minor,
                confidence: 0.8,
            }),
            grid: None,
        };
        assert_eq!(src.file_stem(), "Midnight_ Drive - Bass - 90bpm Am");
        assert_eq!(
            src.midi_path(),
            PathBuf::from("/out/MIDI/Midnight_ Drive - Bass - 90bpm Am.mid")
        );
        let sel = MidiSource {
            start: 10.6705,
            end: 12.0,
            is_selection: true,
            bpm: None,
            key: None,
            ..src
        };
        assert!(
            sel.file_stem()
                .starts_with("Midnight_ Drive - Bass - 0m10s670")
        );
    }

    #[test]
    fn demo_notes_survive_note_creation() {
        let audio = crate::demo::stem(StemKind::Bass);
        let post = demo_posteriorgrams(StemKind::Bass, 0.0, &audio);
        let notes = transcribe::notes(&post, &NoteOptions::default());
        let want = crate::demo::notes(StemKind::Bass).len();
        assert!(
            notes.len() >= want * 8 / 10 && notes.len() <= want * 13 / 10,
            "{} vs {want}",
            notes.len()
        );
    }
}
