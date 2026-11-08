//! Where the app keeps its files.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Models and the ONNX Runtime library.
    pub models: PathBuf,
    /// yt-dlp (and ffmpeg if the user drops one in).
    pub tools: PathBuf,
    /// Raw downloads from links.
    pub downloads: PathBuf,
    /// Root of everything above.
    pub data: PathBuf,
}

impl AppPaths {
    pub fn detect() -> Self {
        let data = directories::ProjectDirs::from("com", "orth4x0r", "Stem Splitter")
            .map(|d| d.data_local_dir().to_path_buf())
            .unwrap_or_else(|| std::env::temp_dir().join("stem-splitter"));
        Self {
            models: data.join("models"),
            tools: data.join("tools"),
            downloads: data.join("downloads"),
            data,
        }
    }
}

/// `Music/Stem Splitter`, or the home folder / current dir as fallbacks.
pub fn default_output_dir() -> PathBuf {
    let dirs = directories::UserDirs::new();
    let base = dirs
        .as_ref()
        .and_then(|d| d.audio_dir().map(Path::to_path_buf))
        .or_else(|| dirs.as_ref().map(|d| d.home_dir().join("Music")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Stem Splitter")
}

/// Make a string safe to use as a file or folder name on Windows.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']);
    let mut out: String = trimmed.chars().take(120).collect();
    if out.is_empty() {
        out = "untitled".into();
    }
    // Reserved device names.
    let upper = out.to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"];
    if reserved.contains(&upper.as_str())
        || ((upper.starts_with("COM") || upper.starts_with("LPT")) && upper.len() == 4)
    {
        out.push('_');
    }
    out
}

/// Open a folder in Explorer / Finder / the file manager.
pub fn open_in_file_manager(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program)
        .arg(path)
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes() {
        assert_eq!(sanitize_file_name("a/b:c?"), "a_b_c_");
        assert_eq!(sanitize_file_name("  song.  "), "song");
        assert_eq!(sanitize_file_name("CON"), "CON_");
        assert_eq!(sanitize_file_name(""), "untitled");
    }
}
