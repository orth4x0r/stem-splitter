//! "Split drums" and "Clean vocals": one more model run on a stem the open session already
//! has, whose stems join the session as a sub-group under it (kick / snare / toms / hi-hat /
//! ride / crash under Drums, dry + reverb or clean + noise under Vocals).
//!
//! Runs on a worker thread like a split job (progress through [`AppEvent::Refine`], the
//! [`AtomicBool`] cancel flag checked between model segments), through the same [`Runner`]s as
//! chains, and writes the new stems as WAVs next to the others so export, drag-out, chop, kit
//! and re-opening from the library treat them like any other stem.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use stem_audio::{AudioBuffer, WavFormat};
use stem_models::{Device, ModelInfo, StemKind};

use super::chain::{self, Runner};
use super::{AppEvent, Notifier, StemOut};
use crate::backend::guard;
use crate::paths::AppPaths;
use crate::waveform::{BUCKETS, Envelope};

/// What a refine run does to its stem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefineOp {
    /// Drum parts from a drums stem.
    DrumParts,
    /// De-reverb / de-echo / denoise a vocal (or any) stem.
    CleanUp,
}

impl RefineOp {
    /// The op a model is for, from what it outputs.
    pub fn of(model: &ModelInfo) -> Option<Self> {
        if model.stems.iter().any(|k| k.is_drum_part()) {
            Some(Self::DrumParts)
        } else if model
            .stems
            .iter()
            .any(|k| matches!(k, StemKind::Dry | StemKind::Clean))
        {
            Some(Self::CleanUp)
        } else {
            None
        }
    }

    /// The op offered on a stem card of `kind`.
    pub fn for_stem(kind: StemKind) -> Option<Self> {
        match kind {
            StemKind::Drums => Some(Self::DrumParts),
            StemKind::Vocals
            | StemKind::LeadVocals
            | StemKind::BackingVocals
            | StemKind::Dialogue
            | StemKind::Dry => Some(Self::CleanUp),
            _ => None,
        }
    }

    /// Button label.
    pub fn verb(self) -> &'static str {
        match self {
            Self::DrumParts => "Split drums",
            Self::CleanUp => "Clean vocals",
        }
    }

    /// Progress line, "Splitting drums".
    pub fn doing(self) -> &'static str {
        match self {
            Self::DrumParts => "Splitting drums",
            Self::CleanUp => "Cleaning vocals",
        }
    }
}

/// The stem a sub-stem hangs under in the results, given the session's stem kinds: drum parts
/// under Drums, clean-up results under the vocal they came from.
pub fn parent_kind(kind: StemKind, present: &[StemKind]) -> Option<StemKind> {
    let candidates: &[StemKind] = if kind.is_drum_part() {
        &[StemKind::Drums]
    } else if matches!(kind, StemKind::Dry | StemKind::Reverb) {
        &[
            StemKind::Vocals,
            StemKind::LeadVocals,
            StemKind::BackingVocals,
            StemKind::Dialogue,
        ]
    } else if matches!(kind, StemKind::Clean | StemKind::Noise) {
        &[
            StemKind::Dry,
            StemKind::Vocals,
            StemKind::LeadVocals,
            StemKind::BackingVocals,
            StemKind::Dialogue,
        ]
    } else {
        &[]
    };
    candidates.iter().copied().find(|k| present.contains(k))
}

pub struct RefineRequest {
    pub id: u64,
    /// The stem being refined.
    pub parent: StemKind,
    pub input: AudioBuffer,
    pub model: ModelInfo,
    pub device: Device,
    pub paths: AppPaths,
    /// The session's folder: the new WAVs go next to the other stems.
    pub out_dir: PathBuf,
    pub wav: WavFormat,
    /// Tempo / key tags for the new WAVs.
    pub meta: stem_audio::wavmeta::WavMeta,
    /// Stem kinds the session already has; those outputs are dropped, never overwritten.
    pub existing: Vec<StemKind>,
    pub demo: bool,
    /// Chunk overlap for the model run.
    pub quality: crate::quality::SplitQuality,
}

pub enum RefineEvent {
    /// Detail line and fraction of the model run (`None` = indeterminate).
    Progress(String, Option<f32>),
    Done(Vec<StemOut>),
    Failed(String),
    Cancelled,
}

/// Start a refine run. Returns immediately.
pub fn spawn(req: RefineRequest, cancel: Arc<AtomicBool>, n: Notifier) {
    std::thread::Builder::new()
        .name(format!("refine-{}", req.id))
        .spawn(move || {
            let id = req.id;
            let send = |ev| n.send(AppEvent::Refine(id, ev));
            let result = guard("refine", || run(&req, &cancel, &send));
            send(match result {
                Ok(Ok(stems)) => RefineEvent::Done(stems),
                Ok(Err(_)) if cancel.load(Ordering::Relaxed) => RefineEvent::Cancelled,
                Ok(Err(e)) | Err(e) => RefineEvent::Failed(e),
            });
        })
        .expect("spawn refine thread");
}

