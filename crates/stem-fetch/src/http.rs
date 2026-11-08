//! Plain HTTP downloads (direct audio links, yt-dlp and ffmpeg binaries).

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use reqwest::blocking::{Client, Response};

use crate::{FetchError, FetchProgress, Result};

const USER_AGENT: &str = concat!("stem-splitter/", env!("CARGO_PKG_VERSION"));
const CHUNK: usize = 64 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) fn client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(30))
        // The blocking client defaults to a 30 s timeout for the whole transfer, far too short
        // for a 150 MB ffmpeg zip on a slow line; this only guards against a stalled connection.
        .timeout(Duration::from_secs(2 * 60 * 60))
        .build()
        .map_err(|e| FetchError::Http(e.to_string()))
}

/// GET `url`, failing on non-success status codes.
pub(crate) fn get(client: &Client, url: &str) -> Result<Response> {
    let resp = client
        .get(url)
        .send()
        .map_err(|e| FetchError::Http(format!("{url}: {}", error_chain(&e))))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(FetchError::Http(format!("{url}: HTTP {status}")));
    }
    Ok(resp)
}

/// `Content-Type` without parameters, lowercased.
pub(crate) fn content_type(resp: &Response) -> Option<String> {
    let v = resp.headers().get(reqwest::header::CONTENT_TYPE)?;
    let v = v.to_str().ok()?;
    Some(v.split(';').next()?.trim().to_ascii_lowercase())
}

/// Stream the body of `resp` to `dest` through `dest.part`, reporting progress and checking
/// `cancel` between chunks. The `.part` file is removed on failure or cancellation.
pub(crate) fn save(
    mut resp: Response,
    dest: &Path,
    progress: &mut dyn FnMut(FetchProgress),
    cancel: &AtomicBool,
) -> Result<()> {
    let part = part_path(dest);
    let result = (|| {
        let total = resp.content_length().filter(|&n| n > 0);
        let mut out = BufWriter::new(File::create(&part)?);
        let mut buf = vec![0u8; CHUNK];
        let mut bytes = 0u64;
        let mut last = Instant::now();
        let report = |bytes: u64, progress: &mut dyn FnMut(FetchProgress)| {
            progress(FetchProgress::Downloading {
                fraction: total.map(|t| (bytes as f64 / t as f64).min(1.0) as f32),
                bytes,
            })
        };
        report(0, progress);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(FetchError::Cancelled);
            }
            let n = resp
                .read(&mut buf)
                .map_err(|e| FetchError::Http(format!("reading response: {e}")))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            bytes += n as u64;
            if last.elapsed() >= PROGRESS_INTERVAL {
                last = Instant::now();
                report(bytes, progress);
            }
        }
        if let Some(t) = total
            && bytes < t
        {
            return Err(FetchError::Http(format!(
                "connection closed after {bytes} of {t} bytes"
            )));
        }
        out.flush()?;
        drop(out);
        report(bytes, progress);
        Ok(())
    })();
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

/// Download `url` straight to `dest` (used for tool binaries).
pub(crate) fn download_file(
    url: &str,
    dest: &Path,
    progress: &mut dyn FnMut(FetchProgress),
    cancel: &AtomicBool,
) -> Result<()> {
    let client = client()?;
    let resp = get(&client, url)?;
    save(resp, dest, progress, cancel)
}

pub(crate) fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// reqwest's top-level messages ("error sending request") hide the useful part in `source()`.
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        let s_msg = s.to_string();
        if !msg.contains(&s_msg) {
            msg.push_str(": ");
            msg.push_str(&s_msg);
        }
        src = s.source();
    }
    msg
}
