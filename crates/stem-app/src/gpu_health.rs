//! GPU failure memory: when a separation fails on DirectML (an error, or broken output that
//! `stem_models` retried on the CPU), the (model, adapter) pair is remembered for
//! [`REMEMBER_SECS`] so the next runs of that model go straight to the CPU instead of failing
//! on the GPU first.
//!
//! Idea from gentij/stem-splitter-core (MIT OR Apache-2.0), which caches "unhealthy" execution
//! providers for 7 days; reimplemented here, no code copied.
//!
//! Kept in its own small JSON file in the data folder (not in [`crate::settings::Settings`]),
//! because the worker threads record failures while the UI owns the settings.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use stem_models::Device;

/// How long a GPU failure keeps a model on the CPU: 7 days.
pub const REMEMBER_SECS: u64 = 7 * 24 * 3600;

const FILE: &str = "gpu_failures.json";

/// One remembered failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuFailure {
    pub model_id: String,
    /// Display name, for the Settings list.
    #[serde(default)]
    pub model_name: String,
    /// The DirectML adapter's name ("AMD Radeon RX 6700S").
    pub adapter: String,
    /// Unix seconds.
    pub when: u64,
    /// What went wrong, as `stem_models` reported it.
    #[serde(default)]
    pub reason: String,
}

impl GpuFailure {
    /// Still within [`REMEMBER_SECS`] at `now`. A time in the future (clock changed) counts as
    /// fresh rather than forever.
    pub fn is_active(&self, now: u64) -> bool {
        now.saturating_sub(self.when) < REMEMBER_SECS && self.when <= now + REMEMBER_SECS
    }

