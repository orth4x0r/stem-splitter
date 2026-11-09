//! "Split only a section": the range a job cuts its input to, the pre-split step (fetch +
//! decode + beat analysis of the full input, for the picker), the picker's state, a rough
//! time estimate, and `origin.json`, which remembers where a split came from so the results
//! page can re-split a selection of it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use stem_audio::AudioBuffer;
use stem_models::{ModelArch, StemKind};

use crate::analysis::{SnapMode, TrackAnalysis};
use crate::backend::guard;
use crate::pipeline::{AppEvent, InputSource, JobEvent, JobRequest, Notifier, Stage};
use crate::timeline::Timeline;
use crate::waveform::{BUCKETS, Envelope};

/// Context the model hears on each side of a section, trimmed off afterwards, so the cut
/// edges sound like the rest of the stem.
pub const PAD_SECS: f64 = 1.0;
/// Shortest section the picker allows.
pub const MIN_SECS: f64 = 1.0;

/// Part of a track in seconds, `start < end`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SplitRange {
    pub start: f64,
    pub end: f64,
}

impl SplitRange {
    pub fn new(a: f64, b: f64) -> Self {
        Self {
            start: a.min(b).max(0.0),
            end: a.max(b).max(0.0),
        }
    }

    pub fn len(&self) -> f64 {
        self.end - self.start
    }

    /// "1m02-1m34", for file and folder names.
    pub fn label(&self) -> String {
        format!("{}-{}", stamp(self.start), stamp(self.end))
    }

    /// `None` when the range covers (all but a few ms of) a `duration`-long track.
    pub fn partial(self, duration: f64) -> Option<Self> {
        let whole = self.start <= 0.01 && self.end >= duration - 0.01;
        (!whole && self.len() > 0.0).then_some(self)
    }
}

/// "1m02" (whole seconds, rounded down).
fn stamp(t: f64) -> String {
    let s = t.max(0.0).floor() as u64;
    format!("{}m{:02}", s / 60, s % 60)
}

/// Output folder name for a split: the title, plus " (1m02-1m34)" for a section.
pub fn dir_name(title: &str, range: Option<SplitRange>) -> String {
    match range {
        Some(r) => format!("{title} ({})", r.label()),
        None => title.to_owned(),
    }
}

// ---- cutting -------------------------------------------------------------------------------

/// A section cut out of the input with [`PAD_SECS`] of context on each side.
pub struct Cut {
    pub audio: AudioBuffer,
    /// Frames of padding before the section.
    lead: usize,
    /// Frames in the section itself; `None` = the whole input, nothing to trim.
    len: Option<usize>,
}

/// Cut `mix` to `range` plus padding (clamped to the track). `None` keeps all of it.
pub fn cut(mix: &AudioBuffer, range: Option<SplitRange>) -> Cut {
    let Some(r) = range else {
        return Cut {
            audio: mix.clone(),
            lead: 0,
            len: None,
        };
    };
    let sr = mix.sample_rate as f64;
    let n = mix.frames();
    let f0 = ((r.start * sr).round() as usize).min(n);
    let f1 = ((r.end * sr).round() as usize).clamp(f0, n);
    let pad = (PAD_SECS * sr) as usize;
    let p0 = f0.saturating_sub(pad);
    let p1 = (f1 + pad).min(n);
    Cut {
        audio: mix.slice(p0, p1),
        lead: f0 - p0,
        len: Some(f1 - f0),
    }
}

impl Cut {
    /// Trim the padding off separated stems (same rate and length as [`Cut::audio`]).
    pub fn trim(&self, stems: Vec<(StemKind, AudioBuffer)>) -> Vec<(StemKind, AudioBuffer)> {
        let Some(len) = self.len else { return stems };
        stems
            .into_iter()
            .map(|(k, b)| (k, b.slice(self.lead, self.lead + len)))
            .collect()
    }
}

/// `audio` cut to `range` without padding (demo mode).
pub fn slice(audio: &AudioBuffer, range: Option<SplitRange>) -> AudioBuffer {
    let mut c = cut(audio, range);
    match c.len {
        Some(len) => c.audio.slice(c.lead, c.lead + len),
        None => std::mem::take(&mut c.audio),
    }
}

// ---- origin --------------------------------------------------------------------------------

const ORIGIN_FILE: &str = "origin.json";

