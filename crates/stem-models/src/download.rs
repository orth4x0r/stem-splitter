//! HTTP download to a file: `<dest>.part` first, renamed into place only once complete (and
//! verified when a sha256 is known), so a half-finished file is never mistaken for a model.
//!
//! An interrupted or cancelled download keeps its `.part` file; the next attempt resumes it
//! with an HTTP `Range` request (servers that ignore ranges just send the whole file again).

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::{ModelError, Result};

const BUF_SIZE: usize = 256 * 1024;

/// Live numbers for a running download.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DownloadStatus {
    /// Bytes on disk so far (including what an earlier attempt left in the `.part` file).
    pub done: u64,
    /// Full size, when the server says.
    pub total: Option<u64>,
    /// Smoothed transfer rate of this attempt.
    pub bytes_per_sec: f64,
    /// Bytes that were already there from an earlier, interrupted attempt.
    pub resumed_from: u64,
}

impl DownloadStatus {
    pub fn fraction(&self) -> Option<f32> {
        self.total
            .filter(|&t| t > 0)
            .map(|t| (self.done as f64 / t as f64).min(1.0) as f32)
    }

    /// Seconds left at the current rate.
    pub fn eta_secs(&self) -> Option<f64> {
        let left = self.total?.saturating_sub(self.done);
        (self.bytes_per_sec > 1.0).then(|| left as f64 / self.bytes_per_sec)
    }
}

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

/// Feed a file's bytes into `h`.
fn hash_into(path: &Path, h: &mut Sha256) -> Result<()> {
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; BUF_SIZE];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        h.update(&buf[..n]);
    }
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut h = Sha256::new();
    hash_into(path, &mut h)?;
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
    download_with_status(url, dest, sha256, &mut |s| progress(s.fraction()), cancel)
}

/// [`download_to`] with byte counts and speed. Resumes a `.part` file left by an earlier
/// attempt; keeps it when cancelled or interrupted, deletes it when the data turns out bad.
pub(crate) fn download_with_status(
    url: &str,
    dest: &Path,
    sha256: Option<&str>,
    progress: &mut dyn FnMut(&DownloadStatus),
    cancel: &AtomicBool,
) -> Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = part_path(dest);
    let mut result = download_inner(url, &part, sha256, progress, cancel);
    if matches!(result, Err(Retry::FromScratch)) {
        let _ = std::fs::remove_file(&part);
        result = download_inner(url, &part, sha256, progress, cancel);
    }
    match result {
        Ok(()) => {
            std::fs::rename(&part, dest)?;
            Ok(())
        }
        Err(Retry::FromScratch) => {
            let _ = std::fs::remove_file(&part);
            Err(ModelError::Download(format!(
                "{url}: server rejected the resume request"
            )))
        }
        Err(Retry::Corrupt(e)) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
        // Cancelled / network error: keep the .part so the next attempt resumes.
        Err(Retry::No(e)) => Err(e),
    }
}

enum Retry {
    /// The `.part` can't be resumed (e.g. HTTP 416): delete it and start over once.
    FromScratch,
    /// The downloaded data is wrong (size or checksum): delete it.
    Corrupt(ModelError),
    /// Anything else; a `.part` file stays for resuming.
    No(ModelError),
}

impl From<ModelError> for Retry {
    fn from(e: ModelError) -> Self {
        Retry::No(e)
    }
}

impl From<std::io::Error> for Retry {
    fn from(e: std::io::Error) -> Self {
        Retry::No(e.into())
    }
}

/// First byte offset of a `Content-Range: bytes <start>-<end>/<total>` header, and the total.
fn content_range(resp: &reqwest::blocking::Response) -> Option<(u64, Option<u64>)> {
    let v = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)?
        .to_str()
        .ok()?;
    let rest = v.trim().strip_prefix("bytes")?.trim();
    let (range, total) = rest.split_once('/')?;
    let start = range.split_once('-')?.0.trim().parse().ok()?;
    Some((start, total.trim().parse().ok()))
}

/// Exponentially smoothed bytes/second, updated at most every 250 ms.
struct Speed {
    last_t: Instant,
    last_bytes: u64,
    rate: f64,
}

impl Speed {
    fn new(bytes: u64) -> Self {
        Self {
            last_t: Instant::now(),
            last_bytes: bytes,
            rate: 0.0,
        }
    }
    /// Returns true when the rate was updated (a good moment to report progress).
    fn update(&mut self, bytes: u64) -> bool {
        let dt = self.last_t.elapsed().as_secs_f64();
        if dt < 0.25 {
            return false;
        }
        let inst = (bytes - self.last_bytes) as f64 / dt;
        self.rate = if self.rate == 0.0 {
            inst
        } else {
            0.7 * self.rate + 0.3 * inst
        };
        self.last_t = Instant::now();
        self.last_bytes = bytes;
        true
    }
}

