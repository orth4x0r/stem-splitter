//! HTTP download to a file: `<dest>.part` first, renamed into place only once complete (and
//! verified when a sha256 is known), so a half-finished file is never mistaken for a model.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::{ModelError, Result};

const BUF_SIZE: usize = 256 * 1024;

pub(crate) fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; BUF_SIZE];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("stem-splitter/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        // Model files are large; no overall deadline (cancel covers the "stuck" case).
        .timeout(None::<Duration>)
        .build()
        .map_err(|e| ModelError::Download(e.to_string()))
}

/// Download `url` to `dest`. `progress` gets the completed fraction (None if the server sends
/// no length). Checks `cancel` between reads.
pub(crate) fn download_to(
    url: &str,
    dest: &Path,
    sha256: Option<&str>,
    progress: &mut dyn FnMut(Option<f32>),
    cancel: &AtomicBool,
) -> Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = part_path(dest);
    let result = download_inner(url, &part, sha256, progress, cancel);
    match result {
        Ok(()) => {
            std::fs::rename(&part, dest)?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

fn download_inner(
    url: &str,
    part: &Path,
    sha256: Option<&str>,
    progress: &mut dyn FnMut(Option<f32>),
    cancel: &AtomicBool,
) -> Result<()> {
    tracing::info!("downloading {url}");
    progress(Some(0.0));
    let mut resp = client()?
        .get(url)
        .send()
        .map_err(|e| ModelError::Download(format!("{url}: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ModelError::Download(format!("{url}: HTTP {status}")));
    }
    let total = resp.content_length().filter(|&n| n > 0);
    let mut file = File::create(part)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; BUF_SIZE];
    let mut done: u64 = 0;
    let mut last_reported = -1.0f32;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ModelError::Cancelled);
        }
        let n = resp
            .read(&mut buf)
            .map_err(|e| ModelError::Download(format!("{url}: {e}")))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        done += n as u64;
        match total {
            Some(t) => {
                let f = (done as f64 / t as f64).min(1.0) as f32;
                // ~200 updates per file are plenty for a progress bar.
                if f - last_reported >= 0.005 {
                    last_reported = f;
                    progress(Some(f));
                }
            }
            None => {
                if done % (8 * BUF_SIZE as u64) < n as u64 {
                    progress(None);
                }
            }
        }
    }
    file.flush()?;
    file.sync_all()?;
    drop(file);
    if let Some(t) = total
        && done != t
    {
        return Err(ModelError::Download(format!(
            "{url}: connection closed after {done} of {t} bytes"
        )));
    }
    if let Some(expected) = sha256 {
        let got = hex(&hasher.finalize());
        if !got.eq_ignore_ascii_case(expected) {
            return Err(ModelError::Download(format!(
                "{url}: sha256 mismatch (expected {expected}, got {got})"
            )));
        }
    }
    progress(Some(1.0));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_path_appends_suffix() {
        assert_eq!(
            part_path(Path::new("/x/models/a.onnx")),
            PathBuf::from("/x/models/a.onnx.part")
        );
    }

    #[test]
    fn sha256_of_known_data() {
        let dir = std::env::temp_dir().join(format!("stem-models-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("abc.txt");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
