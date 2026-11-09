//! "Export stems": which stems, file naming template, output format, and the optional
//! "without X" mixes (every stem but one summed: instrumental without drums, etc.) that make
//! remixing easier. WAV exports carry tempo / key chunks (see `stem_audio::wavmeta`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use stem_audio::wavmeta::WavMeta;
use stem_audio::{AudioBuffer, AudioFormat};
use stem_models::StemKind;

use crate::analysis::TrackAnalysis;
use crate::paths::sanitize_file_name;
use crate::pipeline::{AppEvent, Notifier};
use crate::section::SplitRange;

/// Gives the same names as before templates had tokens ("Song - Vocals").
pub const DEFAULT_TEMPLATE: &str = "{title} - {stem}";
/// Tokens a template understands (`{track}` is an old spelling of `{title}`).
pub const TOKENS: [&str; 7] = [
    "{title}", "{stem}", "{bpm}", "{key}", "{range}", "{model}", "{n}",
];

/// What a template's tokens expand to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NameCtx {
    pub title: String,
    pub bpm: Option<f32>,
    /// Short key name, "Am".
    pub key: Option<String>,
    /// The section a section split covers.
    pub range: Option<SplitRange>,
    pub model: String,
}

impl NameCtx {
    pub fn new(
        title: &str,
        analysis: Option<&TrackAnalysis>,
        range: Option<SplitRange>,
        model: &str,
    ) -> Self {
        Self {
            title: title.to_owned(),
            bpm: analysis.and_then(|a| a.bpm()),
            key: analysis.and_then(|a| a.key.as_ref()).map(|k| k.short()),
            range,
            model: model.to_owned(),
        }
    }
}

/// Fill a naming template: `{title}` (or `{track}`), `{stem}`, `{bpm}` ("92"), `{key}`
/// ("Am"), `{range}` ("1m02-1m34", section splits only), `{model}`, `{n}` (01, 02…).
/// Unknown values leave no stray separators behind, and a section split without `{range}`
/// in the template gets " (1m02-1m34)" appended so whole-track and section files never
/// collide. Result is safe as a Windows file name (no extension).
pub fn render_name(template: &str, ctx: &NameCtx, stem: &str, n: usize) -> String {
    let mut t = if template.trim().is_empty() {
        DEFAULT_TEMPLATE.to_owned()
    } else {
        template.to_owned()
    };
    if ctx.range.is_some() && !t.contains("{range}") {
        t.push_str(" ({range})");
    }
    let bpm = ctx.bpm.map(crate::analysis::bpm_label).unwrap_or_default();
    let range = ctx.range.map(|r| r.label()).unwrap_or_default();
    let name = t
        .replace("{track}", &ctx.title)
        .replace("{title}", &ctx.title)
        .replace("{stem}", stem)
        .replace("{bpm}", &bpm)
        .replace("{key}", ctx.key.as_deref().unwrap_or(""))
        .replace("{range}", &range)
        .replace("{model}", &ctx.model)
        .replace("{n}", &format!("{n:02}"));
    sanitize_file_name(&tidy(&name))
}

