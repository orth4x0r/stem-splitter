//! Searching YouTube / SoundCloud by text through yt-dlp's `ytsearchN:` / `scsearchN:` pseudo
//! URLs, listed flat (`--flat-playlist -J`) so nothing is downloaded and one request is enough.
//!
//! Also decides whether text typed into the URL box is a link, a file path or a search query.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{FetchError, Result, playlist};

/// How many results one search asks for.
pub const SEARCH_LIMIT: usize = 20;

/// Where to search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SearchSite {
    #[default]
    YouTube,
    SoundCloud,
}

impl SearchSite {
    pub const ALL: [SearchSite; 2] = [SearchSite::YouTube, SearchSite::SoundCloud];

    pub fn label(self) -> &'static str {
        match self {
            SearchSite::YouTube => "YouTube",
            SearchSite::SoundCloud => "SoundCloud",
        }
    }

    /// yt-dlp's search prefix (without the count).
    fn prefix(self) -> &'static str {
        match self {
            SearchSite::YouTube => "ytsearch",
            SearchSite::SoundCloud => "scsearch",
        }
    }
}

/// One search hit.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    /// URL to hand to [`crate::Fetcher::fetch`].
    pub url: String,
    pub title: String,
    /// Channel / uploader / artist, when yt-dlp lists one.
    pub uploader: Option<String>,
    pub duration_secs: Option<f64>,
    pub views: Option<u64>,
}

/// What the text in the URL box is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Empty,
    /// One or more links (with or without `https://`).
    Url,
    /// A file on disk, or something written like a path.
    Path,
    /// Anything else: words to search for.
    Query,
}

/// Classify text typed into the URL box. Links are anything with a scheme, or text whose
/// every word looks like `host.tld/...` (the queue accepts several links at once); paths are
/// existing files or text written like a path (`C:\...`, `/...`, `~/...`). The rest is a
/// search query, e.g. `dilla donuts`, `vol.2 breaks` or `soul sample 1972`.
pub fn classify_input(text: &str) -> InputKind {
    let t = text.trim().trim_matches('"').trim();
    if t.is_empty() {
        return InputKind::Empty;
    }
    if t.contains("://") {
        return InputKind::Url;
    }
    if crate::names::looks_like_path(t) || Path::new(t).is_file() {
        return InputKind::Path;
    }
    if t.split_whitespace().all(looks_like_host) {
        return InputKind::Url;
    }
    InputKind::Query
}

/// `youtube.com/watch?v=x`, `www.x.com`, `youtu.be/abc`: a dotted host whose last label is a
/// plausible top-level domain (letters only, 2+), so `vol.2` or `1.5x` stay words.
fn looks_like_host(tok: &str) -> bool {
    let tok = tok.trim_matches(['"', '\'', ',', ';', '<', '>']);
    let host = tok.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("");
    let Some((name, tld)) = host.rsplit_once('.') else {
        return false;
    };
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && (2..=24).contains(&tld.len())
        && tld.chars().all(|c| c.is_ascii_alphabetic())
}

/// The pseudo URL yt-dlp searches with, e.g. `ytsearch20:dilla donuts`. Whitespace is collapsed;
/// `None` for an empty query.
pub fn search_target(site: SearchSite, query: &str, limit: usize) -> Option<String> {
    let q = query.split_whitespace().collect::<Vec<_>>().join(" ");
    if q.is_empty() {
        return None;
    }
    Some(format!("{}{}:{q}", site.prefix(), limit.clamp(1, 100)))
}

/// Parse `yt-dlp --flat-playlist -J "ytsearchN:..."` (or `scsearchN:`) output.
pub fn parse_search_results(json: &str) -> Result<Vec<SearchResult>> {
    let v: Value = serde_json::from_str(json)
        .map_err(|e| FetchError::YtDlp(format!("bad search JSON: {e}")))?;
    let entries = v
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::YtDlp("yt-dlp listed no search results".into()))?;
    Ok(entries.iter().filter_map(entry).collect())
}

fn entry(e: &Value) -> Option<SearchResult> {
    let s = |k: &str| {
        e.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_owned)
    };
    // Live streams can't be split; channels and playlists can show up in some extractors.
    if e.get("live_status").and_then(Value::as_str) == Some("is_live") {
        return None;
    }
    let ie = s("ie_key").unwrap_or_default();
    if ie.eq_ignore_ascii_case("YoutubeTab") {
        return None;
    }
    let url = s("webpage_url")
        .or_else(|| s("url").filter(|u| u.contains("://")))
        .or_else(|| {
            s("id")
                .filter(|_| ie.eq_ignore_ascii_case("youtube"))
                .map(|id| format!("https://www.youtube.com/watch?v={id}"))
        })?;
    let title = s("title")?;
    if matches!(title.as_str(), "[Deleted video]" | "[Private video]") {
        return None;
    }
    let uploader = s("channel")
        .or_else(|| s("uploader"))
        .or_else(|| s("artist"))
        .or_else(|| soundcloud_user(&url));
    let duration_secs = e
        .get("duration")
        .and_then(Value::as_f64)
        .filter(|d| d.is_finite() && *d > 0.0);
    let views = e.get("view_count").and_then(Value::as_u64);
    Some(SearchResult {
        url,
        title,
        uploader,
        duration_secs,
        views,
    })
}

