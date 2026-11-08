//! URL classification and file-name helpers.

use std::path::{Path, PathBuf};

use url::Url;

/// Extensions we download directly instead of going through yt-dlp.
pub(crate) const DIRECT_AUDIO_EXTENSIONS: &[&str] = &[
    "wav", "flac", "mp3", "m4a", "aac", "ogg", "opus", "aiff", "aif",
];

/// Longest file name (in bytes, without extension) we create.
const MAX_STEM_BYTES: usize = 150;

/// What [`crate::Fetcher::fetch`] should do with its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// An existing file on disk (plain path or `file://` URL).
    Local(PathBuf),
    /// An http(s) link whose path ends in an audio extension.
    Direct(Url),
    /// Anything else: handed to yt-dlp.
    Page(String),
}

pub(crate) fn classify(input: &str) -> std::io::Result<Source> {
    let input = input.trim();
    if input.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty URL",
        ));
    }
    if input.len() >= 7 && input[..7].eq_ignore_ascii_case("file://") {
        let path = Url::parse(input)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("not a valid file URL: {input}"),
                )
            })?;
        return local(path);
    }
    if !input.contains("://") {
        let path = PathBuf::from(input);
        if path.exists() || looks_like_path(input) {
            return local(path);
        }
        // Bare "youtu.be/abc" or "ytsearch:..." style input: let yt-dlp have a go.
        return Ok(Source::Page(input.to_string()));
    }
    if is_direct_audio_url(input)
        && let Ok(url) = Url::parse(input)
    {
        return Ok(Source::Direct(url));
    }
    Ok(Source::Page(input.to_string()))
}

fn local(path: PathBuf) -> std::io::Result<Source> {
    if path.is_file() {
        Ok(Source::Local(path))
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("file not found: {}", path.display()),
        ))
    }
}

/// Input without a scheme that is clearly meant as a file path.
fn looks_like_path(s: &str) -> bool {
    let b = s.as_bytes();
    let drive =
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    drive
        || s.starts_with('/')
        || s.starts_with('\\')
        || s.starts_with("./")
        || s.starts_with("../")
        || s.starts_with(".\\")
        || s.starts_with("..\\")
        || s.starts_with('~')
        || Path::new(s).is_absolute()
}

/// True for http(s) URLs whose path ends in a known audio extension (query and fragment ignored).
pub(crate) fn is_direct_audio_url(url: &str) -> bool {
    let Ok(url) = Url::parse(url.trim()) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(last) = url.path_segments().and_then(|mut s| s.next_back()) else {
        return false;
    };
    let last = decode(last);
    match last.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => DIRECT_AUDIO_EXTENSIONS
            .iter()
            .any(|e| e.eq_ignore_ascii_case(ext)),
        _ => false,
    }
}

fn decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

/// Sanitized local file name (with extension) for a direct-download URL.
pub(crate) fn file_name_for_url(url: &Url) -> String {
    let last = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .map(decode)
        .unwrap_or_default();
    let (stem, ext) = match last.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), ext.to_ascii_lowercase()),
        _ => (last.clone(), String::new()),
    };
    let stem = sanitize_file_stem(&stem);
    let ext: String = ext.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if ext.is_empty() {
        stem
    } else {
        format!("{stem}.{ext}")
    }
}

/// Make `name` safe as a file name (without extension) on Windows, macOS and Linux.
pub fn sanitize_file_stem(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    // Windows silently drops trailing dots and spaces; leading ones are just confusing.
    out = out
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string();
    if out.len() > MAX_STEM_BYTES {
        let mut end = MAX_STEM_BYTES;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out = out.trim_end_matches(['.', ' ']).to_string();
    }
    if out.is_empty() {
        return "download".to_string();
    }
    if is_reserved_windows_name(&out) {
        out.insert(0, '_');
    }
    out
}

fn is_reserved_windows_name(stem: &str) -> bool {
    // "CON.tar" is reserved too: Windows looks at the part before the first dot.
    let base = stem.split('.').next().unwrap_or(stem).trim_end();
    let upper = base.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => {
            let b = upper.as_bytes();
            b.len() == 4
                && (upper.starts_with("COM") || upper.starts_with("LPT"))
                && (b'1'..=b'9').contains(&b[3])
        }
    }
}

