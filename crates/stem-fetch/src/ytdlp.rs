//! Running yt-dlp and making sense of its output.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::{FetchError, FetchProgress, Fetched, Result, process};

/// Format selection when ffmpeg is missing: streams symphonia can decode (AAC in mp4/m4a, mp3)
/// first. Opus/WebM only as a last resort, since we can't decode it without ffmpeg.
pub(crate) const NO_FFMPEG_FORMAT: &str =
    "bestaudio[ext=m4a]/bestaudio[acodec^=mp4a]/bestaudio[ext=mp3]/best[ext=mp4]/bestaudio";

const OUTPUT_TEMPLATE: &str = "%(title).150B [%(id)s].%(ext)s";

// Markers for the lines we ask yt-dlp to print, so they can't be confused with anything else.
const PROGRESS_TAG: &str = "STEMFETCH-PROGRESS ";
const POSTPROCESS_TAG: &str = "STEMFETCH-PP ";
const FILE_TAG: &str = "STEMFETCH-FILE ";
const TITLE_TAG: &str = "STEMFETCH-TITLE ";

/// Command-line arguments (after the program name) for downloading `url`.
pub(crate) fn args(
    url: &str,
    download_dir: &Path,
    ffmpeg: Option<&Path>,
    cookies_from_browser: Option<&str>,
) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::new();
    let mut push = |s: &str| a.push(s.into());
    for s in [
        "--ignore-config",
        "--no-playlist",
        // A playlist URL with nothing else in it: take its first entry, not the whole list.
        "--playlist-items",
        "1",
        "--newline",
        "--no-colors",
        "--encoding",
        "utf-8",
        "--restrict-filenames",
        "--no-mtime",
        // --print implies --quiet and --simulate; undo the parts we need.
        "--no-simulate",
        "--progress",
        "--progress-template",
    ] {
        push(s);
    }
    push(&format!(
        "download:{PROGRESS_TAG}%(progress.downloaded_bytes)s/%(progress.total_bytes)s/\
         %(progress.total_bytes_estimate)s/%(progress.fragment_index)s/\
         %(progress.fragment_count)s"
    ));
    push("--progress-template");
    push(&format!(
        "postprocess:{POSTPROCESS_TAG}%(progress.status)s %(progress.postprocessor)s"
    ));
    push("--print");
    push(&format!("after_move:{FILE_TAG}%(filepath)s"));
    push("--print");
    push(&format!("after_move:{TITLE_TAG}%(title)j"));
    push("-o");
    push(OUTPUT_TEMPLATE);
    match ffmpeg {
        Some(ff) => {
            for s in ["-f", "bestaudio/best", "-x", "--audio-format", "wav"] {
                push(s);
            }
            if let Some(dir) = ff.parent().filter(|d| !d.as_os_str().is_empty()) {
                push("--ffmpeg-location");
                a.push(dir.as_os_str().to_owned());
            }
        }
        None => {
            push("-f");
            push(NO_FFMPEG_FORMAT);
        }
    }
    if let Some(browser) = cookies_from_browser
        .map(str::trim)
        .filter(|b| !b.is_empty())
    {
        a.push("--cookies-from-browser".into());
        a.push(browser.into());
    }
    a.push("--paths".into());
    a.push(download_dir.as_os_str().to_owned());
    a.push("--".into());
    a.push(url.into());
    a
}

/// One line of yt-dlp stdout, interpreted.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Line {
    Progress(FetchProgress),
    /// A post-processor (ExtractAudio, ...) started.
    Converting,
    File(PathBuf),
    Title(String),
    Other,
}

pub(crate) fn parse_line(line: &str) -> Line {
    let line = line.trim_end_matches(['\r', '\n']);
    if let Some(rest) = line.strip_prefix(PROGRESS_TAG) {
        return parse_progress(rest).map_or(Line::Other, Line::Progress);
    }
    if let Some(rest) = line.strip_prefix(POSTPROCESS_TAG) {
        let mut it = rest.split_whitespace();
        let status = it.next().unwrap_or("");
        let pp = it.next().unwrap_or("");
        // MoveFiles just renames the finished file; it isn't worth a "Converting" state.
        return if status == "started" && pp != "MoveFiles" {
            Line::Converting
        } else {
            Line::Other
        };
    }
    if let Some(rest) = line.strip_prefix(FILE_TAG) {
        if !rest.is_empty() && rest != "NA" {
            return Line::File(PathBuf::from(rest));
        }
        return Line::Other;
    }
    if let Some(rest) = line.strip_prefix(TITLE_TAG) {
        return match serde_json::from_str::<Option<String>>(rest) {
            Ok(Some(t)) => Line::Title(t),
            _ => Line::Other,
        };
    }
    Line::Other
}

