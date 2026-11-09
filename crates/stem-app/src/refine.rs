//! The app side of "Split drums" / "Clean vocals" (worker: `pipeline::refine`): which models a
//! stem card offers, starting and cancelling a run, and folding its stems into the session as
//! a sub-group under the stem they came from.
//!
//! New stems are appended to `Session::tracks` (so every index other features hold, such as
//! the chop stem, the selection lane or the kit, stays valid); the results view draws each one
//! under its parent through [`Session::display_order`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use stem_models::{ModelInfo, StemKind};

use crate::app::{Session, StemApp, Track};
use crate::pipeline::refine::{self, RefineEvent, RefineOp, RefineRequest};

/// A refine run in progress (one at a time).
pub struct RefineJob {
    pub id: u64,
    pub op: RefineOp,
    /// Index of the stem being refined in `Session::tracks`.
    pub track: usize,
    pub model_name: String,
    /// The model's id (it may download during the run).
    pub(crate) model_id: &'static str,
    pub detail: String,
    pub frac: Option<f32>,
    /// Displayed progress, eased towards `frac`.
    pub shown: f32,
    pub started: f64,
    pub cancel: Arc<AtomicBool>,
    /// The session it belongs to.
    pub(crate) out_dir: std::path::PathBuf,
}

/// One model a stem card can offer, with what stops it running here.
pub struct Offer {
    pub model: ModelInfo,
    /// `None` = ready (downloaded, or downloads when picked); else why not.
    pub blocked: Option<String>,
    pub downloaded: bool,
}

impl Session {
    /// Track indices in drawing order: each stem followed by the sub-stems split from it.
    pub fn display_order(&self) -> Vec<usize> {
        let mut order = Vec::with_capacity(self.tracks.len());
        let tops: Vec<usize> = (0..self.tracks.len())
            .filter(|&i| self.parent_index(i).is_none())
            .collect();
        for i in tops {
            self.push_with_children(i, &mut order, 0);
        }
        // Anything left (a cycle can't happen, but never drop a stem).
        for i in 0..self.tracks.len() {
            if !order.contains(&i) {
                order.push(i);
            }
        }
        order
    }

    fn push_with_children(&self, i: usize, order: &mut Vec<usize>, depth: usize) {
        order.push(i);
        if depth > 3 {
            return;
        }
        for j in 0..self.tracks.len() {
            if j != i && self.parent_index(j) == Some(i) && !order.contains(&j) {
                self.push_with_children(j, order, depth + 1);
            }
        }
    }

    /// The track a sub-stem hangs under.
    pub fn parent_index(&self, i: usize) -> Option<usize> {
        let p = self.tracks.get(i)?.parent?;
        self.tracks.iter().position(|t| t.kind == p)
    }

    /// How deep a track sits in the sub-group tree (0 = top level).
    pub fn depth(&self, i: usize) -> usize {
        let mut d = 0;
        let mut cur = i;
        while let Some(p) = self.parent_index(cur) {
            d += 1;
            cur = p;
            if d > 3 {
                break;
            }
        }
        d
    }

    /// Sub-stems split from track `i`.
    pub fn children(&self, i: usize) -> Vec<usize> {
        (0..self.tracks.len())
            .filter(|&j| self.parent_index(j) == Some(i))
            .collect()
    }

    pub fn kinds(&self) -> Vec<StemKind> {
        self.tracks.iter().map(|t| t.kind).collect()
    }
}

impl StemApp {
    /// The models the card of stem `i` offers, ready ones first.
    pub fn refine_offers(&self, op: RefineOp) -> Vec<Offer> {
        let mut out: Vec<Offer> = self
            .backend
            .models
            .iter()
            .filter(|m| RefineOp::of(m) == Some(op))
            .map(|m| {
                let downloaded = self.backend.is_downloaded(m.id);
                let blocked = self.backend.needs_manual_export(m).then(|| {
                    "Not hosted: export it on your PC with the script in tools/ (see the \
                     model's description), then put the .onnx in the models folder"
                        .to_owned()
                });
                Offer {
                    model: m.clone(),
                    blocked,
                    downloaded,
                }
            })
            .collect();
        out.sort_by_key(|o| (o.blocked.is_some(), !o.downloaded));
        out
    }

