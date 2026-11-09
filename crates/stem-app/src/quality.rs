//! "Split quality: Fast / Balanced / Best": how much neighbouring model chunks overlap.
//!
//! Every separator cuts the track into chunks and cross-fades them back together; a larger
//! overlap averages more predictions per sample (fewer seams and chunk-edge artefacts, a
//! fraction of a dB SDR) and costs `(1 - default) / (1 - overlap)` times the model time. Idea
//! from UVR's segment / overlap controls; the values are UVR's and Music-Source-Separation-
//! Training's own choices:
//!
//! | family     | Fast (model default)       | Balanced                 | Best                      |
//! |------------|----------------------------|--------------------------|---------------------------|
//! | MDX-Net    | 0.25 (UVR default)         | 0.5 (UVR option)         | 0.75 (UVR option)         |
//! | HTDemucs   | 0.25 (demucs `--overlap`)  | 0.5                      | 0.75                      |
//! | RoFormer   | catalog (0.5 = `num_overlap` 2) | 0.75 (`num_overlap` 4, the BS-RoFormer config) | 0.875 (`num_overlap` 8) |
//! | MDX23C     | catalog (0.5)              | 0.75 (`num_overlap` 4)   | 0.875 (`num_overlap` 8)   |
//! | SCNet      | 0.75 (reference step 0.25) | 0.75 (already there)     | 0.875                     |
//!
//! A level never lowers a model's default overlap.

use serde::{Deserialize, Serialize};
use stem_models::ModelArch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SplitQuality {
    /// Each model's own default overlap.
    #[default]
    Fast,
    Balanced,
    Best,
}

/// The overlap `stem_models` uses for MDX-Net and HTDemucs when `LoadOptions::overlap` is
/// `None` (`DEFAULT_OVERLAP` in `mdx.rs` / `demucs.rs`).
const MDX_DEMUCS_DEFAULT: f32 = 0.25;

impl SplitQuality {
    pub const ALL: [SplitQuality; 3] = [
        SplitQuality::Fast,
        SplitQuality::Balanced,
        SplitQuality::Best,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SplitQuality::Fast => "Fast",
            SplitQuality::Balanced => "Balanced",
            SplitQuality::Best => "Best",
        }
    }

    /// The overlap a model of `arch` runs with when `LoadOptions::overlap` is `None`.
    pub fn default_overlap(arch: &ModelArch) -> f32 {
        match *arch {
            ModelArch::MdxNet { .. } | ModelArch::HtDemucs { .. } => MDX_DEMUCS_DEFAULT,
            ModelArch::RoFormer { overlap, .. }
            | ModelArch::Scnet { overlap, .. }
            | ModelArch::Mdx23c { overlap, .. } => overlap,
        }
    }

    /// Target overlap for this level (before the "never below the default" rule).
    fn target(self, arch: &ModelArch) -> Option<f32> {
        let (balanced, best) = match arch {
            ModelArch::MdxNet { .. } | ModelArch::HtDemucs { .. } => (0.5, 0.75),
            ModelArch::RoFormer { .. } | ModelArch::Mdx23c { .. } | ModelArch::Scnet { .. } => {
                (0.75, 0.875)
            }
        };
        match self {
            SplitQuality::Fast => None,
            SplitQuality::Balanced => Some(balanced),
            SplitQuality::Best => Some(best),
        }
    }

    /// What to put in `LoadOptions::overlap`: `None` when the model's default already is the
    /// level's overlap (so Fast, and Balanced on SCNet, keep the cache keys they always had).
    pub fn overlap(self, arch: &ModelArch) -> Option<f32> {
        let default = Self::default_overlap(arch);
        self.target(arch)
            .map(|t| t.max(default))
            .filter(|o| (o - default).abs() > 1e-4)
    }

    /// Model time relative to Fast (1.0 = no slower).
    pub fn cost(self, arch: &ModelArch) -> f64 {
        let default = Self::default_overlap(arch) as f64;
        match self.overlap(arch) {
            Some(o) => (1.0 - default) / (1.0 - o as f64).max(0.05),
            None => 1.0,
        }
    }

    /// `stem_models::LoadOptions` for a model of `arch` at this level.
    pub fn load_options(self, arch: &ModelArch) -> stem_models::LoadOptions {
        stem_models::LoadOptions {
            overlap: self.overlap(arch),
            ..Default::default()
        }
    }

    /// Extra cache-key part for a run at this level (`None` = the model's default output).
    pub fn cache_tag(self, arch: &ModelArch) -> Option<String> {
        self.overlap(arch).map(|o| format!("overlap {o:.3}"))
    }
}