/// `downloaded/total/estimate/fragment_index/fragment_count`, any of which may be `NA`.
fn parse_progress(s: &str) -> Option<FetchProgress> {
    let mut f = s.trim().split('/').map(|v| {
        let v = v.trim();
        // Some fields are floats ("12345.0") depending on the extractor.
        v.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0)
    });
    let downloaded = f.next()??;
    let total = f.next().flatten();
    let estimate = f.next().flatten();
    let frag_index = f.next().flatten();
    let frag_count = f.next().flatten();
    let fraction = total
        .or(estimate)
        .filter(|&t| t > 0.0)
        .map(|t| downloaded / t)
        .or_else(|| match (frag_index, frag_count) {
            (Some(i), Some(c)) if c > 0.0 => Some(i / c),
            _ => None,
        })
        .map(|x| x.clamp(0.0, 1.0) as f32);
    Some(FetchProgress::Downloading {
        fraction,
        bytes: downloaded as u64,
    })
}

/// The most useful error message in yt-dlp's stderr: the last `ERROR:` line.
pub(crate) fn error_message(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .rev()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("ERROR:"))
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
}

/// Run yt-dlp with `args`, translating its output into progress and the downloaded file.
pub(crate) fn run(
    ytdlp: &Path,
    args: &[OsString],
    cookies_hint: bool,
    progress: &mut dyn FnMut(FetchProgress),
    cancel: &AtomicBool,
) -> Result<Fetched> {
    tracing::debug!(?ytdlp, ?args, "running yt-dlp");
    let mut child = process::command(ytdlp)
        .args(args)
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| FetchError::YtDlp(format!("could not start {}: {e}", ytdlp.display())))?;

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (tx, rx) = mpsc::channel::<String>();
    let out_thread = std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match r.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tx.send(String::from_utf8_lossy(&buf).into_owned()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let err_thread = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = BufReader::new(stderr).read_to_end(&mut s);
        String::from_utf8_lossy(&s).into_owned()
    });

    let mut file = None;
    let mut title = None;
    let mut converting = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            process::kill_tree(&mut child);
            let _ = out_thread.join();
            let _ = err_thread.join();
            return Err(FetchError::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => match parse_line(&line) {
                Line::Progress(p) => progress(p),
                Line::Converting => {
                    if !converting {
                        converting = true;
                        progress(FetchProgress::Converting);
                    }
                }
                Line::File(p) => file = Some(p),
                Line::Title(t) => title = Some(t),
                Line::Other => {
                    let l = line.trim();
                    if !l.is_empty() {
                        tracing::trace!("yt-dlp: {l}");
                    }
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait()?;
    let _ = out_thread.join();
    let stderr = err_thread.join().unwrap_or_default();
    if !stderr.trim().is_empty() {
        tracing::debug!("yt-dlp stderr:\n{stderr}");
    }

    if !status.success() {
        let mut msg = error_message(&stderr)
            .or_else(|| {
                stderr
                    .lines()
                    .rev()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| format!("exited with {status}"));
        let lower = msg.to_ascii_lowercase();
        if cookies_hint
            && (lower.contains("sign in")
                || lower.contains("login")
                || lower.contains("log in")
                || lower.contains("cookies"))
        {
            msg.push_str(" (this may need a signed-in browser: set the cookies browser option)");
        }
        return Err(FetchError::YtDlp(msg));
    }
    let path = file.ok_or_else(|| {
        FetchError::YtDlp(
            error_message(&stderr).unwrap_or_else(|| "yt-dlp did not report a file".into()),
        )
    })?;
    if !path.is_file() {
        return Err(FetchError::YtDlp(format!(
            "yt-dlp reported {} but it does not exist",
            path.display()
        )));
    }
    let title = title
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| crate::names::title_for_path(&path));
    Ok(Fetched { path, title })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(a: &[OsString]) -> Vec<String> {
        a.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    fn value_after<'a>(a: &'a [String], flag: &str) -> Option<&'a str> {
        let i = a.iter().position(|s| s == flag)?;
        a.get(i + 1).map(String::as_str)
    }

    #[test]
    fn args_with_ffmpeg() {
        let ff = Path::new("/tools/ffmpeg");
        let a = strs(&args(
            "https://youtu.be/x",
            Path::new("/dl"),
            Some(ff),
            Some("firefox"),
        ));
        for flag in [
            "--no-playlist",
            "--newline",
            "--restrict-filenames",
            "-x",
            "--ignore-config",
            "--no-simulate",
            "--progress",
        ] {
            assert!(a.iter().any(|s| s == flag), "missing {flag}");
        }
        assert_eq!(value_after(&a, "--audio-format"), Some("wav"));
        assert_eq!(value_after(&a, "--ffmpeg-location"), Some("/tools"));
        assert_eq!(value_after(&a, "--cookies-from-browser"), Some("firefox"));
        assert_eq!(value_after(&a, "--paths"), Some("/dl"));
        assert_eq!(value_after(&a, "-o"), Some(OUTPUT_TEMPLATE));
        assert_eq!(value_after(&a, "-f"), Some("bestaudio/best"));
        assert!(
            a.iter()
                .any(|s| s.starts_with("after_move:") && s.contains("%(filepath)s"))
        );
        assert_eq!(&a[a.len() - 2..], ["--", "https://youtu.be/x"]);
    }

    #[test]
    fn args_without_ffmpeg() {
        let a = strs(&args(
            "https://x.com/a/status/1",
            Path::new("/dl"),
            None,
            None,
        ));
        assert_eq!(value_after(&a, "-f"), Some(NO_FFMPEG_FORMAT));
        assert!(!a.iter().any(|s| s == "-x"));
        assert!(!a.iter().any(|s| s == "--ffmpeg-location"));
        assert!(!a.iter().any(|s| s == "--cookies-from-browser"));
        let a = strs(&args("u", Path::new("/dl"), None, Some("  ")));
        assert!(!a.iter().any(|s| s == "--cookies-from-browser"));
    }

    #[test]
    fn parses_progress_lines() {
        assert_eq!(
            parse_line("STEMFETCH-PROGRESS 500/1000/NA/NA/NA\n"),
            Line::Progress(FetchProgress::Downloading {
                fraction: Some(0.5),
                bytes: 500
            })
        );
        assert_eq!(
            parse_line("STEMFETCH-PROGRESS 250/NA/1000.0/NA/NA\r\n"),
            Line::Progress(FetchProgress::Downloading {
                fraction: Some(0.25),
                bytes: 250
            })
        );
        assert_eq!(
            parse_line("STEMFETCH-PROGRESS 300/NA/NA/3/4"),
            Line::Progress(FetchProgress::Downloading {
                fraction: Some(0.75),
                bytes: 300
            })
        );
        assert_eq!(
            parse_line("STEMFETCH-PROGRESS 300/NA/NA/NA/NA"),
            Line::Progress(FetchProgress::Downloading {
                fraction: None,
                bytes: 300
            })
        );
        assert_eq!(
            parse_line("STEMFETCH-PROGRESS 2000/1000/NA/NA/NA"),
            Line::Progress(FetchProgress::Downloading {
                fraction: Some(1.0),
                bytes: 2000
            })
        );
        assert_eq!(parse_line("STEMFETCH-PROGRESS NA/NA/NA/NA/NA"), Line::Other);
        assert_eq!(parse_line("[download] 5.5% of 18KiB"), Line::Other);
    }

    #[test]
    fn parses_other_lines() {
        assert_eq!(
            parse_line("STEMFETCH-PP started ExtractAudio"),
            Line::Converting
        );
        assert_eq!(parse_line("STEMFETCH-PP started MoveFiles"), Line::Other);
        assert_eq!(
            parse_line("STEMFETCH-PP finished ExtractAudio"),
            Line::Other
        );
        assert_eq!(
            parse_line("STEMFETCH-FILE C:\\Users\\me\\Music\\a [b].wav\r\n"),
            Line::File(PathBuf::from("C:\\Users\\me\\Music\\a [b].wav"))
        );
        assert_eq!(
            parse_line(r#"STEMFETCH-TITLE "Beyonc\u00e9 \"Halo\" (Live)""#),
            Line::Title("Beyoncé \"Halo\" (Live)".into())
        );
        assert_eq!(parse_line("STEMFETCH-TITLE null"), Line::Other);
    }

    #[test]
    fn picks_last_error_line() {
        let stderr = "WARNING: something\nERROR: [youtube] abc: first\nnoise\n\
                      ERROR: [youtube] abc: Video unavailable\n";
        assert_eq!(
            error_message(stderr).as_deref(),
            Some("[youtube] abc: Video unavailable")
        );
        assert_eq!(error_message("WARNING: only a warning"), None);
    }
}
