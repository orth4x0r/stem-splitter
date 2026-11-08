//! Turn a URL into a local audio file.
//!
//! Direct links to audio files are downloaded with reqwest. Anything else (YouTube, X/Twitter,
//! SoundCloud, Bandcamp, TikTok, ...) goes through `yt-dlp`, which the app downloads and keeps
//! updated in its tools folder. ffmpeg is optional: with it we ask yt-dlp for WAV, without it we
//! take the best audio stream symphonia can decode (m4a/AAC, mp3).

mod http;
mod names;
mod process;
#[cfg(test)]
mod tests;
mod tools;
mod ytdlp;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use names::Source;
pub use names::sanitize_file_stem;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("http: {0}")]
    Http(String),
    #[error("yt-dlp: {0}")]
    YtDlp(String),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, FetchError>;

#[derive(Debug, Clone, PartialEq)]
pub enum FetchProgress {
    /// Setting up tools (e.g. downloading yt-dlp the first time).
    Preparing(String),
    /// `fraction` is `None` when the total size is unknown.
    Downloading { fraction: Option<f32>, bytes: u64 },
    /// Post-processing (yt-dlp extracting audio, ffmpeg converting).
    Converting,
}

#[derive(Debug, Clone)]
pub struct Fetched {
    pub path: PathBuf,
    /// Human-readable title (video title, or file name for direct links).
    pub title: String,
}

/// Where helper binaries live and where downloads land.
#[derive(Debug, Clone)]
pub struct Fetcher {
    pub tools_dir: PathBuf,
    pub download_dir: PathBuf,
    /// Passed to `yt-dlp --cookies-from-browser` (e.g. "firefox", "chrome", "edge"). Some X/Twitter
    /// and age-restricted YouTube posts need a signed-in browser session.
    pub cookies_from_browser: Option<String>,
}

/// For tool downloads, which have no cancel flag of their own.
static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);

impl Fetcher {
    pub fn new(tools_dir: PathBuf, download_dir: PathBuf) -> Self {
        Self {
            tools_dir,
            download_dir,
            cookies_from_browser: None,
        }
    }

    /// Download `url` and return the local audio file.
    ///
    /// Local paths and `file://` URLs are returned as they are. Direct links to audio files are
    /// downloaded over HTTP; everything else (and "audio" links that turn out to be web pages)
    /// goes through yt-dlp, which is downloaded first if needed.
    pub fn fetch(
        &self,
        url: &str,
        progress: &mut dyn FnMut(FetchProgress),
        cancel: &AtomicBool,
    ) -> Result<Fetched> {
        match names::classify(url)? {
            Source::Local(path) => Ok(Fetched {
                title: names::title_for_path(&path),
                path,
            }),
            Source::Direct(u) => match self.fetch_direct(&u, progress, cancel)? {
                Some(fetched) => Ok(fetched),
                None => {
                    tracing::info!("{u} is a web page, trying yt-dlp");
                    self.fetch_ytdlp(u.as_str(), progress, cancel)
                }
            },
            Source::Page(u) => self.fetch_ytdlp(&u, progress, cancel),
        }
    }

    /// Plain HTTP download. `Ok(None)` when the server answers with an HTML page instead of audio.
    fn fetch_direct(
        &self,
        url: &url::Url,
        progress: &mut dyn FnMut(FetchProgress),
        cancel: &AtomicBool,
    ) -> Result<Option<Fetched>> {
        progress(FetchProgress::Preparing("Connecting".into()));
        let client = http::client()?;
        let resp = http::get(&client, url.as_str())?;
        if http::content_type(&resp).is_some_and(|ct| ct == "text/html" || ct.contains("xhtml")) {
            return Ok(None);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(FetchError::Cancelled);
        }
        std::fs::create_dir_all(&self.download_dir)?;
        let dest = names::unique_path(&self.download_dir, &names::file_name_for_url(url));
        http::save(resp, &dest, progress, cancel)?;
        Ok(Some(Fetched {
            title: names::title_for_path(&dest),
            path: dest,
        }))
    }

