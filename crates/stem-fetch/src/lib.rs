#![allow(unused_variables)] // TODO: remove once stubs are implemented
//! Turn a URL into a local audio file.
//!
//! Direct links to audio files are downloaded with reqwest. Anything else (YouTube, X/Twitter,
//! SoundCloud, Bandcamp, TikTok, ...) goes through `yt-dlp`, which the app downloads and keeps
//! updated in its tools folder. ffmpeg is optional: with it we ask yt-dlp for WAV, without it we
//! take the best audio stream symphonia can decode (m4a/AAC, mp3).

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

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
}

impl Fetcher {
    pub fn new(tools_dir: PathBuf, download_dir: PathBuf) -> Self {
        Self {
            tools_dir,
            download_dir,
        }
    }

    /// Download `url` and return the local audio file.
    pub fn fetch(
        &self,
        url: &str,
        progress: &mut dyn FnMut(FetchProgress),
        cancel: &AtomicBool,
    ) -> Result<Fetched> {
        todo!("stem-fetch")
    }

    /// Path to a working yt-dlp, downloading it into `tools_dir` if missing.
    pub fn ensure_ytdlp(&self, progress: &mut dyn FnMut(FetchProgress)) -> Result<PathBuf> {
        todo!("stem-fetch")
    }

    /// Run `yt-dlp -U` (self-update). Sites change often, so the GUI exposes this.
    pub fn update_ytdlp(&self) -> Result<String> {
        todo!("stem-fetch")
    }

    /// ffmpeg on PATH or in `tools_dir`, if any.
    pub fn find_ffmpeg(&self) -> Option<PathBuf> {
        todo!("stem-fetch")
    }
}

/// True when the URL obviously points at an audio file (by extension or content-type).
pub fn is_direct_audio_url(url: &str) -> bool {
    todo!("stem-fetch")
}