fn run(
    req: &RefineRequest,
    cancel: &AtomicBool,
    send: &dyn Fn(RefineEvent),
) -> Result<Vec<StemOut>, String> {
    let mut runner: Box<dyn Runner> = if req.demo {
        Box::new(chain::DemoRunner)
    } else {
        Box::new(chain::OrtRunner::with(&req.paths, req.device, req.quality))
    };
    let emit = |detail: &str, f: Option<f32>| send(RefineEvent::Progress(detail.to_owned(), f));
    runner.prepare(&emit)?;
    super::check(cancel)?;
    let stems = runner.separate(&req.model, &req.input, &emit, cancel)?;
    super::check(cancel)?;
    emit("saving", None);
    write(req, stems)
}

/// Write the new stems (skipping kinds the session has) as `<stem>.wav` in the session folder,
/// the name re-opening from the library looks for.
fn write(req: &RefineRequest, stems: Vec<(StemKind, AudioBuffer)>) -> Result<Vec<StemOut>, String> {
    std::fs::create_dir_all(&req.out_dir).map_err(|e| format!("{}: {e}", req.out_dir.display()))?;
    let mut out = Vec::new();
    for (kind, audio) in stems {
        if req.existing.contains(&kind) || kind == req.parent {
            continue;
        }
        let path = req
            .out_dir
            .join(format!("{}.wav", kind.label().to_lowercase()));
        stem_audio::write_wav(&path, &audio, req.wav).map_err(|e| e.to_string())?;
        if !req.meta.is_empty() {
            let _ = stem_audio::wavmeta::apply_to_file(&path, &req.meta);
        }
        let env = Envelope::from_buffer(&audio, BUCKETS);
        out.push(StemOut {
            kind,
            audio,
            path,
            env,
        });
    }
    if out.is_empty() {
        return Err("the model gave no new stems".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_and_parents() {
        let drums = stem_models::find_model("drumsep-mdx23c").unwrap();
        assert_eq!(RefineOp::of(&drums), Some(RefineOp::DrumParts));
        for id in ["mdx-reverb-hq", "mdx23c-dereverb", "mel-roformer-denoise"] {
            let m = stem_models::find_model(id).unwrap();
            assert_eq!(RefineOp::of(&m), Some(RefineOp::CleanUp), "{id}");
        }
        assert_eq!(
            RefineOp::of(&stem_models::find_model("htdemucs").unwrap()),
            None
        );
        assert_eq!(
            RefineOp::for_stem(StemKind::Drums),
            Some(RefineOp::DrumParts)
        );
        assert_eq!(RefineOp::for_stem(StemKind::Bass), None);

        use StemKind::*;
        let present = [Vocals, Drums, Bass, Other];
        assert_eq!(parent_kind(Kick, &present), Some(Drums));
        assert_eq!(parent_kind(Dry, &present), Some(Vocals));
        assert_eq!(parent_kind(Clean, &[Vocals, Dry]), Some(Dry));
        assert_eq!(parent_kind(Bass, &present), None);
        // A chain that consumed the drums: the parts stand on their own.
        assert_eq!(parent_kind(Snare, &[Vocals, Bass]), None);
    }

    #[test]
    fn demo_run_writes_new_stems_only() {
        let dir = std::env::temp_dir().join(format!("stem-refine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let req = RefineRequest {
            id: 1,
            parent: StemKind::Vocals,
            input: crate::demo::stem(StemKind::Vocals),
            model: stem_models::find_model("mdx-reverb-hq").unwrap(),
            device: Device::Cpu,
            paths: AppPaths {
                models: dir.clone(),
                tools: dir.join("tools"),
                downloads: dir.join("downloads"),
                data: dir.clone(),
            },
            out_dir: dir.join("out"),
            wav: WavFormat::Pcm16,
            meta: Default::default(),
            existing: vec![StemKind::Vocals, StemKind::Reverb],
            demo: true,
            quality: Default::default(),
        };
        let seen = std::cell::RefCell::new(Vec::new());
        let stems = run(&req, &AtomicBool::new(false), &|ev| {
            if let RefineEvent::Progress(_, f) = ev {
                seen.borrow_mut().push(f);
            }
        })
        .unwrap();
        // Reverb was already there: only the dry stem is new.
        assert_eq!(stems.len(), 1);
        assert_eq!(stems[0].kind, StemKind::Dry);
        assert!(stems[0].path.ends_with("dry.wav") && stems[0].path.is_file());
        assert!(!seen.borrow().is_empty());
        // Cancelled before it starts.
        let r = run(&req, &AtomicBool::new(true), &|_| {});
        assert!(r.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
