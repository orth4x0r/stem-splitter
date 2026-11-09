//! Writing chops: numbered slice files in `<session>/Chops`, an SFZ instrument mapping them to
//! notes from MIDI 36 up, and optionally a Decent Sampler `.dspreset`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use stem_audio::{AudioBuffer, AudioFormat, WavFormat};

use crate::paths::sanitize_file_name;

/// First note of the kit: MIDI 36 (C1 in FL Studio / Ableton naming, the GM kick note).
pub const BASE_NOTE: u8 = 36;

pub struct ChopExport {
    pub track: String,
    pub stem: String,
    pub slices: Vec<Arc<AudioBuffer>>,
    pub format: AudioFormat,
    /// The `Chops` folder.
    pub dir: PathBuf,
    pub choke: bool,
    pub dspreset: bool,
}

/// Chops go out as WAV or FLAC: samplers don't load MP3 reliably.
pub fn chop_format(f: AudioFormat) -> AudioFormat {
    match f {
        AudioFormat::Mp3 { .. } => AudioFormat::Wav(WavFormat::Pcm24),
        other => other,
    }
}

/// "<Track> - <Stem>", file-name safe.
pub fn base_name(track: &str, stem: &str) -> String {
    sanitize_file_name(&format!("{track} - {stem}"))
}

/// "<Track> - <Stem> - Chop 01" (no extension); `i` counts from 0.
pub fn chop_name(track: &str, stem: &str, i: usize) -> String {
    format!("{} - Chop {:02}", base_name(track, stem), i + 1)
}

/// MIDI note name in the C3 = 60 convention used by FL Studio / Ableton ("C1" for 36).
pub fn note_name(n: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", NAMES[n as usize % 12], n as i32 / 12 - 2)
}

/// SFZ text for `files` (names relative to the .sfz) on consecutive notes.
pub fn sfz_text(title: &str, files: &[String], choke: bool) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "// {title}: {} chops, made with Stem Splitter.\n// Slice 1 is on MIDI note {BASE_NOTE} ({}), one note per slice upward.\n\n",
        files.len(),
        note_name(BASE_NOTE)
    ));
    s.push_str("<control>\ndefault_path=\n\n<global>\nloop_mode=one_shot\namp_veltrack=80\nampeg_release=0.03\n\n");
    if choke {
        s.push_str("<group>\ngroup=1\noff_by=1\noff_mode=fast\n\n");
    } else {
        s.push_str("<group>\n\n");
    }
    for (i, f) in files.iter().enumerate() {
        let key = BASE_NOTE as usize + i;
        if key > 127 {
            break;
        }
        s.push_str(&format!(
            "<region> sample={f} key={key} // {}\n",
            note_name(key as u8)
        ));
    }
    s
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Decent Sampler preset for `files` on consecutive notes.
pub fn dspreset_text(files: &[String], choke: bool) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<DecentSampler minVersion=\"1.0.0\">\n  <groups attack=\"0.001\" decay=\"0\" sustain=\"1\" release=\"0.03\" volume=\"0dB\">\n",
    );
    let tags = if choke {
        " tags=\"chop\" silencedByTags=\"chop\" silencingMode=\"fast\""
    } else {
        ""
    };
    s.push_str(&format!("    <group{tags}>\n"));
    for (i, f) in files.iter().enumerate() {
        let key = BASE_NOTE as usize + i;
        if key > 127 {
            break;
        }
        s.push_str(&format!(
            "      <sample path=\"{}\" rootNote=\"{key}\" loNote=\"{key}\" hiNote=\"{key}\" trigger=\"attack\"/>\n",
            xml_escape(f)
        ));
    }
    s.push_str("    </group>\n  </groups>\n</DecentSampler>\n");
    s
}