fn download_inner(
    url: &str,
    part: &Path,
    sha256: Option<&str>,
    progress: &mut dyn FnMut(&DownloadStatus),
    cancel: &AtomicBool,
) -> std::result::Result<(), Retry> {
    if cancel.load(Ordering::Relaxed) {
        return Err(ModelError::Cancelled.into());
    }
    let existing = part.metadata().map(|m| m.len()).unwrap_or(0);
    tracing::info!(
        "downloading {url}{}",
        if existing > 0 {
            format!(" (resuming at {existing} bytes)")
        } else {
            String::new()
        }
    );
    let mut req = client()?.get(url);
    if existing > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={existing}-"));
    }
    let mut resp = req
        .send()
        .map_err(|e| ModelError::Download(format!("{url}: {e}")))?;
    let status = resp.status();
    let mut hasher = Sha256::new();
    let (mut file, mut done, total) = if existing > 0
        && status == reqwest::StatusCode::PARTIAL_CONTENT
    {
        let (start, total) = content_range(&resp).unwrap_or((existing, None));
        if start != existing {
            return Err(Retry::FromScratch);
        }
        let total = total.or_else(|| resp.content_length().map(|n| n + existing));
        if sha256.is_some() {
            hash_into(part, &mut hasher)?;
        }
        let file = OpenOptions::new().append(true).open(part)?;
        (file, existing, total)
    } else if existing > 0 && status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        // Usually the .part already holds the whole file (or the file changed upstream).
        return Err(Retry::FromScratch);
    } else if status.is_success() {
        // Fresh download, or a server that ignores Range and sends everything again.
        let total = resp.content_length().filter(|&n| n > 0);
        (File::create(part)?, 0, total)
    } else {
        return Err(ModelError::Download(format!("{url}: HTTP {status}")).into());
    };
    let resumed_from = done;
    let mut speed = Speed::new(done);
    let mut st = DownloadStatus {
        done,
        total,
        bytes_per_sec: 0.0,
        resumed_from,
    };
    progress(&st);
    let mut buf = vec![0u8; BUF_SIZE];
    loop {
        if cancel.load(Ordering::Relaxed) {
            file.flush()?;
            return Err(ModelError::Cancelled.into());
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
        if speed.update(done) {
            st.done = done;
            st.bytes_per_sec = speed.rate;
            progress(&st);
        }
    }
    file.flush()?;
    file.sync_all()?;
    drop(file);
    if let Some(t) = total {
        if done < t {
            // Keep the .part: the next attempt picks up from here.
            return Err(ModelError::Download(format!(
                "{url}: connection closed after {done} of {t} bytes"
            ))
            .into());
        }
        if done > t {
            return Err(Retry::Corrupt(ModelError::Download(format!(
                "{url}: got {done} bytes, expected {t}"
            ))));
        }
    }
    if let Some(expected) = sha256 {
        let got = hex(&hasher.finalize());
        if !got.eq_ignore_ascii_case(expected) {
            return Err(Retry::Corrupt(ModelError::Download(format!(
                "{url}: sha256 mismatch (expected {expected}, got {got})"
            ))));
        }
    }
    st.done = done;
    st.total = Some(done);
    progress(&st);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

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

    #[test]
    fn status_math() {
        let s = DownloadStatus {
            done: 25,
            total: Some(100),
            bytes_per_sec: 25.0,
            resumed_from: 0,
        };
        assert_eq!(s.fraction(), Some(0.25));
        assert_eq!(s.eta_secs(), Some(3.0));
        assert_eq!(DownloadStatus::default().eta_secs(), None);
    }

    /// Serves `body`, honouring `Range: bytes=N-` when `ranges` is set. Records Range headers.
    fn serve(body: Vec<u8>, ranges: bool) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut range = None;
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("range:") {
                        range = Some(v.trim().to_owned());
                    }
                }
                seen2
                    .lock()
                    .unwrap()
                    .push(range.clone().unwrap_or_default());
                let start: Option<usize> = range
                    .filter(|_| ranges)
                    .and_then(|r| r.strip_prefix("bytes=")?.strip_suffix('-')?.parse().ok());
                let _ = match start {
                    Some(s) if s >= body.len() => {
                        write!(stream, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    }
                    Some(s) => write!(
                        stream,
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {s}-{}/{}\r\nConnection: close\r\n\r\n",
                        body.len() - s,
                        body.len() - 1,
                        body.len()
                    )
                    .and_then(|_| stream.write_all(&body[s..])),
                    None => write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .and_then(|_| stream.write_all(&body)),
                };
            }
        });
        (format!("http://{addr}/model.onnx"), seen)
    }

    fn body() -> Vec<u8> {
        (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect()
    }

    fn sha(data: &[u8]) -> String {
        hex(&Sha256::digest(data))
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stem-dl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("model.onnx")
    }

    #[test]
    fn resumes_a_partial_file_with_range() {
        let data = body();
        let (url, seen) = serve(data.clone(), true);
        let dest = tmp("resume");
        std::fs::write(part_path(&dest), &data[..100_000]).unwrap();
        let mut last = DownloadStatus::default();
        let never = AtomicBool::new(false);
        download_with_status(&url, &dest, Some(&sha(&data)), &mut |s| last = *s, &never).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert!(!part_path(&dest).exists());
        assert_eq!(seen.lock().unwrap().as_slice(), ["bytes=100000-"]);
        assert_eq!(last.resumed_from, 100_000);
        assert_eq!(last.done, data.len() as u64);
    }

    #[test]
    fn server_without_ranges_restarts_cleanly() {
        let data = body();
        let (url, _) = serve(data.clone(), false);
        let dest = tmp("norange");
        std::fs::write(part_path(&dest), b"garbage that is not the start").unwrap();
        let never = AtomicBool::new(false);
        download_with_status(&url, &dest, Some(&sha(&data)), &mut |_| {}, &never).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
    }

    #[test]
    fn complete_part_file_gets_416_and_starts_over() {
        let data = body();
        let (url, seen) = serve(data.clone(), true);
        let dest = tmp("416");
        std::fs::write(part_path(&dest), &data).unwrap();
        let never = AtomicBool::new(false);
        download_to(&url, &dest, None, &mut |_| {}, &never).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn bad_checksum_deletes_the_part_file() {
        let data = body();
        let (url, _) = serve(data, true);
        let dest = tmp("badsha");
        let never = AtomicBool::new(false);
        let r = download_to(&url, &dest, Some(&"0".repeat(64)), &mut |_| {}, &never);
        assert!(r.is_err());
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }
}
