//! Processing chains: an ordered list of steps, each running one model (or an ensemble of
//! several) on the mix or on a stem an earlier step produced. "Mel-RoFormer vocals, then
//! HTDemucs on the instrumental" is a two-step chain.
//!
//! Every model run goes through the [`StemCache`], so re-running a preset (or a chain that
//! shares its first steps with an earlier one) reuses what was already separated.
//!
//! The chain logic ([`execute`]) is independent of how a model runs: [`Runner`] does the
//! separating (ONNX Runtime for real jobs, synthetic stems in demo mode and in tests).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use stem_audio::AudioBuffer;
use stem_audio::ensemble::{EnsembleMode, ensemble};
use stem_models::{Device, ModelInfo, ModelManager, Progress, StemKind};

use super::cache::{self, StemCache};
use super::{Emit, InputSource, JobEvent, JobOutput, JobRequest, Stage, check, write_stems};
use crate::gpu_health::{self, GpuFailure};
use crate::quality::SplitQuality;

/// "Mel-RoFormer Vocals (Kim)" -> "Mel-RoFormer", "MDX-Net Kim Vocal 2" -> "MDX Kim Vocal 2":
/// short enough for progress lines and chain diagrams.
pub fn short_model_name(name: &str) -> String {
    let base = name.split(" (").next().unwrap_or(name).trim();
    let base = base.strip_suffix(" Vocals").unwrap_or(base);
    base.replace("MDX-Net ", "MDX ")
}

/// What a step separates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepInput {
    /// The original track.
    Mix,
    /// A stem from the latest earlier step that produced it.
    Stem(StemKind),
}