/// Total physical memory in bytes, where it's cheap to ask (Windows, Linux); `None` elsewhere.
pub fn total_ram_bytes() -> Option<u64> {
    static RAM: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *RAM.get_or_init(read_total_ram)
}

#[cfg(windows)]
fn read_total_ram() -> Option<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut st = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `st` is a properly sized, initialised MEMORYSTATUSEX with dwLength set.
    unsafe { GlobalMemoryStatusEx(&mut st) }.ok()?;
    Some(st.ullTotalPhys)
}

#[cfg(target_os = "linux")]
fn read_total_ram() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_meminfo_total(&info)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn read_total_ram() -> Option<u64> {
    None
}

/// `MemTotal:  16314252 kB` -> bytes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_meminfo_total(info: &str) -> Option<u64> {
    let line = info.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// Below this much RAM, heavy RoFormer models get a note in the model list. 16 GB machines
/// report a little under 16 GiB, so this flags 8 and 12 GB machines only.
pub const LOW_RAM_BYTES: u64 = 13 * 1024 * 1024 * 1024;

/// A short RAM note for heavy (RoFormer) models when this PC has little memory.
pub fn ram_note(arch: &ModelArch, total_ram: Option<u64>) -> Option<String> {
    let ram = total_ram?;
    if !matches!(arch, ModelArch::RoFormer { .. }) || ram >= LOW_RAM_BYTES {
        return None;
    }
    let gb = (ram as f64 / (1024.0 * 1024.0 * 1024.0)).round();
    Some(format!("Heavy for {gb:.0} GB RAM"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arches() -> Vec<ModelArch> {
        ["mdx-kim-vocal-2", "htdemucs", "mel-roformer-kim", "scnet"]
            .iter()
            .map(|id| stem_models::find_model(id).expect(id).arch)
            .collect()
    }

    #[test]
    fn fast_is_the_model_default_and_levels_only_add_overlap() {
        for arch in arches() {
            assert_eq!(SplitQuality::Fast.overlap(&arch), None);
            assert_eq!(SplitQuality::Fast.cost(&arch), 1.0);
            assert_eq!(SplitQuality::Fast.load_options(&arch), Default::default());
            let def = SplitQuality::default_overlap(&arch);
            let mut last = 1.0;
            for q in SplitQuality::ALL {
                if let Some(o) = q.overlap(&arch) {
                    assert!(o > def && o <= 0.95, "{q:?} {arch:?}");
                }
                assert!(q.cost(&arch) >= last, "{q:?} {arch:?}");
                last = q.cost(&arch);
            }
        }
    }

    #[test]
    fn per_family_values() {
        let a = arches();
        let (mdx, demucs, mel, scnet) = (&a[0], &a[1], &a[2], &a[3]);
        assert_eq!(SplitQuality::Balanced.overlap(mdx), Some(0.5));
        assert_eq!(SplitQuality::Best.overlap(mdx), Some(0.75));
        assert_eq!(SplitQuality::Best.cost(mdx), 3.0);
        assert_eq!(SplitQuality::Balanced.overlap(demucs), Some(0.5));
        assert_eq!(SplitQuality::Balanced.overlap(mel), Some(0.75));
        assert_eq!(SplitQuality::Best.overlap(mel), Some(0.875));
        assert_eq!(SplitQuality::Best.cost(mel), 4.0);
        // SCNet already runs at 0.75.
        assert_eq!(SplitQuality::Balanced.overlap(scnet), None);
        assert_eq!(
            SplitQuality::Best.cache_tag(scnet).unwrap(),
            "overlap 0.875"
        );
    }

    #[test]
    fn settings_default_is_fast() {
        let s: crate::settings::Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.split_quality, SplitQuality::Fast);
    }

    #[test]
    fn ram_note_only_for_roformers_on_small_machines() {
        let a = arches();
        let gib = 1024 * 1024 * 1024;
        assert_eq!(
            ram_note(&a[2], Some(8 * gib)).as_deref(),
            Some("Heavy for 8 GB RAM")
        );
        assert_eq!(ram_note(&a[2], Some(16 * gib - gib / 2)), None);
        assert_eq!(ram_note(&a[2], None), None);
        assert_eq!(ram_note(&a[1], Some(8 * gib)), None);
        assert_eq!(
            parse_meminfo_total("MemTotal:       16314252 kB\nMemFree: 1 kB\n"),
            Some(16_314_252 * 1024)
        );
    }
}
