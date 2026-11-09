//! Waveform zoom & scroll: the visible time window, wheel / Ctrl+wheel handling, beat-grid
//! drawing, and a multi-resolution envelope cache so zoomed-in lanes stay sharp and cheap.

use std::sync::{Arc, OnceLock};

use eframe::egui::{self, Color32, Painter, Rect, Stroke};
use stem_audio::AudioBuffer;
use stem_audio::analysis::{BeatGrid, GridDivision, GridLevel};

use crate::waveform::Envelope;

/// Shortest visible span, seconds.
const MIN_SPAN: f64 = 0.25;

/// The visible part of the track, plus the envelope cache for zoomed drawing.
#[derive(Clone)]
pub struct Timeline {
    pub start: f64,
    pub end: f64,
    pub duration: f64,
    /// One pyramid per stem, built in the background when the session opens.
    lod: Arc<OnceLock<Vec<Pyramid>>>,
}

impl Timeline {
    /// Full view of a track; starts building the zoom cache for `audio` on a worker thread.
    pub fn new(duration: f64, audio: Arc<Vec<AudioBuffer>>) -> Self {
        let lod = Arc::new(OnceLock::new());
        let cell = lod.clone();
        std::thread::Builder::new()
            .name("waveform-lod".into())
            .spawn(move || {
                let _ = cell.set(audio.iter().map(Pyramid::build).collect());
            })
            .ok();
        Self {
            start: 0.0,
            end: duration.max(MIN_SPAN),
            duration: duration.max(MIN_SPAN),
            lod,
        }
    }

    pub fn span(&self) -> f64 {
        self.end - self.start
    }

    pub fn is_zoomed(&self) -> bool {
        self.span() < self.duration * 0.999
    }

    /// Zoom factor relative to the whole track (1 = everything visible).
    pub fn zoom(&self) -> f64 {
        self.duration / self.span()
    }

    pub fn pyramid(&self, track: usize) -> Option<&Pyramid> {
        self.lod.get().and_then(|v| v.get(track))
    }

    /// Fraction of the visible window → time.
    pub fn time_at(&self, frac: f64) -> f64 {
        self.start + frac * self.span()
    }

    /// Time → fraction of the visible window (may be outside `0..=1`).
    pub fn frac_of(&self, t: f64) -> f64 {
        (t - self.start) / self.span()
    }

    /// Zoom by `factor` (> 1 zooms in) keeping time `anchor` at the same screen position.
    pub fn zoom_around(&mut self, anchor: f64, factor: f64) {
        let span = (self.span() / factor).clamp(MIN_SPAN.min(self.duration), self.duration);
        let f = self.frac_of(anchor).clamp(0.0, 1.0);
        self.start = anchor - f * span;
        self.end = self.start + span;
        self.clamp();
    }

    /// Move the window by `dt` seconds.
    pub fn scroll(&mut self, dt: f64) {
        self.start += dt;
        self.end += dt;
        self.clamp();
    }

    /// Centre the window on `t` (keeping the zoom).
    pub fn center_on(&mut self, t: f64) {
        let span = self.span();
        self.start = t - span / 2.0;
        self.end = self.start + span;
        self.clamp();
    }

    pub fn reset(&mut self) {
        self.start = 0.0;
        self.end = self.duration;
    }

    fn clamp(&mut self) {
        let span = self.span().min(self.duration);
        if self.start < 0.0 {
            self.start = 0.0;
        }
        if self.start + span > self.duration {
            self.start = self.duration - span;
        }
        self.end = self.start + span;
    }

