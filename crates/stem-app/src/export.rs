//! "Export stems": which stems, file naming template, output format, and the optional
//! "without X" mixes (every stem but one summed: instrumental without drums, etc.) that make
//! remixing easier.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use stem_audio::{AudioBuffer, AudioFormat};
use stem_models::StemKind;

use crate::paths::sanitize_file_name;
use crate::pipeline::{AppEvent, Notifier};

pub const DEFAULT_TEMPLATE: &str = "{track} - {stem}";

/// Fill a naming template. Placeholders: `{track}` (title), `{stem}` (stem name),
/// `{n}` (1-based position). Result is safe as a Windows file name (no extension).
pub fn render_name(template: &str, track: &str, stem: &str, n: usize) -> String {
    let t = if template.trim().is_empty() {
        DEFAULT_TEMPLATE
    } else {
        template
    };
    let name = t
        .replace("{track}", track)
        .replace("{stem}", stem)
        .replace("{n}", &format!("{n:02}"));
    sanitize_file_name(&name)
}

/// Stem name used for a "without X" mix.
pub fn without_label(kind: StemKind) -> String {
    format!("No {}", kind.label().to_lowercase())
}

/// One file to write: a stem name for the template and which tracks to sum.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportItem {
    pub stem_name: String,
    pub include: Vec<usize>,
}

/// The files an export produces: each selected stem, plus (when `without` is on and there
/// are at least three stems, so the mix differs from a single stem) "all but X" for each
/// selected X.
pub fn plan(kinds: &[StemKind], selected: &[bool], without: bool) -> Vec<ExportItem> {
    let sel = |i: usize| selected.get(i).copied().unwrap_or(false);
    let mut out: Vec<ExportItem> = (0..kinds.len())
        .filter(|&i| sel(i))
        .map(|i| ExportItem {
            stem_name: kinds[i].label().to_owned(),
            include: vec![i],
        })
        .collect();
    if without && kinds.len() >= 3 {
        for x in (0..kinds.len()).filter(|&i| sel(i)) {
            out.push(ExportItem {
                stem_name: without_label(kinds[x]),
                include: (0..kinds.len()).filter(|&i| i != x).collect(),
            });
        }
    }
    out
}

pub struct StemExport {
    pub title: String,
    pub stems: Arc<Vec<AudioBuffer>>,
    pub items: Vec<ExportItem>,
    pub template: String,
    pub format: AudioFormat,
    pub dest: PathBuf,
}

/// File path for item `n` (0-based) of an export.
pub fn item_path(
    dest: &Path,
    template: &str,
    title: &str,
    item: &ExportItem,
    n: usize,
    format: AudioFormat,
) -> PathBuf {
    let name = render_name(template, title, &item.stem_name, n + 1);
    dest.join(format!("{name}.{}", format.extension()))
}

fn run(job: &StemExport) -> Result<String, String> {
    std::fs::create_dir_all(&job.dest).map_err(|e| format!("{}: {e}", job.dest.display()))?;
    for (n, item) in job.items.iter().enumerate() {
        let parts: Vec<&AudioBuffer> = item
            .include
            .iter()
            .filter_map(|&i| job.stems.get(i))
            .collect();
        let audio = match parts.as_slice() {
            [] => continue,
            [one] => (*one).clone(),
            many => AudioBuffer::mix(many),
        };
        let path = item_path(&job.dest, &job.template, &job.title, item, n, job.format);
        stem_audio::write_audio(&path, &audio, job.format)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(format!(
        "Exported {} files to {}",
        job.items.len(),
        job.dest.display()
    ))
}

/// Write every planned file on a worker thread; reports through [`AppEvent::StemsExported`].
pub fn spawn(job: StemExport, n: Notifier) {
    std::thread::spawn(move || {
        let result = crate::backend::guard("export stems", || run(&job)).and_then(|r| r);
        n.send(AppEvent::StemsExported(
            result.map(|msg| (msg, job.dest.clone())),
        ));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const K4: [StemKind; 4] = [
        StemKind::Vocals,
        StemKind::Drums,
        StemKind::Bass,
        StemKind::Other,
    ];

    #[test]
    fn names() {
        assert_eq!(
            render_name(DEFAULT_TEMPLATE, "Song: Remix", "Vocals", 1),
            "Song_ Remix - Vocals"
        );
        assert_eq!(render_name("{n} {stem}", "x", "Bass", 3), "03 Bass");
        assert_eq!(render_name("  ", "T", "Drums", 1), "T - Drums");
    }

    #[test]
    fn plan_singles_and_without_mixes() {
        let p = plan(&K4, &[true, true, false, false], false);
        assert_eq!(p.len(), 2);
        assert_eq!(p[1].include, vec![1]);
        let p = plan(&K4, &[true, true, false, false], true);
        assert_eq!(p.len(), 4);
        assert_eq!(p[2].stem_name, "No vocals");
        assert_eq!(p[2].include, vec![1, 2, 3]);
        assert_eq!(p[3].include, vec![0, 2, 3]);
        // Two-stem models: "without vocals" is just the instrumental, so no extra files.
        let two = [StemKind::Vocals, StemKind::Instrumental];
        assert_eq!(plan(&two, &[true, true], true).len(), 2);
    }

    #[test]
    fn writes_files() {
        let sr = 8000;
        let stems: Vec<AudioBuffer> = (0..3)
            .map(|k| AudioBuffer::new(sr, vec![vec![0.1 * (k + 1) as f32; 800]; 2]))
            .collect();
        let dest = std::env::temp_dir().join(format!("stem-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dest);
        let kinds = [StemKind::Vocals, StemKind::Drums, StemKind::Bass];
        let job = StemExport {
            title: "T".into(),
            stems: Arc::new(stems),
            items: plan(&kinds, &[true, false, false], true),
            template: DEFAULT_TEMPLATE.into(),
            format: AudioFormat::Flac { bits: 16 },
            dest: dest.clone(),
        };
        run(&job).unwrap();
        assert!(dest.join("T - Vocals.flac").is_file());
        let mix = stem_audio::decode_file(&dest.join("T - No vocals.flac")).unwrap();
        assert!((mix.channels[0][100] - 0.5).abs() < 1e-3);
        let _ = std::fs::remove_dir_all(&dest);
    }
}
