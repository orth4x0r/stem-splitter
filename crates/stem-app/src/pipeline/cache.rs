//! Stem cache: model output kept under `<data>/cache/stems/<key>/`, so re-running a preset or
//! a chain step on the same input skips the model.
//!
//! A key is a hash of the input (file contents plus the split range), the model id and the
//! path through the chain (see `chain.rs`). Each entry holds `meta.json` and one 32-bit float
//! WAV per stem (lossless, about 100 MB per stem for a 5-minute song), so the cache has a size
//! limit and drops the least recently used entries first.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use stem_audio::{AudioBuffer, WavFormat};
use stem_models::StemKind;

use crate::paths::AppPaths;

const META: &str = "meta.json";

/// The cache folder (a temp folder in demo mode, so demos never touch real data).
pub fn dir(paths: &AppPaths, demo: bool) -> PathBuf {
    if demo {
        std::env::temp_dir()
            .join("stem-splitter-demo")
            .join("cache")
    } else {
        paths.data.join("cache").join("stems")
    }
}

/// A short hex key for `parts` (order matters).
pub fn key(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0u8]);
    }
    hex(&h.finalize()[..16])
}

/// Hash of a file's contents.
pub fn file_key(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()[..16]))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Serialize, Deserialize)]
struct Meta {
    model: String,
    stems: Vec<StemKind>,
}

pub struct StemCache {
    root: PathBuf,
    limit: u64,
}

impl StemCache {
    pub fn new(root: PathBuf, limit: u64) -> Self {
        Self { root, limit }
    }

    #[cfg(test)]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn entry(&self, key: &str) -> PathBuf {
        self.root.join(key)
    }

    /// The stems stored under `key`, or `None` (missing or unreadable entries count as misses).
    pub fn get(&self, key: &str) -> Option<Vec<(StemKind, AudioBuffer)>> {
        let dir = self.entry(key);
        let meta: Meta = serde_json::from_slice(&std::fs::read(dir.join(META)).ok()?).ok()?;
        let mut out = Vec::with_capacity(meta.stems.len());
        for (i, kind) in meta.stems.iter().enumerate() {
            let audio = stem_audio::decode_file(&dir.join(format!("{i}.wav"))).ok()?;
            out.push((*kind, audio));
        }
        // Mark as recently used.
        if let Ok(f) = std::fs::File::options().write(true).open(dir.join(META)) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(out)
    }

    /// Store `stems` under `key`, then trim the cache to its limit.
    pub fn put(
        &self,
        key: &str,
        model: &str,
        stems: &[(StemKind, AudioBuffer)],
    ) -> Result<(), String> {
        let dir = self.entry(key);
        let tmp = self.root.join(format!(".{key}.tmp"));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        let write = || -> Result<(), String> {
            for (i, (_, audio)) in stems.iter().enumerate() {
                stem_audio::write_wav(&tmp.join(format!("{i}.wav")), audio, WavFormat::Float32)
                    .map_err(|e| e.to_string())?;
            }
            let meta = Meta {
                model: model.to_owned(),
                stems: stems.iter().map(|(k, _)| *k).collect(),
            };
            let json = serde_json::to_vec(&meta).map_err(|e| e.to_string())?;
            // meta.json last: an entry without it is never read.
            std::fs::write(tmp.join(META), json).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::rename(&tmp, &dir).map_err(|e| e.to_string())
        };
        let res = write();
        if res.is_err() {
            let _ = std::fs::remove_dir_all(&tmp);
        }
        res?;
        self.evict(key);
        Ok(())
    }

    /// Remove least recently used entries until the cache fits its limit (never `keep`).
    fn evict(&self, keep: &str) {
        let Ok(rd) = std::fs::read_dir(&self.root) else {
            return;
        };
        let mut entries: Vec<(SystemTime, u64, PathBuf)> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| {
                let p = e.path();
                let used = std::fs::metadata(p.join(META))
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                (used, crate::paths::dir_size(&p), p)
            })
            .collect();
        let mut total: u64 = entries.iter().map(|e| e.1).sum();
        entries.sort_by_key(|e| e.0);
        for (_, size, path) in entries {
            if total <= self.limit {
                break;
            }
            if path.file_name().is_some_and(|n| n == keep) {
                continue;
            }
            if std::fs::remove_dir_all(&path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(v: f32, frames: usize) -> AudioBuffer {
        AudioBuffer::new(44_100, vec![vec![v; frames]; 2])
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stem-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn keys_are_stable_and_distinct() {
        assert_eq!(key(&["a", "b"]), key(&["a", "b"]));
        assert_ne!(key(&["a", "b"]), key(&["b", "a"]));
        assert_ne!(key(&["ab", ""]), key(&["a", "b"]));
        assert_eq!(key(&["x"]).len(), 32);
        let dir = temp("file");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), b"hello").unwrap();
        std::fs::write(dir.join("b"), b"hello").unwrap();
        std::fs::write(dir.join("c"), b"hellp").unwrap();
        let k = |n: &str| file_key(&dir.join(n)).unwrap();
        assert_eq!(k("a"), k("b"));
        assert_ne!(k("a"), k("c"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trips_stems_losslessly() {
        let root = temp("rt");
        let c = StemCache::new(root.clone(), u64::MAX);
        assert!(c.get("k").is_none());
        let stems = vec![
            (StemKind::Vocals, buf(0.25, 100)),
            (StemKind::Instrumental, buf(-0.125, 100)),
        ];
        c.put("k", "model", &stems).unwrap();
        let back = c.get("k").unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].0, StemKind::Vocals);
        assert_eq!(back[1].1, stems[1].1);
        // No temp folders left behind.
        let names: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("k")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn evicts_least_recently_used_first() {
        let root = temp("lru");
        // Each entry is ~8 KB of float WAV; allow about two of them.
        let one = buf(0.1, 1000);
        let c = StemCache::new(root.clone(), 17_000);
        let set_used = |k: &str, secs: u64| {
            let f = std::fs::File::options()
                .write(true)
                .open(root.join(k).join(META))
                .unwrap();
            f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
                .unwrap();
        };
        c.put("a", "m", &[(StemKind::Vocals, one.clone())]).unwrap();
        set_used("a", 1_000);
        c.put("b", "m", &[(StemKind::Vocals, one.clone())]).unwrap();
        set_used("b", 2_000);
        // Reading "a" makes it the most recently used.
        assert!(c.get("a").is_some());
        c.put("c", "m", &[(StemKind::Vocals, one)]).unwrap();
        assert!(c.get("a").is_some());
        assert!(c.get("b").is_none(), "b was the least recently used");
        assert!(c.get("c").is_some());
        let _ = std::fs::remove_dir_all(&root);
    }
}