/// Drop what empty tokens leave behind: "()", "[]", doubled separators, a trailing " -".
fn tidy(s: &str) -> String {
    let mut s = s.to_owned();
    loop {
        let before = s.len();
        for (from, to) in [
            ("()", ""),
            ("[]", ""),
            ("  ", " "),
            (" - - ", " - "),
            ("__", "_"),
            (" _ ", " "),
        ] {
            s = s.replace(from, to);
        }
        if s.len() == before {
            break;
        }
    }
    let trimmed = s.trim_matches(|c: char| c.is_whitespace() || c == '-' || c == '_');
    if trimmed.is_empty() {
        "untitled".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// WAV metadata for an export: tempo, root key and meter from the analysis; `one_shot` for
/// hits, `loop_frames` for a loop's `smpl` points.
pub fn wav_meta(
    analysis: Option<&TrackAnalysis>,
    one_shot: bool,
    loop_frames: Option<(u32, u32)>,
) -> WavMeta {
    let mut meta = WavMeta {
        one_shot,
        loop_frames,
        ..Default::default()
    };
    if let Some(a) = analysis {
        meta.tempo = a.bpm();
        meta.root = a.key.as_ref().map(|k| k.tonic % 12);
        meta.beats_per_bar = a.grid.as_ref().map(|g| g.beats_per_bar.clamp(1, 16) as u16);
    }
    meta
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
    pub names: NameCtx,
    /// Tempo / key chunks for WAV files.
    pub meta: WavMeta,
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
    names: &NameCtx,
    item: &ExportItem,
    n: usize,
    format: AudioFormat,
) -> PathBuf {
    let name = render_name(template, names, &item.stem_name, n + 1);
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
        let path = item_path(&job.dest, &job.template, &job.names, item, n, job.format);
        stem_audio::wavmeta::write_audio_meta(&path, &audio, job.format, &job.meta)
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

    fn ctx(title: &str) -> NameCtx {
        NameCtx {
            title: title.into(),
            ..Default::default()
        }
    }

    #[test]
    fn names() {
        assert_eq!(
            render_name(DEFAULT_TEMPLATE, &ctx("Song: Remix"), "Vocals", 1),
            "Song_ Remix - Vocals"
        );
        assert_eq!(render_name("{n} {stem}", &ctx("x"), "Bass", 3), "03 Bass");
        assert_eq!(render_name("  ", &ctx("T"), "Drums", 1), "T - Drums");
        // Templates saved before `{title}` existed keep working, with the same result.
        assert_eq!(
            render_name("{track} - {stem}", &ctx("T"), "Drums", 1),
            render_name(DEFAULT_TEMPLATE, &ctx("T"), "Drums", 1)
        );
    }

    #[test]
    fn all_tokens() {
        let full = NameCtx {
            title: "Song".into(),
            bpm: Some(92.0),
            key: Some("Am".into()),
            range: Some(SplitRange::new(62.4, 94.9)),
            model: "HTDemucs".into(),
        };
        assert_eq!(
            render_name(
                "{title} - {stem} - {bpm}bpm {key} [{range}] {model}",
                &full,
                "Bass",
                1
            ),
            "Song - Bass - 92bpm Am [1m02-1m34] HTDemucs"
        );
        // Section splits get the range even when the template doesn't ask for it.
        assert_eq!(
            render_name(DEFAULT_TEMPLATE, &full, "Bass", 1),
            "Song - Bass (1m02-1m34)"
        );
        // Unknown values leave no empty brackets or dangling separators.
        assert_eq!(
            render_name(
                "{title} - {stem} - {key} ({range})",
                &ctx("Song"),
                "Bass",
                1
            ),
            "Song - Bass"
        );
        assert_eq!(render_name("{key}", &ctx("Song"), "Bass", 1), "untitled");
    }

    #[test]
    fn meta_from_analysis() {
        use stem_audio::analysis::{Key, Mode, Tempo};
        let a = TrackAnalysis {
            tempo: Some(Tempo {
                bpm: 90.0,
                confidence: 1.0,
            }),
            key: Some(Key {
                tonic: 9,
                mode: Mode::Minor,
                confidence: 1.0,
            }),
            grid: None,
            detected_bpm: Some(90.0),
            ..Default::default()
        };
        let m = wav_meta(Some(&a), false, Some((0, 10)));
        assert_eq!(m.tempo, Some(90.0));
        assert_eq!(m.root, Some(9));
        assert_eq!(m.loop_frames, Some((0, 10)));
        assert!(wav_meta(None, false, None).is_empty());
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
        let meta = WavMeta {
            tempo: Some(100.0),
            ..Default::default()
        };
        let job = StemExport {
            names: ctx("T"),
            meta,
            stems: Arc::new(stems.clone()),
            items: plan(&kinds, &[true, false, false], true),
            template: DEFAULT_TEMPLATE.into(),
            format: AudioFormat::Flac { bits: 16 },
            dest: dest.clone(),
        };
        run(&job).unwrap();
        assert!(dest.join("T - Vocals.flac").is_file());
        let mix = stem_audio::decode_file(&dest.join("T - No vocals.flac")).unwrap();
        assert!((mix.channels[0][100] - 0.5).abs() < 1e-3);

        // WAV exports carry the tempo chunk and still decode.
        let wav = StemExport {
            format: AudioFormat::Wav(stem_audio::WavFormat::Pcm24),
            stems: Arc::new(stems),
            names: NameCtx {
                range: Some(SplitRange::new(62.0, 94.0)),
                ..ctx("T")
            },
            items: plan(&kinds, &[false, true, false], false),
            template: DEFAULT_TEMPLATE.into(),
            meta,
            dest: dest.clone(),
        };
        run(&wav).unwrap();
        let path = dest.join("T - Drums (1m02-1m34).wav");
        let bytes = std::fs::read(&path).unwrap();
        let back = stem_audio::wavmeta::read_meta(&bytes).unwrap().unwrap();
        assert_eq!(back.tempo, Some(100.0));
        assert_eq!(stem_audio::decode_file(&path).unwrap().frames(), 800);
        let _ = std::fs::remove_dir_all(&dest);
    }
}