/// Where a split's stems came from, kept next to them in `origin.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Origin {
    /// The link or file the user gave.
    pub source: String,
    /// The local audio file that was split (the download, or the user's file).
    pub input: Option<PathBuf>,
    /// The section of `input` the stems cover; `None` = the whole track.
    pub range: Option<SplitRange>,
    pub model_name: String,
}

impl Origin {
    pub fn load(out_dir: &Path) -> Self {
        std::fs::read_to_string(out_dir.join(ORIGIN_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, out_dir: &Path) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(out_dir.join(ORIGIN_FILE), json);
        }
    }

    /// A selection on the results page (seconds from the section start) as a range of the
    /// original input.
    pub fn absolute(&self, sel: (f64, f64)) -> SplitRange {
        let off = self.range.map_or(0.0, |r| r.start);
        SplitRange::new(off + sel.0, off + sel.1)
    }
}

// ---- pre-split step ------------------------------------------------------------------------

/// The decoded input, ready for picking a section.
pub struct Prepared {
    /// What the user gave (shown in history; retried on failure).
    pub source: InputSource,
    pub title: String,
    /// Local file the split job reads.
    pub input: PathBuf,
    /// The full mix, as one "stem" for the player.
    pub audio: Arc<Vec<AudioBuffer>>,
    pub env: Envelope,
    pub analysis: Option<TrackAnalysis>,
    pub duration: f64,
}

/// Fetch, decode and analyse `req.source` on a worker thread, then send
/// [`JobEvent::Prepared`] (or `Failed` / `Cancelled`) under `req.id`.
pub fn spawn_prepare(req: JobRequest, cancel: Arc<AtomicBool>, n: Notifier) {
    std::thread::Builder::new()
        .name(format!("prepare-{}", req.id))
        .spawn(move || {
            let id = req.id;
            let send = |ev| n.send(AppEvent::Job(id, ev));
            let result = guard("prepare", || prepare(&req, &cancel, &send));
            match result {
                Ok(Ok(p)) => send(JobEvent::Prepared(Box::new(p))),
                Ok(Err(_)) if cancel.load(Ordering::Relaxed) => send(JobEvent::Cancelled),
                Ok(Err(e)) | Err(e) => send(JobEvent::Failed(e)),
            }
        })
        .expect("spawn prepare thread");
}

fn prepare(
    req: &JobRequest,
    cancel: &AtomicBool,
    send: &dyn Fn(JobEvent),
) -> Result<Prepared, String> {
    let stop = || {
        if cancel.load(Ordering::Relaxed) {
            Err("cancelled".to_owned())
        } else {
            Ok(())
        }
    };
    let (input, title, mix) = if req.demo {
        if matches!(req.source, InputSource::Url(_)) {
            send(JobEvent::Stage(
                Stage::Fetching,
                "Downloading audio with yt-dlp".into(),
            ));
            for i in 0..=30 {
                stop()?;
                send(JobEvent::Progress(Some(i as f32 / 30.0)));
                std::thread::sleep(Duration::from_millis(30));
            }
        }
        send(JobEvent::Stage(Stage::Decoding, "Reading audio".into()));
        send(JobEvent::Progress(None));
        std::thread::sleep(Duration::from_millis(400));
        let title = req.title.clone().unwrap_or_else(|| match &req.source {
            InputSource::File(_) => req.source.provisional_title(),
            InputSource::Url(_) => crate::demo::DEMO_TITLE.to_owned(),
        });
        send(JobEvent::Title(title.clone()));
        let input = match &req.source {
            InputSource::File(p) => p.clone(),
            InputSource::Url(_) => std::env::temp_dir().join("stem-splitter-demo/input.wav"),
        };
        (input, title, demo_mix())
    } else {
        let (path, title) = crate::pipeline::fetch_input(req, cancel, send)?;
        stop()?;
        send(JobEvent::Stage(Stage::Decoding, "Reading audio".into()));
        send(JobEvent::Progress(None));
        let mix = stem_audio::decode_file(&path).map_err(|e| e.to_string())?;
        (path, title, mix)
    };
    stop()?;
    send(JobEvent::Stage(Stage::Decoding, "Finding the beat".into()));
    send(JobEvent::Progress(None));
    let mix = mix.to_stereo();
    let env = Envelope::from_buffer(&mix, BUCKETS);
    let analysis = crate::analysis::analyze_stems(&[(StemKind::Other, &mix)]);
    stop()?;
    Ok(Prepared {
        source: req.source.clone(),
        title,
        input,
        duration: mix.duration_secs(),
        audio: Arc::new(vec![mix]),
        env,
        analysis,
    })
}

