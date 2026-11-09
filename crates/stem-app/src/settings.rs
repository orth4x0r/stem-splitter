//! User settings and job history, persisted through eframe's storage as JSON.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use stem_audio::ensemble::EnsembleMode;
use stem_audio::{AudioFormat, WavFormat};
use stem_models::{Device, StemKind};

use crate::pipeline::chain::{Chain, Step, StepInput};

const KEY: &str = "stem-splitter/settings";
const MAX_HISTORY: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WavChoice {
    Pcm16,
    #[default]
    Pcm24,
    Float32,
}

impl WavChoice {
    pub const ALL: [WavChoice; 3] = [WavChoice::Pcm16, WavChoice::Pcm24, WavChoice::Float32];
    pub fn label(self) -> &'static str {
        match self {
            WavChoice::Pcm16 => "16-bit",
            WavChoice::Pcm24 => "24-bit",
            WavChoice::Float32 => "32-bit float",
        }
    }
    pub fn format(self) -> WavFormat {
        match self {
            WavChoice::Pcm16 => WavFormat::Pcm16,
            WavChoice::Pcm24 => WavFormat::Pcm24,
            WavChoice::Float32 => WavFormat::Float32,
        }
    }
}

/// Format for exported stems and samples (the library's own stem files stay WAV).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExportFormat {
    Wav16,
    #[default]
    Wav24,
    Wav32f,
    Flac16,
    Flac24,
    Mp3_320,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 6] = [
        ExportFormat::Wav16,
        ExportFormat::Wav24,
        ExportFormat::Wav32f,
        ExportFormat::Flac16,
        ExportFormat::Flac24,
        ExportFormat::Mp3_320,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Wav16 => "WAV 16-bit",
            ExportFormat::Wav24 => "WAV 24-bit",
            ExportFormat::Wav32f => "WAV 32-bit float",
            ExportFormat::Flac16 => "FLAC 16-bit",
            ExportFormat::Flac24 => "FLAC 24-bit",
            ExportFormat::Mp3_320 => "MP3 320 kbps",
        }
    }
    pub fn audio_format(self) -> AudioFormat {
        match self {
            ExportFormat::Wav16 => AudioFormat::Wav(WavFormat::Pcm16),
            ExportFormat::Wav24 => AudioFormat::Wav(WavFormat::Pcm24),
            ExportFormat::Wav32f => AudioFormat::Wav(WavFormat::Float32),
            ExportFormat::Flac16 => AudioFormat::Flac { bits: 16 },
            ExportFormat::Flac24 => AudioFormat::Flac { bits: 24 },
            ExportFormat::Mp3_320 => AudioFormat::Mp3 { kbps: 320 },
        }
    }
}

/// Browsers yt-dlp can borrow cookies from. Firefox first: on Windows, Chromium-based browsers
/// lock and encrypt their cookie store, so yt-dlp usually can't read it while they run.
pub const COOKIE_BROWSERS: [&str; 8] = [
    "firefox", "chrome", "edge", "brave", "opera", "vivaldi", "chromium", "safari",
];

/// A named recipe: one model, a chain of models, or an ensemble. Models are referenced by id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    /// Built-ins start with `builtin:`, user presets with `user:`.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub chain: Chain,
}

impl Preset {
    pub fn is_builtin(&self) -> bool {
        self.id.starts_with("builtin:")
    }
}