/// `dir/name`, or `dir/name (2).ext` etc. if that already exists.
pub(crate) fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let first = dir.join(file_name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match file_name.rsplit_once('.') {
        Some((s, e)) => (s, format!(".{e}")),
        None => (file_name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("unbounded range")
}

/// Title shown for a local file: its stem.
pub(crate) fn title_for_path(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_audio_urls() {
        assert!(is_direct_audio_url("https://example.com/a/song.wav"));
        assert!(is_direct_audio_url("http://example.com/song.FLAC"));
        assert!(is_direct_audio_url(
            "https://cdn.example.com/x/y/track.mp3?sig=abc&exp=1#t=3"
        ));
        assert!(is_direct_audio_url("https://example.com/my%20song.m4a"));
        for ext in ["aac", "ogg", "opus", "aiff", "aif"] {
            assert!(
                is_direct_audio_url(&format!("https://e.com/s.{ext}")),
                "{ext}"
            );
        }
        assert!(!is_direct_audio_url(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        ));
        assert!(!is_direct_audio_url("https://x.com/user/status/123"));
        assert!(!is_direct_audio_url("https://soundcloud.com/a/b"));
        assert!(!is_direct_audio_url(
            "https://example.com/page?file=song.wav"
        ));
        assert!(!is_direct_audio_url("https://example.com/.wav"));
        assert!(!is_direct_audio_url("https://example.com/video.mp4"));
        assert!(!is_direct_audio_url("ftp://example.com/song.wav"));
        assert!(!is_direct_audio_url("song.wav"));
        assert!(!is_direct_audio_url(""));
    }

    #[test]
    fn classify_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("my beat.wav");
        std::fs::write(&f, b"x").unwrap();

        assert_eq!(
            classify(f.to_str().unwrap()).unwrap(),
            Source::Local(f.clone())
        );
        let file_url = Url::from_file_path(&f).unwrap();
        assert_eq!(
            classify(file_url.as_str()).unwrap(),
            Source::Local(f.clone())
        );
        assert_eq!(
            classify(&format!("  {}  ", f.display())).unwrap(),
            Source::Local(f.clone())
        );

        let missing = dir.path().join("nope.wav");
        let err = classify(missing.to_str().unwrap()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        let err = classify(Url::from_file_path(&missing).unwrap().as_str()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(classify("./nope.wav").is_err());
        assert!(classify("").is_err());

        assert!(matches!(
            classify("https://example.com/a.wav?x=1").unwrap(),
            Source::Direct(_)
        ));
        assert_eq!(
            classify("https://youtu.be/abc").unwrap(),
            Source::Page("https://youtu.be/abc".into())
        );
        assert_eq!(
            classify("youtu.be/abc").unwrap(),
            Source::Page("youtu.be/abc".into())
        );
    }

    #[test]
    fn sanitizing() {
        assert_eq!(
            sanitize_file_stem("a/b\\c:d*e?f\"g<h>i|j"),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(sanitize_file_stem("tab\there\n"), "tab_here_");
        assert_eq!(sanitize_file_stem("  name.  "), "name");
        assert_eq!(sanitize_file_stem("..."), "download");
        assert_eq!(sanitize_file_stem(""), "download");
        assert_eq!(sanitize_file_stem("CON"), "_CON");
        assert_eq!(sanitize_file_stem("com1"), "_com1");
        assert_eq!(sanitize_file_stem("lpt9.x"), "_lpt9.x");
        assert_eq!(sanitize_file_stem("COM10"), "COM10");
        assert_eq!(sanitize_file_stem("console"), "console");
        assert_eq!(sanitize_file_stem("Beyoncé – Halo"), "Beyoncé – Halo");
        let long = "é".repeat(200);
        let s = sanitize_file_stem(&long);
        assert!(s.len() <= MAX_STEM_BYTES);
        assert!(s.chars().all(|c| c == 'é'));
    }

    #[test]
    fn url_file_names() {
        let u = Url::parse("https://e.com/dir/My%20Song%3A%20Remix.WAV?a=b").unwrap();
        assert_eq!(file_name_for_url(&u), "My Song_ Remix.wav");
        let u = Url::parse("https://e.com/dir/con.mp3").unwrap();
        assert_eq!(file_name_for_url(&u), "_con.mp3");
        let u = Url::parse("https://e.com/").unwrap();
        assert_eq!(file_name_for_url(&u), "download");
    }

    #[test]
    fn unique_paths() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(unique_path(dir.path(), "a.wav"), dir.path().join("a.wav"));
        std::fs::write(dir.path().join("a.wav"), b"").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a.wav"),
            dir.path().join("a (2).wav")
        );
        std::fs::write(dir.path().join("a (2).wav"), b"").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a.wav"),
            dir.path().join("a (3).wav")
        );
    }
}