// ---- picker state --------------------------------------------------------------------------

/// The open section picker.
pub struct SectionPick {
    pub prep: Prepared,
    /// In / out points in seconds.
    pub sel: (f64, f64),
    pub snap: SnapMode,
    /// Full-track view, for the beat grid.
    pub view: Timeline,
}

impl SectionPick {
    /// Starts on the whole track, or on `preset` (clamped) when given.
    pub fn new(prep: Prepared, preset: Option<SplitRange>) -> Self {
        let dur = prep.duration;
        let sel = preset
            .map(|r| (r.start.clamp(0.0, dur), r.end.clamp(0.0, dur)))
            .filter(|(a, b)| b - a >= MIN_SECS)
            .unwrap_or((0.0, dur));
        let view = Timeline::new(dur, prep.audio.clone());
        Self {
            prep,
            sel,
            snap: SnapMode::Bar,
            view,
        }
    }

    pub fn grid(&self) -> Option<&stem_audio::analysis::BeatGrid> {
        self.prep.analysis.as_ref().and_then(|a| a.grid.as_ref())
    }

    /// The section to split, `None` for the whole track.
    pub fn range(&self) -> Option<SplitRange> {
        SplitRange::new(self.sel.0, self.sel.1).partial(self.prep.duration)
    }

    pub fn whole(&mut self) {
        self.sel = (0.0, self.prep.duration);
    }

    /// Snap `t` to the grid (unless `free`), within the track.
    pub fn snap_time(&self, t: f64, free: bool) -> f64 {
        let dur = self.prep.duration;
        let t = t.clamp(0.0, dur);
        if free {
            return t;
        }
        crate::analysis::snap(self.grid(), self.snap, t, dur)
    }

    /// Move one edge (`0` = in, `1` = out) to `t`, keeping at least [`MIN_SECS`] between them.
    pub fn set_edge(&mut self, edge: usize, t: f64) {
        let dur = self.prep.duration;
        if edge == 0 {
            self.sel.0 = t.clamp(0.0, (self.sel.1 - MIN_SECS).max(0.0));
        } else {
            self.sel.1 = t.clamp((self.sel.0 + MIN_SECS).min(dur), dur);
        }
    }

    /// Slide the whole section so it starts at `start`, keeping its length.
    pub fn slide_to(&mut self, start: f64) {
        let len = self.sel.1 - self.sel.0;
        let a = start.clamp(0.0, (self.prep.duration - len).max(0.0));
        self.sel = (a, a + len);
    }

    /// Make the section exactly `bars` bars long from the in point (needs a beat grid).
    pub fn set_bars(&mut self, bars: usize) -> bool {
        let Some(g) = self.grid() else { return false };
        let a = g.snap(self.sel.0, stem_audio::analysis::GridDivision::Bar);
        let i = g.beat_index_at(a + 1e-6);
        let b = g.beat_time(i + (bars * g.beats_per_bar.max(1)) as i64);
        let dur = self.prep.duration;
        if b > dur + 0.05 {
            return false;
        }
        self.sel = (a.clamp(0.0, dur), b.min(dur));
        true
    }

    /// Beat index of the first bar line at or after the start of the track (the grid
    /// extrapolates back over a quiet intro).
    pub fn first_bar_beat(&self) -> Option<i64> {
        let g = self.grid()?;
        let bpb = g.beats_per_bar.max(1) as i64;
        let mut i = g.downbeat as i64;
        while g.beat_time(i - bpb) >= -0.05 {
            i -= bpb;
        }
        while g.beat_time(i) < -0.05 {
            i += bpb;
        }
        Some(i)
    }

    /// 1-based bar number at `t`, counted from the track's first bar line.
    pub fn bar_number(&self, t: f64) -> Option<i64> {
        let g = self.grid()?;
        let first = self.first_bar_beat()?;
        let bpb = g.beats_per_bar.max(1) as i64;
        Some((g.beat_index_at(t + 1e-6) - first).div_euclid(bpb) + 1)
    }

    /// Length of the section in bars, when there is a grid.
    pub fn bars(&self) -> Option<f64> {
        self.grid().map(|g| g.bars_between(self.sel.0, self.sel.1))
    }
}

