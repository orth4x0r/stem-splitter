//! End-to-end tests against a tiny HTTP server on 127.0.0.1 (no internet needed).
//! Tests that need yt-dlp or ffmpeg skip themselves when those aren't installed.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::*;

struct Route {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    /// Pause between 16 KiB chunks, to simulate a slow server.
    slow: bool,
}

/// Serves fixed responses by path until the test process exits. Returns the base URL.
fn serve(routes: Vec<(&'static str, Route)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let routes: Arc<HashMap<&'static str, Route>> = Arc::new(routes.into_iter().collect());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let routes = routes.clone();
            std::thread::spawn(move || {
                let _ = handle(stream, &routes);
            });
        }
    });
    format!("http://{addr}")
}

fn handle(mut stream: TcpStream, routes: &HashMap<&'static str, Route>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let path = path.split('?').next().unwrap_or(path);
    let not_found = Route {
        status: 404,
        content_type: "text/plain",
        body: b"not found".to_vec(),
        slow: false,
    };
    let route = routes.get(path).unwrap_or(&not_found);
    let reason = if route.status == 200 { "OK" } else { "Error" };
    write!(
        stream,
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        route.status,
        route.content_type,
        route.body.len()
    )?;
    if method != "HEAD" {
        // Write in pieces so the client sees several chunks.
        for chunk in route.body.chunks(16 * 1024) {
            stream.write_all(chunk)?;
            if route.slow {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    stream.flush()
}

/// A valid 16-bit mono WAV of `samples` samples of a quiet ramp.
fn wav_bytes(samples: u32) -> Vec<u8> {
    let data_len = samples * 2;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&1u16.to_le_bytes()); // mono
    v.extend_from_slice(&44_100u32.to_le_bytes());
    v.extend_from_slice(&(44_100u32 * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..samples {
        v.extend_from_slice(&(((i % 200) as i16 - 100) * 50).to_le_bytes());
    }
    v
}

fn wav_route(body: Vec<u8>) -> Route {
    Route {
        status: 200,
        content_type: "audio/wav",
        body,
        slow: false,
    }
}

fn fetcher(dir: &Path) -> Fetcher {
    Fetcher::new(dir.join("tools"), dir.join("downloads"))
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// yt-dlp is only on PATH where someone installed it (`pip install yt-dlp`).
fn have_ytdlp(f: &Fetcher) -> bool {
    let ok = f.find_ytdlp().is_some();
    if !ok {
        eprintln!("yt-dlp not installed; skipping");
    }
    ok
}

#[test]
fn local_files_pass_through() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Drum Loop 90bpm.wav");
    std::fs::write(&file, wav_bytes(10)).unwrap();
    let f = fetcher(dir.path());
    let never = AtomicBool::new(false);
    let mut events = Vec::new();

    let got = f
        .fetch(file.to_str().unwrap(), &mut |p| events.push(p), &never)
        .unwrap();
    assert_eq!(got.path, file);
    assert_eq!(got.title, "Drum Loop 90bpm");

    let file_url = url::Url::from_file_path(&file).unwrap();
    let got = f.fetch(file_url.as_str(), &mut |_| {}, &never).unwrap();
    assert_eq!(got.path, file);
    assert!(events.is_empty());
    assert!(!dir.path().join("downloads").exists(), "nothing copied");

    let err = f
        .fetch(
            dir.path().join("missing.wav").to_str().unwrap(),
            &mut |_| {},
            &never,
        )
        .unwrap_err();
    assert!(matches!(err, FetchError::Io(_)), "{err:?}");
}

#[test]
fn direct_download() {
    let body = wav_bytes(100_000);
    let base = serve(vec![("/files/My%20Track%3F.wav", wav_route(body.clone()))]);
    let dir = tempfile::tempdir().unwrap();
    let f = fetcher(dir.path());
    let never = AtomicBool::new(false);
    let mut events = Vec::new();

    let url = format!("{base}/files/My%20Track%3F.wav?token=abc");
    let got = f.fetch(&url, &mut |p| events.push(p), &never).unwrap();
    let downloads = dir.path().join("downloads");
    assert_eq!(got.path, downloads.join("My Track_.wav"));
    assert_eq!(got.title, "My Track_");
    assert_eq!(std::fs::read(&got.path).unwrap(), body);
    assert_eq!(files_in(&downloads), ["My Track_.wav"], "no .part left");
    assert_eq!(
        events.last(),
        Some(&FetchProgress::Downloading {
            fraction: Some(1.0),
            bytes: body.len() as u64
        })
    );

    // Same URL again: a second copy instead of overwriting the first.
    let again = f.fetch(&url, &mut |_| {}, &never).unwrap();
    assert_eq!(again.path, downloads.join("My Track_ (2).wav"));
    assert!(!dir.path().join("tools").exists(), "yt-dlp not needed");
}

#[test]
fn direct_download_cancel_removes_part_file() {
    let base = serve(vec![("/big.wav", wav_route(wav_bytes(2_000_000)))]);
    let dir = tempfile::tempdir().unwrap();
    let f = fetcher(dir.path());
    let cancel = AtomicBool::new(false);
    let err = f
        .fetch(
            &format!("{base}/big.wav"),
            &mut |p| {
                if let FetchProgress::Downloading { bytes, .. } = p
                    && bytes == 0
                {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, FetchError::Cancelled), "{err:?}");
    assert!(files_in(&dir.path().join("downloads")).is_empty());
}

#[test]
fn direct_download_http_error() {
    let base = serve(vec![]);
    let dir = tempfile::tempdir().unwrap();
    let err = fetcher(dir.path())
        .fetch(
            &format!("{base}/missing.mp3"),
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
    match err {
        FetchError::Http(msg) => assert!(msg.contains("404"), "{msg}"),
        e => panic!("{e:?}"),
    }
}

#[test]
fn ytdlp_generic_url() {
    let dir = tempfile::tempdir().unwrap();
    let f = fetcher(dir.path());
    if !have_ytdlp(&f) {
        return;
    }
    // No audio extension in the path, so this goes to yt-dlp's generic extractor.
    let body = wav_bytes(44_100);
    let base = serve(vec![("/stream/track", wav_route(body.clone()))]);
    let mut events = Vec::new();
    let got = f
        .fetch(
            &format!("{base}/stream/track"),
            &mut |p| events.push(p),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(got.path.is_file(), "{got:?}");
    assert!(
        got.path.starts_with(dir.path().join("downloads")),
        "{got:?}"
    );
    assert_eq!(got.title, "track");
    if f.find_ffmpeg().is_some() {
        assert_eq!(got.path.extension().unwrap(), "wav");
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, FetchProgress::Downloading { .. })),
        "{events:?}"
    );
}

#[test]
fn ytdlp_without_ffmpeg_keeps_original_stream() {
    let dir = tempfile::tempdir().unwrap();
    let f = fetcher(dir.path());
    if !have_ytdlp(&f) {
        return;
    }
    let body = wav_bytes(44_100);
    let base = serve(vec![("/x/clip", wav_route(body.clone()))]);
    let ytdlp = f.find_ytdlp().unwrap();
    let args = f.ytdlp_args(&format!("{base}/x/clip"), None);
    // The generic extractor's single "format" is not m4a/mp3/mp4, so the final `bestaudio`
    // fallback picks it and the bytes arrive untouched.
    let got = crate::ytdlp::run(&ytdlp, &args, true, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(std::fs::read(&got.path).unwrap(), body);
}

#[test]
fn html_behind_audio_extension_falls_back_to_ytdlp() {
    let dir = tempfile::tempdir().unwrap();
    let f = fetcher(dir.path());
    if !have_ytdlp(&f) {
        return;
    }
    let page = b"<html><head><title>Cool Song</title></head><body>\
        <audio src=\"/media/cool\" type=\"audio/wav\"></audio></body></html>"
        .to_vec();
    let base = serve(vec![
        (
            "/song.mp3",
            Route {
                status: 200,
                content_type: "text/html; charset=utf-8",
                body: page,
                slow: false,
            },
        ),
        ("/media/cool", wav_route(wav_bytes(44_100))),
    ]);
    let result = f.fetch(
        &format!("{base}/song.mp3"),
        &mut |_| {},
        &AtomicBool::new(false),
    );
    eprintln!("{result:?}");
    // Either yt-dlp found the <audio> element, or it tried and reported why it couldn't; what
    // matters is that we didn't save the HTML page as "song.mp3".
    match result {
        Ok(got) => assert_ne!(got.path.file_name().unwrap(), "song.mp3"),
        Err(FetchError::YtDlp(_)) => {}
        Err(e) => panic!("{e:?}"),
    }
    assert!(!dir.path().join("downloads/song.mp3").exists());
}

#[test]
fn ytdlp_error_message_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let f = fetcher(dir.path());
    if !have_ytdlp(&f) {
        return;
    }
    let base = serve(vec![]);
    let err = f
        .fetch(
            &format!("{base}/nothing-here"),
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
    match err {
        FetchError::YtDlp(msg) => assert!(msg.contains("404"), "{msg}"),
        e => panic!("{e:?}"),
    }
}

#[test]
fn ytdlp_cancel_kills_download() {
    let dir = tempfile::tempdir().unwrap();
    let f = fetcher(dir.path());
    if !have_ytdlp(&f) {
        return;
    }
    // ~6 MB at 16 KiB per 50 ms would take ~20 s.
    let base = serve(vec![(
        "/slow/stream",
        Route {
            slow: true,
            ..wav_route(wav_bytes(3_000_000))
        },
    )]);
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let err = f
        .fetch(
            &format!("{base}/slow/stream"),
            &mut |p| {
                if matches!(p, FetchProgress::Downloading { bytes, .. } if bytes > 0) {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, FetchError::Cancelled), "{err:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(15));
}

#[test]
fn ytdlp_cancel_before_start() {
    let dir = tempfile::tempdir().unwrap();
    let f = fetcher(dir.path());
    if !have_ytdlp(&f) {
        return;
    }
    let err = f
        .fetch(
            "https://example.invalid/watch",
            &mut |_| {},
            &AtomicBool::new(true),
        )
        .unwrap_err();
    assert!(matches!(err, FetchError::Cancelled), "{err:?}");
}

#[test]
fn ensure_ytdlp_prefers_tools_dir() {
    let dir = tempfile::tempdir().unwrap();
    let f = fetcher(dir.path());
    std::fs::create_dir_all(&f.tools_dir).unwrap();
    let p = f.tools_dir.join(crate::process::exe_name("yt-dlp"));
    std::fs::write(&p, b"").unwrap();
    crate::process::make_executable(&p).unwrap();
    assert_eq!(f.ensure_ytdlp(&mut |_| {}).unwrap(), p);

    let ff = f.tools_dir.join(crate::process::exe_name("ffmpeg"));
    std::fs::write(&ff, b"").unwrap();
    crate::process::make_executable(&ff).unwrap();
    assert_eq!(f.find_ffmpeg(), Some(ff.clone()));
    assert_eq!(f.ensure_ffmpeg(&mut |_| {}).unwrap(), ff);
}

/// Needs internet: downloads the real yt-dlp release and fetches a YouTube video.
#[test]
#[ignore]
fn online_youtube() {
    let dir = tempfile::tempdir().unwrap();
    let f = fetcher(dir.path());
    let got = f
        .fetch(
            "https://www.youtube.com/watch?v=jNQXAC9IVRw",
            &mut |p| eprintln!("{p:?}"),
            &AtomicBool::new(false),
        )
        .unwrap();
    eprintln!("{got:?}");
    assert!(got.path.is_file());
}

#[test]
fn cookie_errors_are_recognised() {
    assert!(is_cookie_error(
        "Could not copy Chrome cookie database. See  https://github.com/yt-dlp/yt-dlp/issues/7271  for more info"
    ));
    assert!(is_cookie_error(
        "Failed to decrypt with DPAPI. See https://github.com/yt-dlp/yt-dlp/issues/10927"
    ));
    assert!(!is_cookie_error("Video unavailable"));
    assert!(!is_cookie_error(
        "Sign in to confirm your age (this may need a signed-in browser: set the cookies browser option)"
    ));
}
