//! User settings and job history, persisted through eframe's storage as JSON.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use stem_audio::WavFormat;
use stem_models::{Device, StemKind};

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

/// Browsers yt-dlp can borrow cookies from. Firefox first: on Windows, Chromium-based browsers
/// lock and encrypt their cookie store, so yt-dlp usually can't read it while they run.
pub const COOKIE_BROWSERS: [&str; 8] = [
    "firefox", "chrome", "edge", "brave", "opera", "vivaldi", "chromium", "safari",
];

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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_dir: crate::paths::default_output_dir(),
            wav: WavChoice::default(),
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
    }
}
