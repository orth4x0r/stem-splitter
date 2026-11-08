//! Thin layer over the backend crates.
//!
//! The UI never calls the backend crates directly for cheap queries; it goes through
//! [`Backend`], which caches results, survives unimplemented (`todo!()`) backends during
//! development, and serves synthetic data in demo mode.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use stem_models::{Device, ModelArch, ModelInfo, ModelManager, StemKind};

use crate::paths::AppPaths;

/// Run `f`, turning a panic (e.g. an unimplemented backend) into an error string.
pub fn guard<T>(what: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|e| {
        let msg = e
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "unknown error".into());
        tracing::error!("{what} panicked: {msg}");
        if msg.starts_with("not yet implemented") {
            format!("{what}: not implemented yet ({msg})")
        } else {
            format!("{what}: {msg}")
        }
    })
}

pub struct Backend {
    pub demo: bool,
    pub paths: AppPaths,
    pub models: Vec<ModelInfo>,
    pub devices: Vec<(Device, String)>,
    downloaded: HashMap<&'static str, bool>,
    /// Set when the backend could not be queried (shown as a banner).
    pub problem: Option<String>,
}

impl Backend {
    pub fn new(demo: bool, paths: AppPaths) -> Self {
        let mut problem = None;
        let (models, devices) = if demo {
            (demo_registry(), demo_devices())
        } else {
            let models = guard("model registry", stem_models::registry).unwrap_or_else(|e| {
                problem = Some(e);
                Vec::new()
            });
            let devices = guard("device list", stem_models::list_devices)
                .unwrap_or_else(|_| vec![(Device::Cpu, "CPU".to_owned())]);
            (models, devices)
        };
        let mut devices = devices;
        if !devices.iter().any(|(d, _)| *d == Device::Auto) {
            devices.insert(0, (Device::Auto, "Auto (best available)".to_owned()));
        }
        let mut me = Self {
            demo,
            paths,
            models,
            devices,
            downloaded: HashMap::new(),
            problem,
        };
        me.refresh_downloaded();
        me
    }

    pub fn manager(&self) -> ModelManager {
        ModelManager::new(self.paths.models.clone())
    }

    pub fn refresh_downloaded(&mut self) {
        let mgr = self.manager();
        for m in &self.models {
            let ok = if self.demo {
                // Pretend the small model is already there.
                m.id == "mdx-inst-hq3" || self.downloaded.get(m.id).copied().unwrap_or(false)
            } else {
                guard("model check", || mgr.is_downloaded(m)).unwrap_or(false)
            };
            self.downloaded.insert(m.id, ok);
        }
    }

    pub fn set_downloaded(&mut self, id: &str, value: bool) {
        if let Some(m) = self.models.iter().find(|m| m.id == id) {
            self.downloaded.insert(m.id, value);
        }
    }

    pub fn is_downloaded(&self, id: &str) -> bool {
        self.downloaded.get(id).copied().unwrap_or(false)
    }

    /// The chosen model, or the first usable one.
    pub fn model(&self, id: Option<&str>) -> Option<&ModelInfo> {
        id.and_then(|id| self.models.iter().find(|m| m.id == id))
            .filter(|m| self.is_available(m))
            .or_else(|| self.models.iter().find(|m| self.is_available(m)))
    }

    /// HTDemucs weights aren't hosted (they need a manual ONNX export); until the file is in
    /// the models folder they're shown as "coming soon".
    pub fn needs_manual_export(&self, m: &ModelInfo) -> bool {
        !self.demo && matches!(m.arch, ModelArch::HtDemucs { .. }) && !self.is_downloaded(m.id)
    }

    pub fn is_available(&self, m: &ModelInfo) -> bool {
        !self.needs_manual_export(m)
    }

    pub fn device_name(&self, device: Device) -> String {
        self.devices
            .iter()
            .find(|(d, _)| *d == device)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| match device {
                Device::Auto => "Auto".into(),
                Device::Cpu => "CPU".into(),
                Device::DirectMl(i) => format!("DirectML #{i}"),
            })
    }
}

// ---- demo data -----------------------------------------------------------------------------

fn demo_registry() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "htdemucs-4s",
            name: "HTDemucs 4-stem",
            description: "The all-rounder. Vocals, drums, bass and everything else.",
            stems: vec![
                StemKind::Vocals,
                StemKind::Drums,
                StemKind::Bass,
                StemKind::Other,
            ],
            sample_rate: 44_100,
            arch: ModelArch::HtDemucs {
                segment_samples: 343_980,
            },
            url: "https://example.invalid/htdemucs.onnx",
            sha256: None,
            size_bytes: 168_000_000,
            license: "MIT",
        },
        ModelInfo {
            id: "htdemucs-6s",
            name: "HTDemucs 6-stem",
            description: "Adds guitar and piano. Great for flips and isolating chops.",
            stems: vec![
                StemKind::Vocals,
                StemKind::Drums,
                StemKind::Bass,
                StemKind::Guitar,
                StemKind::Piano,
                StemKind::Other,
            ],
            sample_rate: 44_100,
            arch: ModelArch::HtDemucs {
                segment_samples: 343_980,
            },
            url: "https://example.invalid/htdemucs_6s.onnx",
            sha256: None,
            size_bytes: 105_000_000,
            license: "MIT",
        },
        ModelInfo {
            id: "mdx-inst-hq3",
            name: "MDX-Net Inst HQ3",
            description: "Fast, clean acapellas and instrumentals.",
            stems: vec![StemKind::Vocals, StemKind::Instrumental],
            sample_rate: 44_100,
            arch: ModelArch::MdxNet {
                n_fft: 6144,
                hop: 1024,
                dim_f: 3072,
                dim_t: 256,
                compensate: 1.022,
            },
            url: "https://example.invalid/UVR-MDX-NET-Inst_HQ_3.onnx",
            sha256: None,
            size_bytes: 66_800_000,
            license: "MIT",
        },
        ModelInfo {
            id: "dnr-cinematic",
            name: "Cinematic DnR",
            description: "Film and video audio: dialogue, music and sound effects.",
            stems: vec![StemKind::Dialogue, StemKind::Music, StemKind::Effects],
            sample_rate: 44_100,
            arch: ModelArch::MdxNet {
                n_fft: 4096,
                hop: 1024,
                dim_f: 2048,
                dim_t: 256,
                compensate: 1.0,
            },
            url: "https://example.invalid/dnr.onnx",
            sha256: None,
            size_bytes: 54_200_000,
            license: "Apache-2.0",
        },
    ]
}

fn demo_devices() -> Vec<(Device, String)> {
    vec![
        (Device::Auto, "Auto (best available)".into()),
        (Device::DirectMl(0), "AMD Radeon RX 6700S (DirectML)".into()),
        (
            Device::DirectMl(1),
            "AMD Radeon(TM) Graphics (DirectML)".into(),
        ),
        (Device::Cpu, "CPU".into()),
    ]
}

/// "168 MB", "1.2 GB".
pub fn human_bytes(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{b} B")
    }
}

/// "3:07".
pub fn mmss(secs: f64) -> String {
    let s = secs.max(0.0);
    format!(
        "{}:{:02}",
        (s / 60.0).floor() as u64,
        (s % 60.0).floor() as u64
    )
}

/// "1:02.345".
pub fn mmss_ms(secs: f64) -> String {
    let s = secs.max(0.0);
    let ms = ((s.fract()) * 1000.0).floor() as u64;
    format!(
        "{}:{:02}.{:03}",
        (s / 60.0).floor() as u64,
        (s % 60.0).floor() as u64,
        ms
    )
}
