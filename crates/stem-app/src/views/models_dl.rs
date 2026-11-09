//! Text for a running model download: percent, bytes, speed and time left.

use stem_models::DownloadStatus;

use crate::backend::human_bytes;

/// "4:05" / "52 s" style duration.
fn eta(secs: f64) -> String {
    let s = secs.round().max(0.0) as u64;
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{}:{:02}", s / 60, s % 60),
        _ => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
    }
}

/// "42%  ·  120 MB of 300 MB  ·  8.4 MB/s  ·  0:21 left".
pub fn label(st: &DownloadStatus, size_hint: u64) -> String {
    let total = st.total.unwrap_or(size_hint);
    let mut out = if total > 0 {
        format!(
            "{:.0}%  ·  {} of {}",
            (st.done as f64 / total as f64 * 100.0).min(100.0),
            human_bytes(st.done),
            human_bytes(total)
        )
    } else {
        format!("Downloading  ·  {}", human_bytes(st.done))
    };
    if st.bytes_per_sec > 1.0 {
        out.push_str(&format!("  ·  {}/s", human_bytes(st.bytes_per_sec as u64)));
        if let Some(left) = st.eta_secs() {
            out.push_str(&format!("  ·  {} left", eta(left)));
        }
    } else if st.resumed_from > 0 {
        out.push_str("  ·  resuming");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(eta(52.4), "52 s");
        assert_eq!(eta(245.0), "4:05");
        assert_eq!(eta(3720.0), "1 h 02 min");
        let st = DownloadStatus {
            done: 50,
            total: Some(100),
            bytes_per_sec: 10.0,
            resumed_from: 0,
        };
        let l = label(&st, 0);
        assert!(l.starts_with("50%"), "{l}");
        assert!(l.ends_with("5 s left"), "{l}");
        let resumed = DownloadStatus {
            resumed_from: 10,
            ..DownloadStatus::default()
        };
        assert!(label(&resumed, 100).ends_with("resuming"));
    }
}
