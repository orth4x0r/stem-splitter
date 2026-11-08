//! Expanding playlist / album / set URLs into their entries with `yt-dlp --flat-playlist -J`,
//! so the app can queue every track.

use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::{FetchError, Result, process, ytdlp};

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistEntry {
    /// URL to hand back to [`crate::Fetcher::fetch`].
    pub url: String,
    pub title: Option<String>,
    pub duration_secs: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Playlist {
    pub title: Option<String>,
    pub entries: Vec<PlaylistEntry>,
}

/// True for URLs that name a whole list rather than one track: YouTube playlists
/// (`/playlist?list=...`, or `list=` without a `v=` video), SoundCloud sets, Bandcamp albums.
/// A YouTube video opened from inside a playlist (`watch?v=...&list=...`) counts as one track.
pub fn is_playlist_url(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url.trim()) else {
        return false;
    };
    let host = u.host_str().unwrap_or("").trim_start_matches("www.");
    let host = host.trim_start_matches("m.").trim_start_matches("music.");
    let path = u.path();
    let has = |k: &str| u.query_pairs().any(|(key, v)| key == k && !v.is_empty());
    match host {
        "youtube.com" => path.starts_with("/playlist") || (has("list") && !has("v")),
        "soundcloud.com" => path.contains("/sets/"),
        h if h.ends_with("bandcamp.com") => path.starts_with("/album/"),
        _ => false,
    }
}

/// Parse `yt-dlp --flat-playlist -J` output. A single video (no `entries`) becomes a one-entry
/// list with its own URL.
pub fn parse_flat_playlist(json: &str) -> Result<Playlist> {
    let v: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| FetchError::YtDlp(format!("bad playlist JSON: {e}")))?;
    let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_owned);
    let title = s(&v, "title");
    let Some(entries) = v.get("entries").and_then(|e| e.as_array()) else {
        let url = s(&v, "webpage_url")
            .or_else(|| s(&v, "original_url"))
            .or_else(|| s(&v, "url"))
            .ok_or_else(|| FetchError::YtDlp("yt-dlp listed no entries".into()))?;
        return Ok(Playlist {
            title: title.clone(),
            entries: vec![PlaylistEntry {
                url,
                title,
                duration_secs: v.get("duration").and_then(|d| d.as_f64()),
            }],
        });
    };
    let mut out = Vec::new();
    for e in entries {
        let id = s(e, "id");
        let ie = s(e, "ie_key").unwrap_or_default();
        let url = s(e, "webpage_url")
            .or_else(|| s(e, "url").filter(|u| u.contains("://")))
            .or_else(|| {
                // Older yt-dlp versions give YouTube entries as a bare id.
                id.filter(|_| ie.eq_ignore_ascii_case("youtube"))
                    .map(|id| format!("https://www.youtube.com/watch?v={id}"))
            });
        let Some(url) = url else { continue };
        let title = s(e, "title").filter(|t| {
            // Deleted / private YouTube videos keep their slot with a placeholder title.
            !matches!(t.as_str(), "[Deleted video]" | "[Private video]")
        });
        if title.is_none() && s(e, "title").is_some() {
            continue;
        }
        out.push(PlaylistEntry {
            url,
            title,
            duration_secs: e.get("duration").and_then(|d| d.as_f64()),
        });
    }
    if out.is_empty() {
        return Err(FetchError::YtDlp("the playlist is empty".into()));
    }
    Ok(Playlist {
        title,
        entries: out,
    })
}