    /// Ctrl+wheel zooms around the pointer, plain / Shift wheel scrolls while zoomed.
    /// Call with the area the lanes cover and the x range they map time onto. Consumes the
    /// wheel so the page doesn't scroll as well.
    pub fn handle_input(&mut self, ui: &egui::Ui, area: Rect, x0: f32, x1: f32) {
        let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        if !area.contains(pos) {
            return;
        }
        let (zoom, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
        let w = (x1 - x0).max(1.0);
        let anchor = self.time_at((((pos.x - x0) / w).clamp(0.0, 1.0)) as f64);
        if (zoom - 1.0).abs() > 1e-4 {
            self.zoom_around(anchor, zoom as f64);
            ui.ctx().request_repaint();
        }
        if self.is_zoomed() {
            // Horizontal (Shift / trackpad) or vertical wheel both pan.
            let d = if scroll.x.abs() > 0.0 {
                scroll.x
            } else {
                scroll.y
            };
            if d != 0.0 {
                self.scroll(-(d as f64) / w as f64 * self.span());
                ui.ctx()
                    .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
            }
        }
    }

    /// While playing, page the window along when the playhead runs off its right edge.
    pub fn follow(&mut self, playhead: f64) {
        if self.is_zoomed() && (playhead > self.end || playhead < self.start) {
            let span = self.span();
            self.start = playhead - span * 0.05;
            self.end = self.start + span;
            self.clamp();
        }
    }
}

// ---- envelope pyramid ----------------------------------------------------------------------

/// Samples per bucket of the finest cached level; below this we read the audio directly.
const BASE: usize = 64;
/// Each coarser level merges this many buckets.
const STEP: usize = 4;

struct Level {
    per: usize,
    min: Vec<f32>,
    max: Vec<f32>,
    rms: Vec<f32>,
}

/// Min / max / RMS envelopes at 64, 256, 1024, ... samples per bucket.
pub struct Pyramid {
    levels: Vec<Level>,
}

impl Pyramid {
    pub fn build(buf: &AudioBuffer) -> Self {
        let frames = buf.frames();
        let n = frames.div_ceil(BASE).max(1);
        let ch = buf.n_channels().max(1) as f32;
        let mut base = Level {
            per: BASE,
            min: vec![0.0; n],
            max: vec![0.0; n],
            rms: vec![0.0; n],
        };
        for b in 0..n {
            let (s, e) = (b * BASE, ((b + 1) * BASE).min(frames));
            let (mut lo, mut hi, mut sq) = (0.0f32, 0.0f32, 0.0f32);
            for c in &buf.channels {
                for &v in &c[s.min(e)..e] {
                    lo = lo.min(v);
                    hi = hi.max(v);
                    sq += v * v;
                }
            }
            base.min[b] = lo;
            base.max[b] = hi;
            base.rms[b] = (sq / ((e - s).max(1) as f32 * ch)).sqrt();
        }
        let mut levels = vec![base];
        while levels.last().is_some_and(|l| l.min.len() > 256) {
            let prev = levels.last().expect("non-empty");
            let m = prev.min.len().div_ceil(STEP);
            let mut l = Level {
                per: prev.per * STEP,
                min: vec![0.0; m],
                max: vec![0.0; m],
                rms: vec![0.0; m],
            };
            for b in 0..m {
                let r = b * STEP..((b + 1) * STEP).min(prev.min.len());
                l.min[b] = prev.min[r.clone()].iter().copied().fold(0.0, f32::min);
                l.max[b] = prev.max[r.clone()].iter().copied().fold(0.0, f32::max);
                l.rms[b] = prev.rms[r].iter().copied().fold(0.0, f32::max);
            }
            levels.push(l);
        }
        Self { levels }
    }

    /// `(min, max, rms)` over frames `[f0, f1)`, from the coarsest level that still has at
    /// least eight buckets in the range, or from `audio` itself when zoomed in that far.
    pub fn range(&self, audio: &AudioBuffer, f0: usize, f1: usize) -> (f32, f32, f32) {
        let f1 = f1.max(f0 + 1);
        let span = f1 - f0;
        if span < BASE * 8 {
            return raw_range(audio, f0, f1);
        }
        let level = self
            .levels
            .iter()
            .rev()
            .find(|l| l.per * 8 <= span)
            .unwrap_or(&self.levels[0]);
        let n = level.min.len();
        let a = (f0 / level.per).min(n - 1);
        let b = f1.div_ceil(level.per).clamp(a + 1, n);
        let (mut lo, mut hi, mut rms) = (0.0f32, 0.0f32, 0.0f32);
        for i in a..b {
            lo = lo.min(level.min[i]);
            hi = hi.max(level.max[i]);
            rms = rms.max(level.rms[i]);
        }
        (lo, hi, rms)
    }
}

fn raw_range(audio: &AudioBuffer, f0: usize, f1: usize) -> (f32, f32, f32) {
    let frames = audio.frames();
    let (a, b) = (f0.min(frames), f1.min(frames));
    if a >= b {
        return (0.0, 0.0, 0.0);
    }
    let (mut lo, mut hi, mut sq) = (0.0f32, 0.0f32, 0.0f32);
    for c in &audio.channels {
        for &v in &c[a..b] {
            lo = lo.min(v);
            hi = hi.max(v);
            sq += v * v;
        }
    }
    let rms = (sq / ((b - a) * audio.n_channels().max(1)) as f32).sqrt();
    (lo, hi, rms)
}

/// Where a lane gets its column envelopes from.
#[derive(Clone, Copy)]
pub struct WaveSource<'a> {
    pub env: &'a Envelope,
    pub pyramid: Option<&'a Pyramid>,
    pub audio: Option<&'a AudioBuffer>,
}

