//! Where the app keeps its files.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppPaths {
    /// `ModelManager` root: it keeps `models/` and `runtime/` (onnxruntime) under it.
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
            models: data.clone(),
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

/// Total size of the files under `dir` (0 if it doesn't exist). Symlinks are not followed.
pub fn dir_size(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

/// Delete everything inside `dir` (keeping `dir` itself). Returns the bytes freed. Files that
/// can't be removed (e.g. still open) are skipped; the error is returned only if nothing could
/// be removed at all.
pub fn clear_dir(dir: &Path) -> std::io::Result<u64> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(0);
    };
    let mut freed = 0;
    let mut first_err = None;
    let mut removed_any = false;
    for e in rd.flatten() {
        let p = e.path();
        let (size, res) = match e.file_type() {
            Ok(t) if t.is_dir() => (dir_size(&p), std::fs::remove_dir_all(&p)),
            _ => (
                e.metadata().map_or(0, |m| m.len()),
                std::fs::remove_file(&p),
            ),
        };
        match res {
            Ok(()) => {
                freed += size;
                removed_any = true;
            }
            Err(err) => {
                first_err.get_or_insert(err);
            }
        }
    }
    match first_err {
        Some(e) if !removed_any => Err(e),
        _ => Ok(freed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_and_clears_a_folder() {
        let dir = std::env::temp_dir().join(format!("stem-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.bin"), [0u8; 100]).unwrap();
        std::fs::write(dir.join("sub/b.bin"), [0u8; 50]).unwrap();
        assert_eq!(dir_size(&dir), 150);
        assert_eq!(clear_dir(&dir).unwrap(), 150);
        assert!(dir.is_dir());
        assert_eq!(dir_size(&dir), 0);
        assert_eq!(clear_dir(&dir.join("missing")).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitizes() {
        assert_eq!(sanitize_file_name("a/b:c?"), "a_b_c_");
        assert_eq!(sanitize_file_name("  song.  "), "song");
        assert_eq!(sanitize_file_name("CON"), "CON_");
        assert_eq!(sanitize_file_name(""), "untitled");
    }
}