/// Arguments for listing `url` without downloading anything.
pub(crate) fn args(url: &str, cookies_from_browser: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = [
        "--ignore-config",
        "--flat-playlist",
        "--dump-single-json",
        "--no-warnings",
        "--encoding",
        "utf-8",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(b) = cookies_from_browser.map(str::trim).filter(|b| !b.is_empty()) {
        a.push("--cookies-from-browser".into());
        a.push(b.into());
    }
    a.push("--".into());
    a.push(url.into());
    a
}

/// Run yt-dlp to list `url`'s entries. Stops (and kills yt-dlp) when `cancel` is set.
pub(crate) fn run(ytdlp: &Path, args: &[String], cancel: &AtomicBool) -> Result<Playlist> {
    let mut child = process::command(ytdlp)
        .args(args)
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| FetchError::YtDlp(format!("could not start {}: {e}", ytdlp.display())))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    // Read both pipes on threads so a big listing can't fill a pipe and stall yt-dlp.
    let out_t = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = stdout.read_to_end(&mut s);
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = stderr.read_to_end(&mut s);
        String::from_utf8_lossy(&s).into_owned()
    });
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            process::kill_tree(&mut child);
            let _ = out_t.join();
            let _ = err_t.join();
            return Err(FetchError::Cancelled);
        }
        if let Some(st) = child.try_wait()? {
            break st;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = out_t.join().unwrap_or_default();
    let err = err_t.join().unwrap_or_default();
    if !status.success() {
        return Err(FetchError::YtDlp(
            ytdlp::error_message(&err).unwrap_or_else(|| format!("exited with {status}")),
        ));
    }
    parse_flat_playlist(&String::from_utf8_lossy(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_playlist_urls() {
        for u in [
            "https://www.youtube.com/playlist?list=PL123",
            "https://music.youtube.com/playlist?list=OLAK5uy_x",
            "https://youtube.com/watch?list=PL123",
            "https://soundcloud.com/artist/sets/summer",
            "https://artist.bandcamp.com/album/first",
        ] {
            assert!(is_playlist_url(u), "{u}");
        }
        for u in [
            "https://www.youtube.com/watch?v=abc&list=PL123",
            "https://www.youtube.com/watch?v=abc",
            "https://youtu.be/abc",
            "https://soundcloud.com/artist/track",
            "https://artist.bandcamp.com/track/one",
            "https://x.com/a/status/1",
            "not a url",
        ] {
            assert!(!is_playlist_url(u), "{u}");
        }
    }

    #[test]
    fn parses_flat_youtube_playlist() {
        let json = r#"{"_type":"playlist","title":"Beats","entries":[
            {"_type":"url","ie_key":"Youtube","id":"aaa","url":"https://www.youtube.com/watch?v=aaa","title":"One","duration":61.0},
            {"_type":"url","ie_key":"Youtube","id":"bbb","url":"bbb","title":"Two"},
            {"_type":"url","ie_key":"Youtube","id":"ccc","url":"https://www.youtube.com/watch?v=ccc","title":"[Private video]"}
        ]}"#;
        let p = parse_flat_playlist(json).unwrap();
        assert_eq!(p.title.as_deref(), Some("Beats"));
        assert_eq!(p.entries.len(), 2);
        assert_eq!(p.entries[0].url, "https://www.youtube.com/watch?v=aaa");
        assert_eq!(p.entries[0].duration_secs, Some(61.0));
        assert_eq!(p.entries[1].url, "https://www.youtube.com/watch?v=bbb");
        assert_eq!(p.entries[1].title.as_deref(), Some("Two"));
    }

    #[test]
    fn parses_single_video_and_errors() {
        let p = parse_flat_playlist(
            r#"{"id":"x","title":"Solo","webpage_url":"https://www.youtube.com/watch?v=x"}"#,
        )
        .unwrap();
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.entries[0].url, "https://www.youtube.com/watch?v=x");
        assert!(parse_flat_playlist(r#"{"entries":[]}"#).is_err());
        assert!(parse_flat_playlist("nope").is_err());
    }

    #[test]
    fn args_end_with_url() {
        let a = args("https://y/p", Some("firefox"));
        assert!(a.contains(&"--flat-playlist".to_string()));
        assert!(a.windows(2).any(|w| w[0] == "--cookies-from-browser" && w[1] == "firefox"));
        assert_eq!(a.last().map(String::as_str), Some("https://y/p"));
    }
}