impl WaveSource<'_> {
    /// `(min, max, rms)` between `t0` and `t1` seconds of a `duration`-long track.
    pub fn range(&self, t0: f64, t1: f64, duration: f64) -> (f32, f32, f32) {
        if let (Some(p), Some(a)) = (self.pyramid, self.audio) {
            let sr = a.sample_rate as f64;
            let f0 = (t0.max(0.0) * sr) as usize;
            let f1 = (t1.max(0.0) * sr).ceil() as usize;
            return p.range(a, f0, f1);
        }
        let d = duration.max(1e-9);
        self.env.range((t0 / d) as f32, (t1 / d) as f32)
    }
}

// ---- grid drawing --------------------------------------------------------------------------

/// Subtle beat / bar lines over a lane. Beats are only drawn when they're far enough apart.
pub fn paint_grid(p: &Painter, rect: Rect, view: &Timeline, grid: &BeatGrid, accent: Color32) {
    let px_per_beat = rect.width() as f64 / (view.span() / grid.period());
    if px_per_beat < 1.5 {
        return;
    }
    let division = if px_per_beat > 48.0 {
        GridDivision::HalfBeat
    } else if px_per_beat > 7.0 {
        GridDivision::Beat
    } else {
        GridDivision::Bar
    };
    let w = rect.width();
    for (t, level) in grid.points(view.start, view.end, division) {
        let x = rect.left() + (view.frac_of(t) as f32) * w;
        if x < rect.left() || x > rect.right() {
            continue;
        }
        let full = rect.y_range();
        let inner = egui::Rangef::new(
            rect.top() + rect.height() * 0.3,
            rect.bottom() - rect.height() * 0.3,
        );
        let (c, y) = match level {
            GridLevel::Bar => (crate::theme::alpha(accent, 0.32), full),
            GridLevel::Beat => (Color32::from_white_alpha(14), full),
            GridLevel::HalfBeat => (Color32::from_white_alpha(6), inner),
        };
        p.vline(x, y, Stroke::new(1.0, c));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tl(d: f64) -> Timeline {
        Timeline {
            start: 0.0,
            end: d,
            duration: d,
            lod: Arc::new(OnceLock::new()),
        }
    }

    #[test]
    fn zoom_keeps_anchor_and_clamps() {
        let mut t = tl(100.0);
        t.zoom_around(25.0, 4.0);
        assert!((t.span() - 25.0).abs() < 1e-9);
        assert!((t.frac_of(25.0) - 0.25).abs() < 1e-9);
        t.scroll(-1000.0);
        assert_eq!(t.start, 0.0);
        t.scroll(1000.0);
        assert!((t.end - 100.0).abs() < 1e-9);
        t.zoom_around(50.0, 1e9);
        assert!((t.span() - MIN_SPAN).abs() < 1e-9);
        t.zoom_around(50.0, 1e-9);
        assert!(!t.is_zoomed());
    }

    #[test]
    fn pyramid_matches_raw() {
        let x: Vec<f32> = (0..100_000).map(|i| ((i as f32) * 0.001).sin()).collect();
        let buf = AudioBuffer::new(44_100, vec![x]);
        let p = Pyramid::build(&buf);
        assert!(p.levels.len() >= 3);
        for (f0, f1) in [
            (0, 100_000),
            (1_000, 9_000),
            (50_000, 50_090),
            (12_345, 54_321),
        ] {
            let (lo, hi, _) = p.range(&buf, f0, f1);
            let (rlo, rhi, _) = raw_range(&buf, f0, f1);
            // Bucket edges can widen the range a little, never narrow it.
            assert!(lo <= rlo + 1e-6 && hi >= rhi - 1e-6, "{f0}..{f1}");
            assert!(
                (hi - rhi).abs() < 0.02 && (lo - rlo).abs() < 0.02,
                "{f0}..{f1}"
            );
        }
    }
}