/// Rough wall-clock seconds to split `audio_secs` of audio with a model of `arch` (padding
/// and model loading included). Ballpark figures for a mid-range GPU / 8-core CPU.
pub fn estimate_secs(arch: &ModelArch, gpu: bool, audio_secs: f64) -> f64 {
    // Seconds of work per second of audio.
    let (on_gpu, on_cpu) = match arch {
        ModelArch::MdxNet { .. } => (0.05, 0.45),
        ModelArch::HtDemucs { .. } => (0.12, 0.9),
        ModelArch::RoFormer { .. } => (0.5, 6.0),
        ModelArch::Scnet { .. } => (0.08, 0.7),
    };
    let rate = if gpu { on_gpu } else { on_cpu };
    let load = if gpu { 5.0 } else { 3.0 };
    load + rate * (audio_secs + 2.0 * PAD_SECS)
}

// ---- demo ----------------------------------------------------------------------------------

/// Demo mode's "full song": the demo loop arranged into intro / verse / break / drop / outro
/// (36 bars, 96 s), so the picker has some shape to look at.
pub fn demo_arrangement(kind: StemKind) -> AudioBuffer {
    let lp = crate::demo::stem(kind);
    let sr = lp.sample_rate;
    let bar = (crate::demo::demo_duration() as f64 / 12.0 * sr as f64) as usize;
    let rhythm = matches!(kind, StemKind::Drums);
    let low = matches!(kind, StemKind::Bass);
    let voice = matches!(kind, StemKind::Vocals | StemKind::Dialogue);
    // (bars, gain at start, gain at end)
    let parts: [(usize, f32, f32); 5] = [
        // Intro: chords only, swelling in.
        (
            4,
            if rhythm || low || voice { 0.0 } else { 0.3 },
            if rhythm || low || voice { 0.0 } else { 1.0 },
        ),
        (12, 1.0, 1.0),
        // Break: no drums or bass.
        (
            4,
            if rhythm || low { 0.0 } else { 0.8 },
            if rhythm || low { 0.0 } else { 0.8 },
        ),
        (12, 1.0, 1.0),
        // Outro: drums and chords fading out.
        (4, if voice || low { 0.0 } else { 1.0 }, 0.0),
    ];
    let total: usize = parts.iter().map(|p| p.0 * bar).sum();
    let n = lp.frames().max(1);
    let mut chans = vec![vec![0.0f32; total]; lp.n_channels()];
    let mut at = 0;
    for (bars, g0, g1) in parts {
        let len = bars * bar;
        for i in 0..len {
            let g = g0 + (g1 - g0) * i as f32 / len as f32;
            if g == 0.0 {
                continue;
            }
            for (c, ch) in chans.iter_mut().enumerate() {
                ch[at + i] = lp.channels[c][(at + i) % n] * g;
            }
        }
        at += len;
    }
    AudioBuffer::new(sr, chans)
}