/// `https://soundcloud.com/some-artist/track` -> `some-artist` (flat SoundCloud results often
/// carry no uploader field).
fn soundcloud_user(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let host = u
        .host_str()?
        .trim_start_matches("www.")
        .trim_start_matches("m.");
    if host != "soundcloud.com" {
        return None;
    }
    u.path_segments()?
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Run a search with yt-dlp (the flat listing arguments from [`playlist::args`]).
pub(crate) fn run(
    ytdlp: &Path,
    site: SearchSite,
    query: &str,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<Vec<SearchResult>> {
    let target = search_target(site, query, limit)
        .ok_or_else(|| FetchError::YtDlp("type something to search for".into()))?;
    let args = playlist::args(&target, None);
    parse_search_results(&playlist::run_json(ytdlp, &args, cancel)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_links_paths_and_queries() {
        use InputKind::*;
        let cases = [
            ("", Empty),
            ("   ", Empty),
            ("https://www.youtube.com/watch?v=abc", Url),
            ("http://a.b/c.mp3", Url),
            ("youtube.com/watch?v=b", Url),
            ("youtu.be/abc", Url),
            ("soundcloud.com/artist/track", Url),
            ("www.example.org", Url),
            ("a.com/x b.com/y", Url),
            ("file:///tmp/x.wav", Url),
            ("C:\\Users\\me\\Music\\song.wav", Path),
            ("C:/Music/song.wav", Path),
            ("/home/me/song.wav", Path),
            ("~/Music/song.wav", Path),
            ("./song.wav", Path),
            ("dilla donuts", Query),
            ("soul sample 1972", Query),
            ("amen break", Query),
            ("vol.2 breaks", Query),
            ("dusty breaks vol.2", Query),
            ("1.5x speed", Query),
            ("lofi", Query),
            ("youtube.com dilla", Query),
            ("ytsearch:dilla", Query),
        ];
        for (text, want) in cases {
            assert_eq!(classify_input(text), want, "{text:?}");
        }
    }

    #[test]
    fn existing_files_are_paths() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("my song.wav");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(classify_input(&f.display().to_string()), InputKind::Path);
    }

    #[test]
    fn builds_search_targets() {
        assert_eq!(
            search_target(SearchSite::YouTube, "  dilla   donuts ", 20).as_deref(),
            Some("ytsearch20:dilla donuts")
        );
        assert_eq!(
            search_target(SearchSite::SoundCloud, "soul flip", 5).as_deref(),
            Some("scsearch5:soul flip")
        );
        assert_eq!(search_target(SearchSite::YouTube, "  ", 20), None);
        // The target goes after `--`, so a query starting with `-` is not an option.
        let a = playlist::args(&search_target(SearchSite::YouTube, "-x", 20).unwrap(), None);
        assert_eq!(a[a.len() - 2], "--");
        assert!(a.contains(&"--flat-playlist".to_string()));
    }

    /// Shaped like `yt-dlp --flat-playlist --dump-single-json "ytsearch5:amen break"` output
    /// (same keys and nesting, values edited; YouTube is unreachable from the build sandbox),
    /// with the awkward cases mixed in: a live stream, a channel, a bare-id url, null fields.
    const YT_JSON: &str = include_str!("testdata/ytsearch-flat.json");
    /// The same for `scsearch4:soul sample`; flat SoundCloud entries carry less metadata.
    const SC_JSON: &str = include_str!("testdata/scsearch-flat.json");

    #[test]
    fn parses_youtube_search() {
        let r = parse_search_results(YT_JSON).unwrap();
        // 5 entries: one live stream and one channel are dropped.
        assert_eq!(r.len(), 3, "{r:#?}");
        assert_eq!(r[0].url, "https://www.youtube.com/watch?v=GxZuq57_bYM");
        assert_eq!(
            r[0].title,
            "The Winstons - Amen, Brother (Original Amen Break)"
        );
        assert_eq!(r[0].uploader.as_deref(), Some("Break Archive"));
        assert_eq!(r[0].duration_secs, Some(158.0));
        assert_eq!(r[0].views, Some(4_812_337));
        // Uploader falls back from channel to uploader; bare ids become watch URLs.
        assert_eq!(r[1].uploader.as_deref(), Some("drumtrax"));
        assert_eq!(r[2].url, "https://www.youtube.com/watch?v=qwQLk7NcpO4");
        assert_eq!(r[2].duration_secs, None);
    }

    #[test]
    fn parses_soundcloud_search() {
        let r = parse_search_results(SC_JSON).unwrap();
        assert_eq!(r.len(), 4);
        assert_eq!(
            r[0].url,
            "https://soundcloud.com/crate-digger/soul-sample-1972"
        );
        assert_eq!(r[0].uploader.as_deref(), Some("Crate Digger"));
        assert_eq!(r[0].duration_secs, Some(94.524));
        // No uploader field: taken from the permalink.
        assert_eq!(r[1].uploader.as_deref(), Some("beatsbyjm"));
        assert_eq!(r[1].duration_secs, None);
        assert_eq!(r[3].title, "Dusty soul loop (prod. ✦ kei)");
    }

    #[test]
    fn empty_and_bad_search_output() {
        assert_eq!(
            parse_search_results(r#"{"_type":"playlist","entries":[]}"#).unwrap(),
            vec![]
        );
        assert!(parse_search_results(r#"{"id":"x"}"#).is_err());
        assert!(parse_search_results("ERROR: nope").is_err());
    }
}
