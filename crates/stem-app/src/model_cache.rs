//! Keeps the last loaded model between splits.
//!
//! Building an onnxruntime session (above all a DirectML one, which compiles the graph for the
//! GPU) takes seconds, and a queue, a chain or "split again" loads the same model over and
//! over. One model is kept (models are hundreds of MB of video memory) and dropped after a few
//! idle minutes.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use stem_models::{Device, ModelInfo, Separator};

/// How long an unused model stays loaded.
const IDLE: Duration = Duration::from_secs(5 * 60);

/// What a cached session was built from. The file's size and time catch a re-download.
#[derive(Debug, Clone, PartialEq)]
struct Key {
    model: &'static str,
    path: PathBuf,
    device: Device,
    file: Option<(u64, SystemTime)>,
}

impl Key {
    fn new(info: &ModelInfo, path: &Path, device: Device) -> Self {
        let file = std::fs::metadata(path)
            .ok()
            .and_then(|m| Some((m.len(), m.modified().ok()?)));
        Self {
            model: info.id,
            path: path.to_path_buf(),
            device,
            file,
        }
    }
}

struct Entry {
    key: Key,
    sep: Box<dyn Separator>,
    parked: Instant,
}

static CACHE: Mutex<Option<Entry>> = Mutex::new(None);

fn slot() -> std::sync::MutexGuard<'static, Option<Entry>> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The cached separator for (`info`, `path`, `device`), or a freshly loaded one. Hand it back
/// with [`park`] when done; while it is out, other jobs load their own.
pub fn take(info: &ModelInfo, path: &Path, device: Device) -> Result<Lease, String> {
    take_with(info, path, device, |i, p, d| {
        stem_models::load(i, p, d).map_err(|e| e.to_string())
    })
}

fn take_with(
    info: &ModelInfo,
    path: &Path,
    device: Device,
    load: impl FnOnce(&ModelInfo, &Path, Device) -> Result<Box<dyn Separator>, String>,
) -> Result<Lease, String> {
    let key = Key::new(info, path, device);
    let hit = {
        let mut s = slot();
        match s.take() {
            Some(e) if e.key == key => Some(e.sep),
            other => {
                // Another model: free it before loading this one, so both never sit in
                // video memory at once.
                drop(other);
                None
            }
        }
    };
    let (sep, cached) = match hit {
        Some(sep) => {
            tracing::info!("{}: reusing the loaded model", info.id);
            (sep, true)
        }
        None => (load(info, path, device)?, false),
    };
    Ok(Lease { key, sep, cached })
}

/// A separator borrowed from the cache.
pub struct Lease {
    key: Key,
    pub sep: Box<dyn Separator>,
    /// Came from the cache (no load).
    #[cfg_attr(not(test), allow(dead_code))]
    pub cached: bool,
}

/// Put a separator back for the next job, replacing whatever is cached.
pub fn park(lease: Lease) {
    let parked = Instant::now();
    *slot() = Some(Entry {
        key: lease.key,
        sep: lease.sep,
        parked,
    });
    // Drop it if nothing uses it for a while.
    let _ = std::thread::Builder::new()
        .name("model-cache-expiry".into())
        .spawn(move || {
            std::thread::sleep(IDLE);
            let mut s = slot();
            if s.as_ref().is_some_and(|e| e.parked == parked) {
                *s = None;
            }
        });
}

/// Free the cached model now.
#[cfg(test)]
fn clear() {
    *slot() = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use stem_audio::AudioBuffer;
    use stem_models::{ModelError, Progress, StemKind};

    struct Fake(ModelInfo);
    impl Separator for Fake {
        fn info(&self) -> &ModelInfo {
            &self.0
        }
        fn separate(
            &mut self,
            _: &AudioBuffer,
            _: &mut dyn FnMut(Progress),
            _: &AtomicBool,
        ) -> stem_models::Result<Vec<(StemKind, AudioBuffer)>> {
            Err(ModelError::Cancelled)
        }
    }

    #[test]
    fn reuses_the_same_model_only() {
        let a = stem_models::find_model("htdemucs").unwrap();
        let b = stem_models::find_model("mdx-inst-hq3").unwrap();
        let p = Path::new("/nonexistent/model.onnx");
        let loads = std::cell::Cell::new(0);
        let load = |i: &ModelInfo, _: &Path, _: Device| -> Result<Box<dyn Separator>, String> {
            loads.set(loads.get() + 1);
            Ok(Box::new(Fake(i.clone())))
        };
        clear();
        let l = take_with(&a, p, Device::Cpu, load).unwrap();
        assert!(!l.cached);
        park(l);
        let l = take_with(&a, p, Device::Cpu, load).unwrap();
        assert!(l.cached, "same model, same device: no reload");
        park(l);
        let l = take_with(&a, p, Device::DirectMl(0), load).unwrap();
        assert!(!l.cached, "another device reloads");
        park(l);
        let l = take_with(&b, p, Device::DirectMl(0), load).unwrap();
        assert!(!l.cached && l.sep.info().id == b.id);
        // While leased, a second job gets its own copy.
        let l2 = take_with(&b, p, Device::DirectMl(0), load).unwrap();
        assert!(!l2.cached);
        assert_eq!(loads.get(), 4);
        clear();
    }
}