/// The demo arrangement with every stem of the 4-stem model summed.
pub fn demo_mix() -> AudioBuffer {
    let parts: Vec<AudioBuffer> = [
        StemKind::Vocals,
        StemKind::Drums,
        StemKind::Bass,
        StemKind::Other,
    ]
    .into_iter()
    .map(demo_arrangement)
    .collect();
    AudioBuffer::mix(&parts.iter().collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(sr: u32, secs: f64) -> AudioBuffer {
        let n = (sr as f64 * secs) as usize;
        let ch: Vec<f32> = (0..n).map(|i| i as f32).collect();
        AudioBuffer::new(sr, vec![ch.clone(), ch])
    }

    #[test]
    fn labels() {
        let r = SplitRange::new(94.9, 62.4);
        assert_eq!(r.start, 62.4);
        assert_eq!(r.label(), "1m02-1m34");
        assert_eq!(dir_name("Song", Some(r)), "Song (1m02-1m34)");
        assert_eq!(dir_name("Song", None), "Song");
        assert_eq!(SplitRange::new(0.0, 600.5).label(), "0m00-10m00");
    }

    #[test]
    fn whole_track_is_not_a_section() {
        assert!(SplitRange::new(0.0, 100.0).partial(100.0).is_none());
        assert!(SplitRange::new(0.005, 99.995).partial(100.0).is_none());
        assert!(SplitRange::new(10.0, 100.0).partial(100.0).is_some());
    }

    #[test]
    fn cut_pads_and_trim_restores_the_section() {
        let sr = 1000;
        let mix = ramp(sr, 10.0);
        let c = cut(&mix, Some(SplitRange::new(3.0, 5.0)));
        // 1 s of padding each side.
        assert_eq!(c.audio.frames(), 4000);
        assert_eq!(c.audio.channels[0][0], 2000.0);
        // A pass-through "model": the trimmed stem is exactly the section.
        let out = c.trim(vec![(StemKind::Vocals, c.audio.clone())]);
        let s = &out[0].1;
        assert_eq!(s.frames(), 2000);
        assert_eq!(s.channels[0][0], 3000.0);
        assert_eq!(s.channels[1][1999], 4999.0);
        // Padding is clamped at the track edges.
        let c = cut(&mix, Some(SplitRange::new(0.0, 0.5)));
        assert_eq!(c.audio.frames(), 1500);
        let out = c.trim(vec![(StemKind::Drums, c.audio.clone())]);
        assert_eq!(out[0].1.channels[0][0], 0.0);
        assert_eq!(out[0].1.frames(), 500);
        let c = cut(&mix, Some(SplitRange::new(9.5, 10.0)));
        assert_eq!(c.audio.frames(), 1500);
        assert_eq!(
            c.trim(vec![(StemKind::Bass, c.audio.clone())])[0]
                .1
                .frames(),
            500
        );
        // No range: untouched.
        let c = cut(&mix, None);
        assert_eq!(c.audio.frames(), 10_000);
        assert_eq!(
            c.trim(vec![(StemKind::Bass, mix.clone())])[0].1.frames(),
            10_000
        );
        assert_eq!(
            slice(&mix, Some(SplitRange::new(1.0, 2.0))).channels[0][0],
            1000.0
        );
    }

    #[test]
    fn origin_maps_selection_back_to_the_input() {
        let o = Origin {
            range: Some(SplitRange::new(60.0, 90.0)),
            ..Default::default()
        };
        assert_eq!(o.absolute((2.0, 10.0)), SplitRange::new(62.0, 70.0));
        assert_eq!(
            Origin::default().absolute((2.0, 10.0)),
            SplitRange::new(2.0, 10.0)
        );
        let dir = std::env::temp_dir().join(format!("stem-origin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let o = Origin {
            source: "https://example.com/x".into(),
            input: Some(dir.join("in.wav")),
            range: Some(SplitRange::new(1.0, 2.0)),
            model_name: "M".into(),
        };
        o.save(&dir);
        assert_eq!(Origin::load(&dir), o);
        assert_eq!(Origin::load(&dir.join("missing")), Origin::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn picker(secs: f64) -> SectionPick {
        let mix = ramp(1000, secs);
        SectionPick::new(
            Prepared {
                source: InputSource::File("x.wav".into()),
                title: "x".into(),
                input: "x.wav".into(),
                env: Envelope::default(),
                analysis: Some(TrackAnalysis {
                    tempo: None,
                    key: None,
                    grid: Some(stem_audio::analysis::BeatGrid {
                        bpm: 120.0,
                        beats: (0..200).map(|i| i as f64 * 0.5).collect(),
                        downbeat: 0,
                        beats_per_bar: 4,
                    }),
                    detected_bpm: None,
                }),
                duration: secs,
                audio: Arc::new(vec![mix]),
            },
            None,
        )
    }

    #[test]
    fn picker_edges_snap_and_clamp() {
        let mut p = picker(60.0);
        assert_eq!(p.sel, (0.0, 60.0));
        assert!(p.range().is_none());
        // Bars are 2 s long at 120 BPM in 4/4.
        assert_eq!(p.snap_time(9.2, false), 10.0);
        assert_eq!(p.snap_time(9.2, true), 9.2);
        p.set_edge(0, 10.0);
        p.set_edge(1, 5.0); // can't cross the in point
        assert_eq!(p.sel, (10.0, 11.0));
        assert!(p.set_bars(8));
        assert_eq!(p.sel, (10.0, 26.0));
        assert_eq!(p.bars(), Some(8.0));
        assert_eq!(p.first_bar_beat(), Some(0));
        assert_eq!(p.bar_number(10.0), Some(6));
        assert_eq!(p.bar_number(11.9), Some(6));
        p.slide_to(50.0);
        assert_eq!(p.sel, (44.0, 60.0));
        assert!(!p.set_bars(9)); // would run past the end
        assert_eq!(p.range(), Some(SplitRange::new(44.0, 60.0)));
        p.whole();
        assert!(p.range().is_none());
        let pre = SectionPick::new(picker(60.0).prep, Some(SplitRange::new(20.0, 90.0)));
        assert_eq!(pre.sel, (20.0, 60.0));
    }

    #[test]
    fn estimates_grow_with_length() {
        let arch = ModelArch::HtDemucs {
            segment_samples: 343_980,
        };
        let short = estimate_secs(&arch, true, 30.0);
        let long = estimate_secs(&arch, true, 240.0);
        assert!(short < long);
        assert!(estimate_secs(&arch, false, 30.0) > short);
    }

    /// The whole flow in demo mode, on worker threads like the app runs it: pre-split step,
    /// then a section split; the stems cover just the section and remember where it came from.
    #[test]
    fn demo_prepare_then_split_section() {
        use crate::pipeline::{AppEvent, JobOutput};
        let (tx, rx) = crossbeam_channel::unbounded();
        let n = Notifier::new(tx, eframe::egui::Context::default());
        let tmp = std::env::temp_dir().join(format!("stem-section-e2e-{}", std::process::id()));
        let paths = crate::paths::AppPaths {
            models: tmp.clone(),
            tools: tmp.join("tools"),
            downloads: tmp.join("downloads"),
            data: tmp.clone(),
        };
        let req = JobRequest {
            id: 7,
            source: InputSource::Url("https://example.com/watch?v=x".into()),
            model: stem_models::registry().into_iter().next().unwrap(),
            device: stem_models::Device::Cpu,
            output_root: tmp.join("out"),
            wav: stem_audio::WavFormat::Pcm16,
            cookies_from_browser: None,
            paths,
            demo: true,
            range: None,
            title: None,
            chain: None,
        };
        let wait = |want_done: bool| -> Result<AppEvent, String> {
            loop {
                match rx
                    .recv_timeout(Duration::from_secs(60))
                    .map_err(|e| e.to_string())?
                {
                    AppEvent::Job(_, JobEvent::Failed(e)) => return Err(e),
                    ev @ AppEvent::Job(_, JobEvent::Prepared(_)) if !want_done => return Ok(ev),
                    ev @ AppEvent::Job(_, JobEvent::Done(_)) if want_done => return Ok(ev),
                    _ => {}
                }
            }
        };
        spawn_prepare(req.clone(), Arc::new(AtomicBool::new(false)), n.clone());
        let AppEvent::Job(_, JobEvent::Prepared(prep)) = wait(false).unwrap() else {
            unreachable!()
        };
        assert!((prep.duration - 96.0).abs() < 0.1);
        let mut pick = SectionPick::new(*prep, None);
        pick.sel = (pick.snap_time(32.0, false), 0.0);
        assert!(pick.set_bars(4));
        let range = pick.range().unwrap();
        let split = JobRequest {
            source: InputSource::File(pick.prep.input.clone()),
            title: Some(pick.prep.title.clone()),
            range: Some(range),
            ..req
        };
        crate::pipeline::spawn_job(split, Arc::new(AtomicBool::new(false)), n);
        let AppEvent::Job(_, JobEvent::Done(out)) = wait(true).unwrap() else {
            unreachable!()
        };
        let out: JobOutput = *out;
        for s in &out.stems {
            assert!((s.audio.duration_secs() - range.len()).abs() < 0.01);
            assert!(s.path.is_file());
        }
        let dir = out
            .out_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(dir.ends_with(&format!("({})", range.label())), "{dir}");
        assert_eq!(Origin::load(&out.out_dir).range, Some(range));
        let _ = std::fs::remove_dir_all(&out.out_dir);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn demo_arrangement_has_structure() {
        let d = demo_arrangement(StemKind::Drums);
        assert!((d.duration_secs() - 96.0).abs() < 0.1);
        let sr = d.sample_rate as f64;
        let rms = |a: f64, b: f64| {
            let s = &d.channels[0][(a * sr) as usize..(b * sr) as usize];
            (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt()
        };
        assert_eq!(rms(0.0, 10.0), 0.0); // intro: no drums
        assert!(rms(12.0, 40.0) > 0.01);
    }
}
