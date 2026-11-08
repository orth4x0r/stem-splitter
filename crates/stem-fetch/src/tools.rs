//! Locating and installing helper binaries (yt-dlp, ffmpeg).

use std::fs::File;
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

use crate::{FetchError, Result, http};

/// Official yt-dlp release asset for this platform.
pub(crate) fn ytdlp_download_url() -> &'static str {
    if cfg!(windows) {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
    } else if cfg!(target_os = "macos") {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux_aarch64"
    } else {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux"
    }
}

/// Static Windows ffmpeg build (GPL; it's a separate program we run, never link).
pub(crate) const FFMPEG_WINDOWS_ZIP: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";

/// Extract `bin/ffmpeg(.exe)` and `bin/ffprobe(.exe)` from an ffmpeg build zip into `dest`.
/// Returns the path of ffmpeg.
pub(crate) fn extract_ffmpeg_zip(zip_path: &Path, dest: &Path) -> Result<PathBuf> {
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(zip_err)?;
    let wanted = [
        crate::process::exe_name("ffmpeg"),
        crate::process::exe_name("ffprobe"),
    ];
    std::fs::create_dir_all(dest)?;
    let mut ffmpeg = None;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(zip_err)?;
        if !entry.is_file() {
            continue;
        }
        // Only the file name is used, so odd paths in the archive can't escape `dest`.
        let name = entry.name().map_err(zip_err)?.replace('\\', "/");
        let mut parts = name.rsplit('/');
        let file_name = parts.next().unwrap_or("");
        let parent = parts.next().unwrap_or("");
        if parent != "bin" || !wanted.iter().any(|w| w.eq_ignore_ascii_case(file_name)) {
            continue;
        }
        let target = dest.join(file_name.to_ascii_lowercase());
        let part = http::part_path(&target);
        {
            let mut out = BufWriter::new(File::create(&part)?);
            io::copy(&mut entry, &mut out)?;
            io::Write::flush(&mut out)?;
        }
        std::fs::rename(&part, &target)?;
        crate::process::make_executable(&target)?;
        if file_name.eq_ignore_ascii_case(&wanted[0]) {
            ffmpeg = Some(target);
        }
    }
    ffmpeg.ok_or_else(|| {
        FetchError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            "ffmpeg not found in the downloaded archive",
        ))
    })
}

fn zip_err(e: zip::result::ZipError) -> FetchError {
    FetchError::Io(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("zip: {e}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn extracts_ffmpeg_from_zip() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("ff.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            let root = "ffmpeg-master-latest-win64-gpl";
            for (name, body) in [
                (format!("{root}/LICENSE.txt"), "license"),
                (
                    format!("{root}/bin/{}", crate::process::exe_name("ffmpeg")),
                    "FF",
                ),
                (
                    format!("{root}/bin/{}", crate::process::exe_name("ffprobe")),
                    "FP",
                ),
                (
                    format!("{root}/bin/{}", crate::process::exe_name("ffplay")),
                    "PL",
                ),
                (
                    format!("{root}/doc/{}", crate::process::exe_name("ffmpeg")),
                    "no",
                ),
            ] {
                w.start_file(name, opts).unwrap();
                w.write_all(body.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        let tools = dir.path().join("tools");
        let ff = extract_ffmpeg_zip(&zip_path, &tools).unwrap();
        assert_eq!(ff, tools.join(crate::process::exe_name("ffmpeg")));
        assert_eq!(std::fs::read_to_string(&ff).unwrap(), "FF");
        let fp = tools.join(crate::process::exe_name("ffprobe"));
        assert_eq!(std::fs::read_to_string(fp).unwrap(), "FP");
        assert!(!tools.join(crate::process::exe_name("ffplay")).exists());
        assert!(crate::process::in_dir(&tools, "ffmpeg").is_some());
    }

    #[test]
    fn zip_without_ffmpeg_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("empty.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            w.start_file("readme.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(b"hi").unwrap();
            w.finish().unwrap();
        }
        assert!(extract_ffmpeg_zip(&zip_path, dir.path()).is_err());
    }
}