/// The presets that ship with the app.
pub fn builtin_presets() -> Vec<Preset> {
    let p = |id: &str, name: &str, description: &str, steps: Vec<Step>| Preset {
        id: format!("builtin:{id}"),
        name: name.to_owned(),
        description: description.to_owned(),
        chain: Chain { steps },
    };
    vec![
        p(
            "fast-4",
            "Fast 4-stem",
            "SCNet: vocals, drums, bass and other in seconds, even on the CPU.",
            vec![Step::new(StepInput::Mix, "scnet")],
        ),
        p(
            "best-vocals",
            "Best vocals",
            "Mel-RoFormer (Kim): the cleanest acapella and instrumental.",
            vec![Step::new(StepInput::Mix, "mel-roformer-kim")],
        ),
        p(
            "beatmaker-4",
            "Beatmaker 4-stem",
            "RoFormer takes the vocal, HTDemucs splits the rest into drums, bass and other.",
            vec![
                Step::new(StepInput::Mix, "mel-roformer-kim"),
                Step::new(StepInput::Stem(StemKind::Instrumental), "htdemucs"),
            ],
        ),
        p(
            "ensemble-vocals",
            "Ensemble vocals",
            "Two vocal models averaged, UVR-style: fewer artefacts, twice the time.",
            vec![Step {
                input: StepInput::Mix,
                models: vec!["mel-roformer-kim".into(), "mdx-kim-vocal-2".into()],
                ensemble: EnsembleMode::Average,
            }],
        ),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub title: String,
    pub source: String,
    pub model_name: String,
    pub stems: Vec<StemKind>,
    pub out_dir: PathBuf,
    /// Unix seconds.
    pub when: u64,
    pub duration_secs: f64,
    /// Tempo as last shown (after any halve / double).
    #[serde(default)]
    pub bpm: Option<f32>,
    /// "A minor".
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub output_dir: PathBuf,
    pub wav: WavChoice,
    /// Exported stems and samples.
    pub export_format: ExportFormat,
    /// File names for "Export stems", e.g. "{title} - {stem}" (see `export::render_name`).
    pub export_template: String,
    /// "Export stems" also writes "all but X" mixes.
    pub export_without: bool,
    pub cookies_from_browser: Option<String>,
    pub model_id: Option<String>,
    pub device: Device,
    /// Exported samples: normalise to -0.1 dBFS.
    pub normalize_samples: bool,
    /// Exported samples: fade length in milliseconds.
    pub sample_fade_ms: f32,
    /// Loop the selected region during playback.
    pub loop_region: bool,
    /// Snap selection edges to the beat grid.
    pub snap: crate::analysis::SnapMode,
    /// Exported samples: trim selections that are nearly N bars to exactly N bars.
    pub export_loop_bars: bool,
    pub history: Vec<HistoryEntry>,
    /// Turn off ambient drift, springs, entrances, confetti and the launch intro.
    pub reduce_motion: bool,
    /// Play the short logo intro on launch.
    pub intro: bool,
    /// Open the section picker after download, before separating.
    pub pick_section: bool,
    /// The preset to split with; `None` = the single model in `model_id`.
    pub preset_id: Option<String>,
    /// Presets the user saved (built-ins come from [`builtin_presets`]).
    pub presets: Vec<Preset>,
    /// Reuse model output for inputs that were split before.
    pub stem_cache: bool,
    /// Stem cache size limit in GB.
    pub stem_cache_gb: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_dir: crate::paths::default_output_dir(),
            wav: WavChoice::default(),
            export_format: ExportFormat::default(),
            export_template: crate::export::DEFAULT_TEMPLATE.to_owned(),
            export_without: false,
            cookies_from_browser: None,
            model_id: None,
            device: Device::Auto,
            normalize_samples: true,
            sample_fade_ms: 6.0,
            loop_region: true,
            snap: crate::analysis::SnapMode::default(),
            export_loop_bars: true,
            history: Vec::new(),
            reduce_motion: false,
            intro: true,
            pick_section: false,
            preset_id: None,
            presets: Vec::new(),
            stem_cache: true,
            stem_cache_gb: 8.0,
        }
    }
}