    fn fetch_ytdlp(
        &self,
        url: &str,
        progress: &mut dyn FnMut(FetchProgress),
        cancel: &AtomicBool,
    ) -> Result<Fetched> {
        let ytdlp = self.ensure_ytdlp(progress)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(FetchError::Cancelled);
        }
        std::fs::create_dir_all(&self.download_dir)?;
        let ffmpeg = self.find_ffmpeg();
        let args = self.ytdlp_args(url, ffmpeg.as_deref());
        progress(FetchProgress::Preparing("Looking up media".into()));
        ytdlp::run(
            &ytdlp,
            &args,
            self.cookies_from_browser.is_none(),
            progress,
            cancel,
        )
    }

    /// The yt-dlp arguments (after the program name) [`Fetcher::fetch`] uses for `url`, given the
    /// ffmpeg it found (if any).
    pub fn ytdlp_args(&self, url: &str, ffmpeg: Option<&Path>) -> Vec<OsString> {
        ytdlp::args(
            url,
            &self.download_dir,
            ffmpeg,
            self.cookies_from_browser.as_deref(),
        )
    }

    /// Path to a working yt-dlp, downloading it into `tools_dir` if missing.
    pub fn ensure_ytdlp(&self, progress: &mut dyn FnMut(FetchProgress)) -> Result<PathBuf> {
        if let Some(p) = self.find_ytdlp() {
            return Ok(p);
        }
        progress(FetchProgress::Preparing(
            "Downloading yt-dlp (first run only)".into(),
        ));
        std::fs::create_dir_all(&self.tools_dir)?;
        let dest = self.tools_dir.join(process::exe_name("yt-dlp"));
        http::download_file(tools::ytdlp_download_url(), &dest, progress, &NEVER_CANCEL)?;
        process::make_executable(&dest)?;
        Ok(dest)
    }

    /// yt-dlp in `tools_dir` or on PATH, without downloading anything.
    pub fn find_ytdlp(&self) -> Option<PathBuf> {
        process::in_dir(&self.tools_dir, "yt-dlp").or_else(|| process::on_path("yt-dlp"))
    }

    /// Run `yt-dlp -U` (self-update). Sites change often, so the GUI exposes this.
    pub fn update_ytdlp(&self) -> Result<String> {
        let ytdlp = self.ensure_ytdlp(&mut |_| {})?;
        let out = process::command(&ytdlp)
            .args(["--ignore-config", "-U"])
            .env("PYTHONIOENCODING", "utf-8")
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| FetchError::YtDlp(format!("could not start {}: {e}", ytdlp.display())))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let combined = format!("{}\n{}", stdout.trim(), stderr.trim())
            .trim()
            .to_string();
        if out.status.success() {
            Ok(combined)
        } else {
            Err(FetchError::YtDlp(
                ytdlp::error_message(&stderr).unwrap_or(combined),
            ))
        }
    }

    /// ffmpeg on PATH or in `tools_dir`, if any.
    pub fn find_ffmpeg(&self) -> Option<PathBuf> {
        process::in_dir(&self.tools_dir, "ffmpeg").or_else(|| process::on_path("ffmpeg"))
    }

    /// ffmpeg, installing a static build into `tools_dir` if there is none (Windows only; on
    /// other systems install it with the package manager). With ffmpeg, yt-dlp can convert any
    /// stream (Opus/WebM included) to WAV.
    pub fn ensure_ffmpeg(&self, progress: &mut dyn FnMut(FetchProgress)) -> Result<PathBuf> {
        if let Some(p) = self.find_ffmpeg() {
            return Ok(p);
        }
        if !cfg!(windows) {
            return Err(FetchError::Io(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "ffmpeg not found; install it with your package manager",
            )));
        }
        progress(FetchProgress::Preparing("Downloading ffmpeg".into()));
        std::fs::create_dir_all(&self.tools_dir)?;
        let zip = self.tools_dir.join("ffmpeg-download.zip");
        http::download_file(tools::FFMPEG_WINDOWS_ZIP, &zip, progress, &NEVER_CANCEL)?;
        progress(FetchProgress::Preparing("Unpacking ffmpeg".into()));
        let result = tools::extract_ffmpeg_zip(&zip, &self.tools_dir);
        let _ = std::fs::remove_file(&zip);
        result
    }
}

/// Local file paths (and `file://` URLs) are passed straight through by [`Fetcher::fetch`].
/// True when the URL obviously points at an audio file (by extension or content-type).
pub fn is_direct_audio_url(url: &str) -> bool {
    names::is_direct_audio_url(url)
}