impl StepInput {
    pub fn label(self) -> String {
        match self {
            StepInput::Mix => "Full mix".into(),
            StepInput::Stem(k) => k.label().into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub input: StepInput,
    /// Model ids. Two or more make an ensemble: every model runs on the same input and the
    /// stems they share are combined with [`Step::ensemble`].
    pub models: Vec<String>,
    #[serde(default)]
    pub ensemble: EnsembleMode,
}

impl Step {
    pub fn new(input: StepInput, model: &str) -> Self {
        Self {
            input,
            models: vec![model.to_owned()],
            ensemble: EnsembleMode::Average,
        }
    }
    pub fn is_ensemble(&self) -> bool {
        self.models.len() > 1
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Chain {
    pub steps: Vec<Step>,
}

/// Where each step's stems end up.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Stems each step produces (an ensemble: the stems its models share).
    pub produced: Vec<Vec<StemKind>>,
    /// The chain's output: `(step, stem)` in order.
    pub outputs: Vec<(usize, StemKind)>,
}

impl Plan {
    pub fn kinds(&self) -> Vec<StemKind> {
        self.outputs.iter().map(|(_, k)| *k).collect()
    }
}

impl Chain {
    pub fn single(model: &str) -> Self {
        Self {
            steps: vec![Step::new(StepInput::Mix, model)],
        }
    }

    /// Every model id the chain uses, first use first, no repeats.
    pub fn model_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = Vec::new();
        for s in &self.steps {
            for m in &s.models {
                if !ids.contains(&m.as_str()) {
                    ids.push(m);
                }
            }
        }
        ids
    }

    /// The step whose output feeds `step`'s input (`None` = the mix).
    fn source_of(&self, step: usize, produced: &[Vec<StemKind>]) -> Option<usize> {
        match self.steps[step].input {
            StepInput::Mix => None,
            StepInput::Stem(k) => (0..step).rev().find(|&j| produced[j].contains(&k)),
        }
    }

    /// Check the chain against the model list and work out its output stems.
    ///
    /// The output keeps every stem in step order, except stems a later step consumes (the
    /// instrumental that HTDemucs splits further) and repeats of a stem an earlier step already
    /// gave (HTDemucs' leftover "vocals" from an instrumental): the first one wins.
    pub fn plan(&self, find: &dyn Fn(&str) -> Option<ModelInfo>) -> Result<Plan, String> {
        if self.steps.is_empty() {
            return Err("add at least one step".into());
        }
        let mut produced: Vec<Vec<StemKind>> = Vec::new();
        for (i, step) in self.steps.iter().enumerate() {
            if step.models.is_empty() {
                return Err(format!("step {} has no model", i + 1));
            }
            let mut kinds: Option<Vec<StemKind>> = None;
            for id in &step.models {
                let m = find(id).ok_or_else(|| format!("step {}: unknown model “{id}”", i + 1))?;
                kinds = Some(match kinds {
                    None => m.stems.clone(),
                    Some(k) => k.into_iter().filter(|s| m.stems.contains(s)).collect(),
                });
            }
            let kinds = kinds.unwrap_or_default();
            if kinds.is_empty() {
                return Err(format!(
                    "step {}: the ensemble's models have no stem in common",
                    i + 1
                ));
            }
            produced.push(kinds);
            if let StepInput::Stem(k) = step.input
                && self.source_of(i, &produced).is_none()
            {
                return Err(format!(
                    "step {}: no earlier step makes a {} stem",
                    i + 1,
                    k.label().to_lowercase()
                ));
            }
        }
        let consumed: Vec<(usize, StemKind)> = (0..self.steps.len())
            .filter_map(|i| match self.steps[i].input {
                StepInput::Stem(k) => self.source_of(i, &produced).map(|j| (j, k)),
                StepInput::Mix => None,
            })
            .collect();
        let mut outputs: Vec<(usize, StemKind)> = Vec::new();
        for (i, kinds) in produced.iter().enumerate() {
            for k in kinds {
                if !consumed.contains(&(i, *k)) && !outputs.iter().any(|(_, o)| o == k) {
                    outputs.push((i, *k));
                }
            }
        }
        if outputs.is_empty() {
            return Err("the chain doesn't output any stems".into());
        }
        Ok(Plan { produced, outputs })
    }
}

/// A chain with its models resolved, ready to run.
#[derive(Debug, Clone)]
pub struct ChainSpec {
    /// Preset name, shown in progress and history.
    pub name: String,
    pub chain: Chain,
    /// Every model the chain uses.
    pub models: Vec<ModelInfo>,
    /// Use and fill the stem cache.
    pub use_cache: bool,
    /// Stem cache size limit in bytes (least recently used results go first).
    pub cache_limit: u64,
}

impl ChainSpec {
    fn find(&self, id: &str) -> Option<ModelInfo> {
        self.models.iter().find(|m| m.id == id).cloned()
    }
    pub fn plan(&self) -> Result<Plan, String> {
        self.chain.plan(&|id| self.find(id))
    }
}

/// Runs one model on one input.
pub trait Runner {
    /// Called once before the first model that isn't cached.
    fn prepare(&mut self, _emit: &dyn Fn(&str, Option<f32>)) -> Result<(), String> {
        Ok(())
    }
    /// Extra cache-key part for `model`'s runs (e.g. a non-default overlap), so results made
    /// with other settings aren't reused.
    fn cache_tag(&self, _model: &ModelInfo) -> Option<String> {
        None
    }
    /// Separate `input` with `model`. `emit(detail, fraction)` reports progress within this run.
    fn separate(
        &mut self,
        model: &ModelInfo,
        input: &AudioBuffer,
        emit: &dyn Fn(&str, Option<f32>),
        cancel: &AtomicBool,
    ) -> Result<Vec<(StemKind, AudioBuffer)>, String>;
}

/// Progress from [`execute`].
pub enum ChainEvent {
    /// `(detail, overall fraction of all model runs)`.
    Progress(String, f32),
    /// A model run came from the cache.
    CacheHit,
}

/// Run `spec` on `mix`. `input_key` identifies the input (file hash plus range); the cache keys
/// every run on it, the model id and the path through the chain.
pub fn execute(
    spec: &ChainSpec,
    mix: &AudioBuffer,
    input_key: &str,
    cache: Option<&StemCache>,
    runner: &mut dyn Runner,
    cancel: &AtomicBool,
    emit: &dyn Fn(ChainEvent),
) -> Result<Vec<(StemKind, AudioBuffer)>, String> {
    let plan = spec.plan()?;
    let chain = &spec.chain;
    let n_steps = chain.steps.len();
    let total_runs: usize = chain
        .steps
        .iter()
        .map(|s| s.models.len())
        .sum::<usize>()
        .max(1);
    let mut done_runs = 0usize;
    let mut prepared = false;
    // Per step: its stems and its cache key (the key of what it produced).
    let mut results: Vec<(Vec<(StemKind, AudioBuffer)>, String)> = Vec::new();
    for (i, step) in chain.steps.iter().enumerate() {
        check(cancel)?;
        let (input, in_key): (&AudioBuffer, String) = match step.input {
            StepInput::Mix => (mix, input_key.to_owned()),
            StepInput::Stem(k) => {
                let j = chain
                    .source_of(i, &plan.produced)
                    .ok_or("chain input is missing")?;
                let (stems, key) = &results[j];
                let buf = stems
                    .iter()
                    .find(|(s, _)| *s == k)
                    .map(|(_, b)| b)
                    .ok_or_else(|| format!("step {} produced no {}", j + 1, k.label()))?;
                (buf, cache::key(&[key, k.label()]))
            }
        };
        let mut runs: Vec<(Vec<(StemKind, AudioBuffer)>, String)> = Vec::new();
        for id in &step.models {
            let model = spec.find(id).ok_or_else(|| format!("unknown model {id}"))?;
            let run_key = match runner.cache_tag(&model) {
                Some(tag) => cache::key(&[&in_key, model.id, &tag]),
                None => cache::key(&[&in_key, model.id]),
            };
            let prefix = if n_steps > 1 {
                format!("Step {}/{n_steps} · ", i + 1)
            } else {
                String::new()
            };
            let on = match step.input {
                StepInput::Mix => String::new(),
                StepInput::Stem(k) => format!(" on {}", k.label().to_lowercase()),
            };
            let base = done_runs as f32 / total_runs as f32;
            let share = 1.0 / total_runs as f32;
            if let Some(stems) = cache.and_then(|c| c.get(&run_key)) {
                emit(ChainEvent::CacheHit);
                emit(ChainEvent::Progress(
                    format!("{prefix}{}{on} · cached", short_model_name(model.name)),
                    base + share,
                ));
                runs.push((stems, run_key));
                done_runs += 1;
                continue;
            }
            if !prepared {
                runner.prepare(&|d, _| emit(ChainEvent::Progress(d.to_owned(), base)))?;
                prepared = true;
            }
            let label = format!("{prefix}{}{on}", short_model_name(model.name));
            let stems = runner.separate(
                &model,
                input,
                &|detail, f| {
                    let d = if detail.is_empty() {
                        label.clone()
                    } else {
                        format!("{label} · {detail}")
                    };
                    emit(ChainEvent::Progress(d, base + share * f.unwrap_or(0.0)))
                },
                cancel,
            )?;
            check(cancel)?;
            let saving = if cache.is_some() { " · caching" } else { "" };
            emit(ChainEvent::Progress(
                format!("{label}{saving}"),
                base + share,
            ));
            if let Some(c) = cache
                && let Err(e) = c.put(&run_key, model.id, &stems)
            {
                tracing::warn!("stem cache: {e}");
            }
            runs.push((stems, run_key));
            done_runs += 1;
        }
        let combined = if runs.len() == 1 {
            runs.pop().unwrap_or_default()
        } else {
            emit(ChainEvent::Progress(
                format!("Combining · {}", step.ensemble.label()),
                done_runs as f32 / total_runs as f32,
            ));
            combine(&plan.produced[i], &runs, step.ensemble)?
        };
        results.push(combined);
    }
    let mut out = Vec::new();
    for (i, k) in &plan.outputs {
        if let Some((_, buf)) = results[*i].0.iter().find(|(s, _)| s == k) {
            out.push((*k, buf.clone()));
        }
    }
    Ok(out)
}

/// Ensemble the stems `kinds` across `runs`; the key is derived from the runs' keys.
fn combine(
    kinds: &[StemKind],
    runs: &[(Vec<(StemKind, AudioBuffer)>, String)],
    mode: EnsembleMode,
) -> Result<(Vec<(StemKind, AudioBuffer)>, String), String> {
    let mut stems = Vec::new();
    for k in kinds {
        let bufs: Vec<&AudioBuffer> = runs
            .iter()
            .filter_map(|(s, _)| s.iter().find(|(sk, _)| sk == k).map(|(_, b)| b))
            .collect();
        let buf = ensemble(&bufs, mode).map_err(|e| e.to_string())?;
        stems.push((*k, buf));
    }
    let mut parts: Vec<&str> = vec!["ensemble", mode.label()];
    parts.extend(runs.iter().map(|(_, k)| k.as_str()));
    Ok((stems, cache::key(&parts)))
}

// ---- the job ---------------------------------------------------------------------------------

/// The full split job for a chain: fetch, decode, run the chain (through the cache), write.
/// `range` (seconds) limits the split to a section of the track.
pub(super) fn run(
    req: &JobRequest,
    spec: &ChainSpec,
    cancel: &AtomicBool,
    send: Emit<'_>,
) -> Result<JobOutput, String> {
    let plan = spec.plan()?;
    let range = req.range;
    let (mix, title, file_key, input) = if req.demo {
        let (mix, title, key) = demo_input(req, cancel, send)?;
        (mix, title, key, None)
    } else {
        let (path, title) = super::fetch_input(req, cancel, send)?;
        check(cancel)?;
        send(JobEvent::Stage(Stage::Decoding, "Reading audio".into()));
        send(JobEvent::Progress(None));
        let mix = stem_audio::decode_file(&path).map_err(|e| e.to_string())?;
        let key = cache::file_key(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        (mix, title, key, Some(path))
    };
    check(cancel)?;
    // Only the chosen section, plus the padding the models see as context (trimmed below).
    let cut = if req.demo {
        crate::section::cut(&crate::section::slice(&mix, range), None)
    } else {
        crate::section::cut(&mix, range)
    };
    drop(mix);
    let range_key = match range {
        Some(r) => format!("{:.3}-{:.3}", r.start, r.end),
        None => "full".to_owned(),
    };
    let input_key = cache::key(&[&file_key, &range_key]);
    let cache = spec
        .use_cache
        .then(|| StemCache::new(cache::dir(&req.paths, req.demo), spec.cache_limit));

    send(JobEvent::Stage(
        Stage::PreparingModel,
        format!("{} · {} steps", spec.name, spec.chain.steps.len()),
    ));
    send(JobEvent::Progress(None));
    let mut runner: Box<dyn Runner> = if req.demo {
        Box::new(DemoRunner)
    } else {
        Box::new(OrtRunner::new(req))
    };
    let last = std::cell::RefCell::new(String::new());
    let stems = execute(
        spec,
        &cut.audio,
        &input_key,
        cache.as_ref(),
        runner.as_mut(),
        cancel,
        &|ev| {
            if let ChainEvent::Progress(detail, f) = ev {
                if *last.borrow() != detail {
                    send(JobEvent::Stage(Stage::Separating, detail.clone()));
                    *last.borrow_mut() = detail;
                }
                send(JobEvent::Progress(Some(f)));
            }
        },
    )?;
    let stems = cut.trim(stems);
    check(cancel)?;
    debug_assert_eq!(stems.len(), plan.outputs.len());
    let mut req = req.clone();
    if req.demo {
        req.output_root = std::env::temp_dir().join("stem-splitter-demo");
    }
    let mut out = write_stems(&req, &title, stems, send)?;
    out.origin.model_name = spec.name.clone();
    if input.is_some() {
        out.origin.input = input;
        out.origin.save(&out.out_dir);
    }
    Ok(out)
}

/// Demo mode: a fake download, the synthetic mix, and a key from the source text.
fn demo_input(
    req: &JobRequest,
    cancel: &AtomicBool,
    send: Emit<'_>,
) -> Result<(AudioBuffer, String, String), String> {
    if matches!(req.source, InputSource::Url(_)) {
        send(JobEvent::Stage(
            Stage::Fetching,
            "Downloading audio with yt-dlp".into(),
        ));
        for i in 0..=30 {
            check(cancel)?;
            send(JobEvent::Progress(Some(i as f32 / 30.0)));
            std::thread::sleep(Duration::from_millis(30));
        }
        send(JobEvent::Title(crate::demo::DEMO_TITLE.into()));
    }
    let title = match &req.source {
        InputSource::File(_) => req.source.provisional_title(),
        InputSource::Url(_) => crate::demo::DEMO_TITLE.to_owned(),
    };
    let stems: Vec<AudioBuffer> = [
        StemKind::Vocals,
        StemKind::Drums,
        StemKind::Bass,
        StemKind::Other,
    ]
    .iter()
    .map(|k| crate::demo::stem(*k))
    .collect();
    let mix = AudioBuffer::mix(&stems.iter().collect::<Vec<_>>());
    Ok((mix, title, cache::key(&["demo", &req.source.display()])))
}

/// Synthetic stems after a short fake run.
pub(super) struct DemoRunner;

impl Runner for DemoRunner {
    fn separate(
        &mut self,
        model: &ModelInfo,
        _input: &AudioBuffer,
        emit: &dyn Fn(&str, Option<f32>),
        cancel: &AtomicBool,
    ) -> Result<Vec<(StemKind, AudioBuffer)>, String> {
        for i in 0..=60 {
            check(cancel)?;
            emit("", Some(i as f32 / 60.0));
            std::thread::sleep(Duration::from_millis(30));
        }
        Ok(model
            .stems
            .iter()
            .map(|k| (*k, crate::demo::stem(*k)))
            .collect())
    }
}

/// ONNX Runtime: makes sure the runtime and each model are downloaded, loads and runs it.
///
/// GPU failures are remembered per (model, adapter) in [`crate::gpu_health`]: a model that failed
/// on DirectML in the last 7 days goes straight to the CPU.
pub(crate) struct OrtRunner {
    mgr: ModelManager,
    device: Device,
    quality: SplitQuality,
    /// The `gpu_health` file.
    health: PathBuf,
}

impl OrtRunner {
    fn new(req: &JobRequest) -> Self {
        Self::with(&req.paths, req.device, req.quality)
    }

    pub(crate) fn with(
        paths: &crate::paths::AppPaths,
        device: Device,
        quality: SplitQuality,
    ) -> Self {
        Self {
            mgr: ModelManager::new(paths.models.clone()),
            device,
            quality,
            health: gpu_health::file(&paths.data),
        }
    }

    /// Load the downloaded `model` at `path` and separate `input`, on the CPU when this model
    /// failed on the GPU lately, and remembering a GPU failure (`stem_models` already retried
    /// it on the CPU) for next time.
    pub(crate) fn run_loaded(
        &self,
        model: &ModelInfo,
        path: &std::path::Path,
        input: &AudioBuffer,
        emit: &dyn Fn(&str, Option<f32>),
        cancel: &AtomicBool,
    ) -> Result<Vec<(StemKind, AudioBuffer)>, String> {
        let now = crate::settings::now_unix();
        let adapter = gpu_health::adapter_for(self.device);
        let remembered = adapter
            .as_deref()
            .and_then(|a| gpu_health::lookup(&self.health, model.id, a, now));
        let device = if remembered.is_some() {
            Device::Cpu
        } else {
            self.device
        };
        emit("loading", Some(0.0));
        let options = self.quality.load_options(&model.arch);
        let _awake = crate::awake::KeepAwake::new();
        let mut lease = crate::model_cache::take(model, path, device, &options)?;
        let sep = &mut lease.sep;
        let on = Cell::new(match sep.active_device() {
            // Short: the progress line also names the step and model.
            Device::Cpu if remembered.is_some() => "CPU (GPU failed recently)",
            Device::Cpu if self.device != Device::Cpu => "CPU fallback",
            Device::Cpu => "CPU",
            _ => "",
        });
        let failure = RefCell::new(None::<String>);
        let result = sep.separate(
            input,
            &mut |p| match p {
                Progress::Separating(f) => emit(on.get(), Some(f)),
                Progress::LoadingModel => emit("loading", Some(0.0)),
                Progress::RetryingOnCpu(why) => {
                    on.set("CPU (GPU failed)");
                    emit("GPU failed, retrying on CPU", Some(0.0));
                    *failure.borrow_mut() = Some(why);
                }
                Progress::Downloading { .. } => {}
            },
            cancel,
        );
        let failure = failure.into_inner();
        // Kept loaded for the next split (queue, chain, "split again"), unless it just fell
        // back to the CPU.
        if result.is_ok() && failure.is_none() {
            crate::model_cache::park(lease);
        }
        if let (Some(reason), Some(adapter)) = (failure, adapter) {
            gpu_health::update(&self.health, now, |f| {
                f.record(GpuFailure {
                    model_id: model.id.to_owned(),
                    model_name: model.name.to_owned(),
                    adapter,
                    when: now,
                    reason,
                })
            });
        }
        result.map_err(|e| e.to_string())
    }
}

fn pct(f: Option<f32>) -> String {
    f.map_or(String::new(), |f| format!(" {:.0}%", f * 100.0))
}

impl Runner for OrtRunner {
    fn prepare(&mut self, emit: &dyn Fn(&str, Option<f32>)) -> Result<(), String> {
        emit("Starting ONNX Runtime", None);
        self.mgr
            .ensure_runtime(&mut |p| {
                if let Progress::Downloading { fraction } = p {
                    emit(&format!("Downloading ONNX Runtime{}", pct(fraction)), None);
                }
            })
            .map_err(|e| e.to_string())
    }

    fn cache_tag(&self, model: &ModelInfo) -> Option<String> {
        self.quality.cache_tag(&model.arch)
    }

    fn separate(
        &mut self,
        model: &ModelInfo,
        input: &AudioBuffer,
        emit: &dyn Fn(&str, Option<f32>),
        cancel: &AtomicBool,
    ) -> Result<Vec<(StemKind, AudioBuffer)>, String> {
        let path = if self.mgr.is_downloaded(model) {
            self.mgr.model_path(model)
        } else {
            self.mgr
                .download(
                    model,
                    &mut |p| {
                        if let Progress::Downloading { fraction } = p {
                            emit(&format!("downloading{}", pct(fraction)), Some(0.0));
                        }
                    },
                    cancel,
                )
                .map_err(|e| e.to_string())?
        };
        check(cancel)?;
        self.run_loaded(model, &path, input, emit, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn model(id: &'static str, stems: &[StemKind]) -> ModelInfo {
        let mut m = stem_models::find_model("htdemucs").expect("catalog has htdemucs");
        m.id = id;
        m.name = id;
        m.stems = stems.to_vec();
        m
    }

    use StemKind::*;

    fn models() -> Vec<ModelInfo> {
        vec![
            model("voc-a", &[Vocals, Instrumental]),
            model("voc-b", &[Instrumental, Vocals]),
            model("four", &[Drums, Bass, Other, Vocals]),
            model("karaoke", &[BackingVocals, LeadVocals]),
        ]
    }

    fn spec(chain: Chain) -> ChainSpec {
        ChainSpec {
            name: "test".into(),
            chain,
            models: models(),
            use_cache: true,
            cache_limit: u64::MAX,
        }
    }

    fn beatmaker() -> Chain {
        Chain {
            steps: vec![
                Step::new(StepInput::Mix, "voc-a"),
                Step::new(StepInput::Stem(Instrumental), "four"),
            ],
        }
    }

    #[test]
    fn plans_outputs_and_rejects_bad_chains() {
        let s = spec(beatmaker());
        assert_eq!(s.plan().unwrap().kinds(), vec![Vocals, Drums, Bass, Other]);

        let ens = spec(Chain {
            steps: vec![Step {
                input: StepInput::Mix,
                models: vec!["voc-a".into(), "voc-b".into()],
                ensemble: EnsembleMode::MaxSpec,
            }],
        });
        assert_eq!(ens.plan().unwrap().kinds(), vec![Vocals, Instrumental]);

        let vocal_chain = spec(Chain {
            steps: vec![
                Step::new(StepInput::Mix, "voc-a"),
                Step::new(StepInput::Stem(Vocals), "karaoke"),
            ],
        });
        assert_eq!(
            vocal_chain.plan().unwrap().kinds(),
            vec![Instrumental, BackingVocals, LeadVocals]
        );

        let bad = |c: Chain| spec(c).plan().unwrap_err();
        assert!(bad(Chain::default()).contains("at least one"));
        assert!(bad(Chain::single("nope")).contains("unknown model"));
        assert!(
            bad(Chain {
                steps: vec![Step::new(StepInput::Stem(Drums), "four")]
            })
            .contains("no earlier step")
        );
        assert!(
            bad(Chain {
                steps: vec![Step {
                    input: StepInput::Mix,
                    models: vec!["voc-a".into(), "karaoke".into()],
                    ensemble: EnsembleMode::Average,
                }]
            })
            .contains("in common")
        );
    }

    /// Records runs; every stem is a constant-valued buffer tagged by model and input level.
    struct FakeRunner {
        runs: RefCell<Vec<String>>,
    }

    impl Runner for FakeRunner {
        fn separate(
            &mut self,
            model: &ModelInfo,
            input: &AudioBuffer,
            emit: &dyn Fn(&str, Option<f32>),
            _cancel: &AtomicBool,
        ) -> Result<Vec<(StemKind, AudioBuffer)>, String> {
            self.runs.borrow_mut().push(model.id.to_owned());
            emit("", Some(0.5));
            let level = input.channels[0][0];
            let tag = match model.id {
                "voc-a" => 0.1,
                "voc-b" => 0.3,
                _ => 0.01,
            };
            Ok(model
                .stems
                .iter()
                .enumerate()
                .map(|(i, k)| {
                    let v = level * 0.5 + tag + i as f32 * 0.001;
                    (*k, AudioBuffer::new(44_100, vec![vec![v; 64]; 2]))
                })
                .collect())
        }
    }

    fn temp_cache(name: &str) -> StemCache {
        let dir = std::env::temp_dir().join(format!("stem-chain-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        StemCache::new(dir, u64::MAX)
    }

    #[test]
    fn runs_steps_in_order_and_feeds_the_right_stem() {
        let mix = AudioBuffer::new(44_100, vec![vec![1.0; 64]; 2]);
        let mut r = FakeRunner {
            runs: RefCell::new(vec![]),
        };
        let never = AtomicBool::new(false);
        let last = RefCell::new(0.0f32);
        let out = execute(&spec(beatmaker()), &mix, "k", None, &mut r, &never, &|e| {
            if let ChainEvent::Progress(_, f) = e {
                assert!(f + 1e-6 >= *last.borrow(), "progress went backwards");
                *last.borrow_mut() = f;
            }
        })
        .unwrap();
        assert_eq!(*r.runs.borrow(), vec!["voc-a", "four"]);
        let kinds: Vec<_> = out.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, vec![Vocals, Drums, Bass, Other]);
        // Vocals come from step 1 (mix level 1.0): 0.5 + 0.1.
        assert!((out[0].1.channels[0][0] - 0.6).abs() < 1e-6);
        // Step 2 ran on step 1's instrumental (0.5 + 0.1 + 0.001 = 0.601).
        let drums = out[1].1.channels[0][0];
        assert!((drums - (0.601 * 0.5 + 0.01)).abs() < 1e-6, "{drums}");
        assert!((*last.borrow() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn ensembles_average_shared_stems() {
        let mix = AudioBuffer::new(44_100, vec![vec![1.0; 64]; 2]);
        let mut r = FakeRunner {
            runs: RefCell::new(vec![]),
        };
        let s = spec(Chain {
            steps: vec![Step {
                input: StepInput::Mix,
                models: vec!["voc-a".into(), "voc-b".into()],
                ensemble: EnsembleMode::Average,
            }],
        });
        let never = AtomicBool::new(false);
        let out = execute(&s, &mix, "k", None, &mut r, &never, &|_| {}).unwrap();
        assert_eq!(out.len(), 2);
        // voc-a vocals = 0.6 (index 0); voc-b vocals = 0.5 + 0.3 + 0.001 (index 1).
        let want = (0.6 + 0.801) / 2.0;
        assert!((out[0].1.channels[0][0] - want).abs() < 1e-5);
    }

    #[test]
    fn cache_skips_model_runs_it_has_seen() {
        let mix = AudioBuffer::new(44_100, vec![vec![1.0; 64]; 2]);
        let cache = temp_cache("hits");
        let never = AtomicBool::new(false);
        let mut r = FakeRunner {
            runs: RefCell::new(vec![]),
        };
        let first = execute(
            &spec(beatmaker()),
            &mix,
            "song",
            Some(&cache),
            &mut r,
            &never,
            &|_| {},
        )
        .unwrap();
        assert_eq!(r.runs.borrow().len(), 2);
        // Same chain again: nothing runs, same output.
        let hits = RefCell::new(0);
        let again = execute(
            &spec(beatmaker()),
            &mix,
            "song",
            Some(&cache),
            &mut r,
            &never,
            &|e| {
                if matches!(e, ChainEvent::CacheHit) {
                    *hits.borrow_mut() += 1;
                }
            },
        )
        .unwrap();
        assert_eq!(r.runs.borrow().len(), 2);
        assert_eq!(*hits.borrow(), 2);
        for ((k1, a), (k2, b)) in first.iter().zip(&again) {
            assert_eq!(k1, k2);
            assert_eq!(a.channels, b.channels);
        }
        // A chain sharing step 1 only re-runs step 2.
        let other = Chain {
            steps: vec![
                Step::new(StepInput::Mix, "voc-a"),
                Step::new(StepInput::Stem(Vocals), "karaoke"),
            ],
        };
        execute(
            &spec(other),
            &mix,
            "song",
            Some(&cache),
            &mut r,
            &never,
            &|_| {},
        )
        .unwrap();
        assert_eq!(*r.runs.borrow(), vec!["voc-a", "four", "karaoke"]);
        // A different input runs everything.
        execute(
            &spec(beatmaker()),
            &mix,
            "other song",
            Some(&cache),
            &mut r,
            &never,
            &|_| {},
        )
        .unwrap();
        assert_eq!(r.runs.borrow().len(), 5);
        let _ = std::fs::remove_dir_all(cache.root());
    }

    /// The whole demo job (as `STEM_DEMO=1` runs it): writes the stems, and a second run of the
    /// same preset on the same input comes from the cache.
    #[test]
    fn demo_job_writes_stems_and_reuses_the_cache() {
        let tmp = std::env::temp_dir().join(format!("stem-chain-job-{}", std::process::id()));
        let paths = crate::paths::AppPaths {
            models: tmp.clone(),
            tools: tmp.join("tools"),
            downloads: tmp.join("downloads"),
            data: tmp.clone(),
        };
        let find = |id: &str| stem_models::find_model(id).unwrap();
        let spec = ChainSpec {
            name: "Beatmaker".into(),
            chain: Chain {
                steps: vec![
                    Step::new(StepInput::Mix, "mel-roformer-kim"),
                    Step::new(StepInput::Stem(Instrumental), "htdemucs"),
                ],
            },
            models: vec![find("mel-roformer-kim"), find("htdemucs")],
            use_cache: true,
            cache_limit: u64::MAX,
        };
        // A unique source, so the (shared) demo cache starts cold for this test.
        let source = InputSource::File(tmp.join(format!("song {}.wav", std::process::id())));
        let req = JobRequest {
            id: 1,
            source,
            model: find("mel-roformer-kim"),
            device: Device::Cpu,
            output_root: tmp.join("out"),
            wav: stem_audio::WavFormat::Pcm16,
            cookies_from_browser: None,
            paths,
            demo: true,
            range: None,
            title: None,
            chain: Some(spec.clone()),
            quality: Default::default(),
        };
        let never = AtomicBool::new(false);
        let details = RefCell::new(Vec::<String>::new());
        let send = |ev| {
            if let JobEvent::Stage(_, d) = ev {
                details.borrow_mut().push(d);
            }
        };
        let t_first = std::time::Instant::now();
        let out = run(&req, &spec, &never, &send).unwrap();
        let first_secs = t_first.elapsed().as_secs_f32();
        let kinds: Vec<_> = out.stems.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, vec![Vocals, Drums, Bass, Other]);
        for s in &out.stems {
            assert!(s.path.is_file());
            assert!(
                s.path
                    .ends_with(format!("{}.wav", s.kind.label().to_lowercase()))
            );
        }
        assert!(!details.borrow().iter().any(|d| d.contains("cached")));
        details.borrow_mut().clear();
        let t0 = std::time::Instant::now();
        let again = run(&req, &spec, &never, &send).unwrap();
        assert_eq!(again.stems.len(), 4);
        let cached = details
            .borrow()
            .iter()
            .filter(|d| d.contains("cached"))
            .count();
        assert_eq!(cached, 2, "{:?}", details.borrow());
        // The fake model runs take ~4 s; cached ones skip them. Compare against the first run
        // rather than a fixed limit: both runs also do the track analysis, which is slow in
        // debug builds on CI runners.
        let second_secs = t0.elapsed().as_secs_f32();
        assert!(
            second_secs < first_secs - 2.0,
            "first {first_secs:.2} s, cached {second_secs:.2} s"
        );
        let _ = std::fs::remove_dir_all(&tmp);
        for o in [out.out_dir, again.out_dir] {
            let _ = std::fs::remove_dir_all(o);
        }
        // Drop this test's two entries from the shared demo cache.
        let input = cache::key(&[&cache::key(&["demo", &req.source.display()]), "full"]);
        let first = cache::key(&[&input, "mel-roformer-kim"]);
        let second = cache::key(&[&cache::key(&[&first, "Instrumental"]), "htdemucs"]);
        let dir = cache::dir(&req.paths, true);
        for k in [first, second] {
            assert!(dir.join(&k).is_dir());
            let _ = std::fs::remove_dir_all(dir.join(k));
        }
    }

    #[test]
    fn cancel_stops_before_the_next_step() {
        let mix = AudioBuffer::new(44_100, vec![vec![1.0; 64]; 2]);
        let cancel = AtomicBool::new(true);
        let mut r = FakeRunner {
            runs: RefCell::new(vec![]),
        };
        assert!(
            execute(
                &spec(beatmaker()),
                &mix,
                "k",
                None,
                &mut r,
                &cancel,
                &|_| {}
            )
            .is_err()
        );
        assert!(r.runs.borrow().is_empty());
    }

    #[test]
    fn short_names() {
        assert_eq!(
            short_model_name("Mel-RoFormer Vocals (Kim)"),
            "Mel-RoFormer"
        );
        assert_eq!(short_model_name("HTDemucs (4 stems)"), "HTDemucs");
        assert_eq!(short_model_name("MDX-Net Kim Vocal 2"), "MDX Kim Vocal 2");
        assert_eq!(short_model_name("SCNet (4 stems, fast)"), "SCNet");
    }

    #[test]
    fn chain_serializes() {
        let c = beatmaker();
        let json = serde_json::to_string(&c).unwrap();
        let back: Chain = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
        // `ensemble` may be missing.
        let s: Step = serde_json::from_str(r#"{"input":"Mix","models":["a"]}"#).unwrap();
        assert_eq!(s.ensemble, EnsembleMode::Average);
    }
}