impl Settings {
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        storage
            .and_then(|s| s.get_string(KEY))
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        if let Ok(json) = serde_json::to_string(self) {
            storage.set_string(KEY, json);
        }
    }

    /// Built-in presets followed by the user's.
    pub fn all_presets(&self) -> Vec<Preset> {
        let mut all = builtin_presets();
        all.extend(self.presets.iter().cloned());
        all
    }

    pub fn find_preset(&self, id: &str) -> Option<Preset> {
        self.all_presets().into_iter().find(|p| p.id == id)
    }

    /// The preset in use, if any (a deleted one counts as none).
    pub fn active_preset(&self) -> Option<Preset> {
        self.preset_id
            .as_deref()
            .and_then(|id| self.find_preset(id))
    }

    /// Insert or replace a user preset (matched by id); returns its id.
    pub fn save_preset(&mut self, mut preset: Preset) -> String {
        if preset.id.is_empty() || preset.is_builtin() {
            let n = self
                .presets
                .iter()
                .filter_map(|p| p.id.strip_prefix("user:")?.parse::<u32>().ok())
                .max()
                .unwrap_or(0);
            preset.id = format!("user:{}", n + 1);
        }
        let id = preset.id.clone();
        match self.presets.iter_mut().find(|p| p.id == id) {
            Some(p) => *p = preset,
            None => self.presets.push(preset),
        }
        id
    }

    pub fn delete_preset(&mut self, id: &str) {
        self.presets.retain(|p| p.id != id);
        if self.preset_id.as_deref() == Some(id) {
            self.preset_id = None;
        }
    }

    /// Stem cache limit in bytes.
    pub fn stem_cache_limit(&self) -> u64 {
        (self.stem_cache_gb.max(0.5) as f64 * 1e9) as u64
    }

    pub fn push_history(&mut self, entry: HistoryEntry) {
        self.history.retain(|h| h.out_dir != entry.out_dir);
        self.history.insert(0, entry);
        self.history.truncate(MAX_HISTORY);
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// "just now", "5 min ago", "3 h ago", "2 days ago".
pub fn relative_time(when: u64) -> String {
    let d = now_unix().saturating_sub(when);
    match d {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", d / 60),
        3600..86400 => format!("{} h ago", d / 3600),
        86400..172800 => "yesterday".into(),
        _ => format!("{} days ago", d / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s = Settings {
            cookies_from_browser: Some("firefox".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.cookies_from_browser.as_deref(), Some("firefox"));
        // Missing fields fall back to defaults.
        let partial: Settings = serde_json::from_str("{\"wav\":\"Pcm16\"}").unwrap();
        assert_eq!(partial.wav, WavChoice::Pcm16);
        assert!(partial.normalize_samples);
        assert_eq!(partial.export_format, ExportFormat::Wav24);
        assert_eq!(partial.export_template, crate::export::DEFAULT_TEMPLATE);
        assert!(partial.stem_cache);
        assert!(partial.preset_id.is_none());
    }

    #[test]
    fn builtin_presets_use_real_models() {
        let find = |id: &str| stem_models::find_model(id);
        let all = builtin_presets();
        assert_eq!(all.len(), 4);
        for p in &all {
            assert!(p.is_builtin());
            let plan = p
                .chain
                .plan(&find)
                .unwrap_or_else(|e| panic!("{}: {e}", p.name));
            assert!(!plan.outputs.is_empty());
        }
        let beat = all.iter().find(|p| p.name == "Beatmaker 4-stem").unwrap();
        assert_eq!(
            beat.chain.plan(&find).unwrap().kinds(),
            vec![
                StemKind::Vocals,
                StemKind::Drums,
                StemKind::Bass,
                StemKind::Other
            ]
        );
    }

    #[test]
    fn user_presets_save_replace_and_delete() {
        let mut s = Settings::default();
        let mut p = builtin_presets().remove(2);
        p.name = "My beat chain".into();
        // Saving a copy of a built-in makes a new user preset.
        let id = s.save_preset(p.clone());
        assert_eq!(id, "user:1");
        assert_eq!(s.all_presets().len(), 5);
        let mut edited = s.find_preset(&id).unwrap();
        edited.name = "Renamed".into();
        assert_eq!(s.save_preset(edited), id);
        assert_eq!(s.presets.len(), 1);
        assert_eq!(s.presets[0].name, "Renamed");
        assert_eq!(s.save_preset(p), "user:2");
        s.preset_id = Some(id.clone());
        assert_eq!(s.active_preset().unwrap().name, "Renamed");
        s.delete_preset(&id);
        assert!(s.preset_id.is_none());
        assert_eq!(s.presets.len(), 1);
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.presets, s.presets);
    }
}