    /// Unix seconds when it stops applying.
    pub fn expires(&self) -> u64 {
        self.when + REMEMBER_SECS
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuFailures {
    pub entries: Vec<GpuFailure>,
}

impl GpuFailures {
    /// The active failure for `model_id` on `adapter`, if any.
    pub fn lookup(&self, model_id: &str, adapter: &str, now: u64) -> Option<&GpuFailure> {
        self.entries
            .iter()
            .find(|f| f.model_id == model_id && f.adapter == adapter && f.is_active(now))
    }

    /// Remember a failure, replacing an older one for the same model and adapter.
    pub fn record(&mut self, failure: GpuFailure) {
        self.entries
            .retain(|f| !(f.model_id == failure.model_id && f.adapter == failure.adapter));
        self.entries.push(failure);
    }

    /// Drop expired entries.
    pub fn prune(&mut self, now: u64) {
        self.entries.retain(|f| f.is_active(now));
    }

    /// Active entries, newest first.
    pub fn active(&self, now: u64) -> Vec<GpuFailure> {
        let mut v: Vec<GpuFailure> = self
            .entries
            .iter()
            .filter(|f| f.is_active(now))
            .cloned()
            .collect();
        v.sort_by_key(|f| std::cmp::Reverse(f.when));
        v
    }

    pub fn forget(&mut self, model_id: &str, adapter: &str) {
        self.entries
            .retain(|f| !(f.model_id == model_id && f.adapter == adapter));
    }
}

/// The JSON file under the app's data folder.
pub fn file(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// Serialises read-modify-write between the job threads and the UI.
static LOCK: Mutex<()> = Mutex::new(());

/// Read the file (empty when missing or unreadable).
pub fn load(path: &Path) -> GpuFailures {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(path: &Path, f: &GpuFailures) {
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(f).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    };
    if let Err(e) = write() {
        tracing::warn!("saving {}: {e}", path.display());
    }
}

/// Load, change, prune and save under the lock.
pub fn update(path: &Path, now: u64, change: impl FnOnce(&mut GpuFailures)) {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = load(path);
    change(&mut f);
    f.prune(now);
    save(path, &f);
}

/// The active failure for `model_id` on `adapter` in the file at `path`.
pub fn lookup(path: &Path, model_id: &str, adapter: &str, now: u64) -> Option<GpuFailure> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load(path).lookup(model_id, adapter, now).cloned()
}

/// The adapter a run on `device` would use, by name; `None` for the CPU.
pub fn adapter_for(device: Device) -> Option<String> {
    match stem_models::resolve_device(device) {
        Device::DirectMl(i) => Some(
            stem_models::gpu_adapters()
                .into_iter()
                .find(|a| a.index == i)
                .map(|a| a.name)
                .unwrap_or_else(|| format!("DirectML adapter {i}")),
        ),
        _ => None,
    }
}

/// "3 days" / "5 h" / "under an hour" until `f` expires.
pub fn time_left(f: &GpuFailure, now: u64) -> String {
    let left = f.expires().saturating_sub(now);
    match left {
        0..3600 => "under an hour".into(),
        3600..86_400 => format!("{} h", left / 3600),
        _ => {
            let d = left.div_ceil(86_400);
            format!("{d} day{}", if d == 1 { "" } else { "s" })
        }
    }
}

/// What the Settings page shows: the file's active entries, re-read only when it changed.
pub fn cached_active(path: &Path, now: u64) -> Vec<GpuFailure> {
    static CACHE: Mutex<Option<(PathBuf, Option<std::time::SystemTime>, GpuFailures)>> =
        Mutex::new(None);
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let fresh = matches!(&*c, Some((p, t, _)) if p == path && *t == mtime);
    if !fresh {
        *c = Some((path.to_path_buf(), mtime, load(path)));
    }
    c.as_ref()
        .map(|(_, _, f)| f.active(now))
        .unwrap_or_default()
}

/// Stand-in entries for the screenshot scene, so it never touches the real file.
static DEMO: Mutex<Option<Vec<GpuFailure>>> = Mutex::new(None);

pub fn set_demo_entries(entries: Vec<GpuFailure>) {
    *DEMO.lock().unwrap_or_else(|e| e.into_inner()) = Some(entries);
}

pub fn demo_entries() -> Option<Vec<GpuFailure>> {
    DEMO.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn clear_demo_entries() {
    if let Some(v) = DEMO.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        v.clear();
    }
}

pub fn forget_demo_entry(model_id: &str) {
    if let Some(v) = DEMO.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        v.retain(|f| f.model_id != model_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400;

    fn fail(model: &str, adapter: &str, when: u64) -> GpuFailure {
        GpuFailure {
            model_id: model.into(),
            model_name: model.into(),
            adapter: adapter.into(),
            when,
            reason: "GPU run failed".into(),
        }
    }

    #[test]
    fn failures_expire_after_seven_days() {
        let t0 = 1_700_000_000;
        let mut f = GpuFailures::default();
        f.record(fail("mel-roformer-kim", "RX 6700S", t0));
        let hit = |f: &GpuFailures, m: &str, a: &str, now: u64| f.lookup(m, a, now).is_some();
        assert!(hit(&f, "mel-roformer-kim", "RX 6700S", t0));
        assert!(hit(&f, "mel-roformer-kim", "RX 6700S", t0 + 7 * DAY - 1));
        assert!(!hit(&f, "mel-roformer-kim", "RX 6700S", t0 + 7 * DAY));
        // Per model and per adapter.
        assert!(!hit(&f, "htdemucs", "RX 6700S", t0 + DAY));
        assert!(!hit(&f, "mel-roformer-kim", "Radeon 680M", t0 + DAY));
        // A clock set back still sees it; one far in the past (failure "in the future") doesn't
        // keep the model on the CPU for good.
        assert!(hit(&f, "mel-roformer-kim", "RX 6700S", t0 - DAY));
        assert!(!hit(&f, "mel-roformer-kim", "RX 6700S", t0 - 30 * DAY));
        f.prune(t0 + 8 * DAY);
        assert!(f.entries.is_empty());
    }

    #[test]
    fn record_replaces_and_forget_removes() {
        let t0 = 1_700_000_000;
        let mut f = GpuFailures::default();
        f.record(fail("a", "gpu", t0));
        f.record(fail("b", "gpu", t0 + 10));
        f.record(fail("a", "gpu", t0 + 6 * DAY));
        assert_eq!(f.entries.len(), 2);
        // The newer failure restarts the clock.
        assert!(f.lookup("a", "gpu", t0 + 10 * DAY).is_some());
        let active: Vec<_> = f
            .active(t0 + 6 * DAY)
            .into_iter()
            .map(|e| e.model_id)
            .collect();
        assert_eq!(active, vec!["a", "b"]);
        f.forget("a", "gpu");
        assert!(f.lookup("a", "gpu", t0 + 6 * DAY).is_none());
        assert!(f.lookup("b", "gpu", t0 + 6 * DAY).is_some());
    }

    #[test]
    fn file_roundtrip_and_time_left() {
        let dir = std::env::temp_dir().join(format!("stem-gpu-health-{}", std::process::id()));
        let path = file(&dir);
        let t0 = 1_700_000_000;
        assert!(lookup(&path, "a", "gpu", t0).is_none());
        update(&path, t0, |f| f.record(fail("a", "gpu", t0)));
        update(&path, t0, |f| f.record(fail("old", "gpu", t0 - 8 * DAY)));
        // Expired entries are pruned on save.
        assert_eq!(load(&path).entries.len(), 1);
        assert_eq!(
            lookup(&path, "a", "gpu", t0 + DAY).unwrap().reason,
            "GPU run failed"
        );
        assert_eq!(cached_active(&path, t0).len(), 1);
        let e = fail("a", "gpu", t0);
        assert_eq!(time_left(&e, t0), "7 days");
        assert_eq!(time_left(&e, t0 + 5 * DAY + 1), "2 days");
        assert_eq!(time_left(&e, t0 + 6 * DAY + 1), "23 h");
        assert_eq!(time_left(&e, t0 + 7 * DAY - 7200), "2 h");
        update(&path, t0, |f| f.forget("a", "gpu"));
        assert!(load(&path).entries.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_adapter_for_the_cpu() {
        assert_eq!(adapter_for(Device::Cpu), None);
    }
}