    /// Start `model` on track `i`. Ignored while another refine runs.
    pub fn start_refine(&mut self, i: usize, model_id: &str, now: f64) {
        if self.refine.is_some() {
            self.toasts.info(
                "One at a time",
                "Wait for the running split to finish, or cancel it.",
            );
            return;
        }
        let Some(model) = self
            .backend
            .models
            .iter()
            .find(|m| m.id == model_id)
            .cloned()
        else {
            return;
        };
        let Some(op) = RefineOp::of(&model) else {
            return;
        };
        let Some(s) = &self.session else { return };
        let (Some(track), Some(audio)) = (s.tracks.get(i), s.audio.get(i)) else {
            return;
        };
        if self.backend.needs_manual_export(&model) {
            self.toasts.error(
                format!("{} isn't installed", model.name),
                "It isn't hosted anywhere: export it with the script in tools/ first.",
            );
            return;
        }
        let id = self.next_job_id;
        self.next_job_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let req = RefineRequest {
            id,
            parent: track.kind,
            input: audio.clone(),
            model: model.clone(),
            device: self.settings.device,
            paths: self.backend.paths.clone(),
            out_dir: s.out_dir.clone(),
            wav: self.settings.wav.format(),
            meta: crate::export::wav_meta(s.analysis.as_ref(), false, None),
            existing: s.kinds(),
            demo: self.demo(),
        };
        self.refine = Some(RefineJob {
            id,
            op,
            track: i,
            model_name: crate::pipeline::chain::short_model_name(model.name),
            model_id: model.id,
            detail: "Starting".into(),
            frac: None,
            shown: 0.0,
            started: now,
            cancel: cancel.clone(),
            out_dir: s.out_dir.clone(),
        });
        refine::spawn(req, cancel, self.notifier.clone());
    }

    pub fn cancel_refine(&mut self) {
        if let Some(j) = &self.refine {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(crate) fn on_refine_event(&mut self, id: u64, ev: RefineEvent) {
        let Some(job) = self.refine.as_mut().filter(|j| j.id == id) else {
            return;
        };
        match ev {
            RefineEvent::Progress(detail, frac) => {
                if !detail.is_empty() {
                    job.detail = detail;
                }
                if frac.is_some() {
                    job.frac = frac;
                }
            }
            RefineEvent::Cancelled => {
                self.refine = None;
                self.toasts.info("Stopped", "Nothing was added.");
            }
            RefineEvent::Failed(e) => {
                let op = job.op;
                self.refine = None;
                self.toasts.error(format!("{} failed", op.verb()), e);
            }
            RefineEvent::Done(stems) => {
                let Some(job) = self.refine.take() else {
                    return;
                };
                // It downloaded on the way if it wasn't there.
                self.backend.set_downloaded(job.model_id, true);
                let same = self
                    .session
                    .as_ref()
                    .is_some_and(|s| s.out_dir == job.out_dir);
                if !same {
                    // The session changed underneath; the files are on disk regardless.
                    return;
                }
                let n = stems.len();
                let colors: Vec<_> = stems
                    .iter()
                    .map(|st| crate::theme::stem_color(st.kind))
                    .collect();
                self.add_sub_stems(job.track, stems);
                let what = match job.op {
                    RefineOp::DrumParts => format!("{n} drum parts"),
                    RefineOp::CleanUp => format!("{n} clean-up stems"),
                };
                self.toasts
                    .success(format!("{what} ready"), job.model_name.clone());
                self.fx.celebrate(colors, what.to_string());
            }
        }
    }

    /// Append `stems` to the open session as sub-stems of track `parent`, keeping the view,
    /// the playhead and play state.
    pub(crate) fn add_sub_stems(&mut self, parent: usize, stems: Vec<crate::pipeline::StemOut>) {
        let Some(s) = &mut self.session else { return };
        let Some(parent_kind) = s.tracks.get(parent).map(|t| t.kind) else {
            return;
        };
        let mut audio: Vec<_> = s.audio.as_ref().clone();
        for st in stems {
            s.tracks.push(Track {
                kind: st.kind,
                env: st.env,
                path: st.path,
                volume: 1.0,
                mute: false,
                solo: false,
                level: 0.0,
                parent: Some(parent_kind),
            });
            audio.push(st.audio);
        }
        let audio = Arc::new(audio);
        s.audio = audio.clone();
        // Keep the view and the playhead where they were.
        let (start, end) = (s.timeline.start, s.timeline.end);
        s.timeline = crate::timeline::Timeline::new(s.duration, audio.clone());
        s.timeline.start = start;
        s.timeline.end = end;
        let pos = self.player.position();
        let playing = self.player.is_playing();
        self.player.load(audio);
        self.player.seek(pos);
        if playing {
            self.player.set_playing(true);
        }
        // The library re-opens the session with its new stems.
        let kinds = s.kinds();
        let out_dir = s.out_dir.clone();
        if let Some(h) = self
            .settings
            .history
            .iter_mut()
            .find(|h| h.out_dir == out_dir)
        {
            h.stems = kinds;
        }
    }
}