/// Delete chop files of this track / stem left over from an earlier export (fewer slices now).
fn remove_old(dir: &Path, base: &str) {
    let prefix = format!("{base} - Chop ");
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Write one slice as `<Track> - <Stem> - Chop NN.<ext>` (blocking). Returns its path.
pub fn write_one(job: &ChopExport, i: usize) -> Result<PathBuf, String> {
    let buf = job.slices.get(i).ok_or("no such slice")?;
    let format = chop_format(job.format);
    std::fs::create_dir_all(&job.dir).map_err(|e| e.to_string())?;
    let path = job.dir.join(format!(
        "{}.{}",
        chop_name(&job.track, &job.stem, i),
        format.extension()
    ));
    stem_audio::write_audio(&path, buf, format).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Write every slice plus the SFZ (and .dspreset). Returns the number of slices written.
pub fn write_all(job: &ChopExport) -> Result<usize, String> {
    if job.slices.is_empty() {
        return Err("no slices".into());
    }
    std::fs::create_dir_all(&job.dir).map_err(|e| e.to_string())?;
    let base = base_name(&job.track, &job.stem);
    remove_old(&job.dir, &base);
    let mut files = Vec::new();
    for i in 0..job.slices.len() {
        let p = write_one(job, i)?;
        files.push(
            p.file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
    }
    let title = format!("{} - {}", job.track, job.stem);
    std::fs::write(
        job.dir.join(format!("{base} - Chops.sfz")),
        sfz_text(&title, &files, job.choke),
    )
    .map_err(|e| e.to_string())?;
    if job.dspreset {
        std::fs::write(
            job.dir.join(format!("{base} - Chops.dspreset")),
            dspreset_text(&files, job.choke),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(files.len())
}

/// Run [`write_all`] on a worker thread; the result arrives on the returned channel.
pub fn spawn(
    job: ChopExport,
    ctx: eframe::egui::Context,
) -> crossbeam_channel::Receiver<crate::chop::ExportResult> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write_all(&job)))
            .unwrap_or_else(|_| Err("chop export crashed".into()))
            .map(|n| (n, job.dir.clone()));
        let _ = tx.send(res);
        ctx.request_repaint();
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_notes() {
        assert_eq!(
            chop_name("Song: A/B", "Drums", 0),
            "Song_ A_B - Drums - Chop 01"
        );
        assert_eq!(note_name(36), "C1");
        assert_eq!(note_name(60), "C3");
        assert_eq!(note_name(37), "C#1");
    }

    #[test]
    fn sfz_and_dspreset() {
        let files = vec!["a - Chop 01.wav".to_owned(), "a - Chop 02.wav".to_owned()];
        let sfz = sfz_text("a", &files, true);
        assert!(sfz.contains("<region> sample=a - Chop 01.wav key=36"));
        assert!(sfz.contains("key=37"));
        assert!(sfz.contains("off_by=1"));
        assert!(!sfz_text("a", &files, false).contains("off_by"));
        let ds = dspreset_text(&["x & y.wav".to_owned()], false);
        assert!(ds.contains("path=\"x &amp; y.wav\" rootNote=\"36\""));
        assert!(ds.starts_with("<?xml"));
    }

    #[test]
    fn writes_files() {
        let dir = std::env::temp_dir().join(format!("stem-chops-{}", std::process::id()));
        let buf = Arc::new(AudioBuffer::new(44_100, vec![vec![0.1; 441]; 2]));
        let mut job = ChopExport {
            track: "T".into(),
            stem: "Drums".into(),
            slices: vec![buf.clone(), buf.clone(), buf.clone()],
            format: AudioFormat::Mp3 { kbps: 320 },
            dir: dir.clone(),
            choke: false,
            dspreset: true,
        };
        assert_eq!(write_all(&job).unwrap(), 3);
        assert!(dir.join("T - Drums - Chop 03.wav").exists());
        assert!(dir.join("T - Drums - Chops.sfz").exists());
        assert!(dir.join("T - Drums - Chops.dspreset").exists());
        // Fewer slices later: the old third file goes away.
        job.slices.pop();
        write_all(&job).unwrap();
        assert!(!dir.join("T - Drums - Chop 03.wav").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
