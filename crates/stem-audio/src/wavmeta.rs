//! Sampler / loop metadata for WAV files: an `acid` chunk (tempo, root note, one-shot vs
//! loop) and a `smpl` chunk (loop points), so DAWs such as FL Studio, Ableton Live, Reaper
//! and Acid pick up the tempo and loop of an exported stem or sample.
//!
//! The chunks are appended after `data` (where Acidizer and most samplers put them) and the
//! RIFF size is patched, so the file stays valid RIFF for every reader. Layouts follow the
//! public descriptions of the formats (`smpl` from the Microsoft/IBM RIFF multimedia spec,
//! `acid` as publicly documented for Sony ACID). No code was copied.

use std::path::Path;

use crate::{AudioBuffer, AudioError, Result, WavFormat, write_wav};

/// What to write into a WAV besides the audio. `Default` writes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WavMeta {
    /// Tempo in BPM (`acid`).
    pub tempo: Option<f32>,
    /// Root key as a pitch class, C = 0 … B = 11 (`acid` root note, `smpl` unity note).
    pub root: Option<u8>,
    /// A one-shot hit rather than a loop / tempo-mapped file.
    pub one_shot: bool,
    /// Beats per bar (ACID meter numerator); 4 when `None`.
    pub beats_per_bar: Option<u16>,
    /// Loop in frames, `[start, end)`, written as a forward `smpl` loop.
    pub loop_frames: Option<(u32, u32)>,
}

impl WavMeta {
    /// True when there is nothing worth writing.
    pub fn is_empty(&self) -> bool {
        self.tempo.is_none() && self.root.is_none() && !self.one_shot && self.loop_frames.is_none()
    }

    /// MIDI note for the root key (C4 = 60 … B4 = 71), 60 when unknown.
    pub fn midi_root(&self) -> u8 {
        60 + self.root.map_or(0, |r| r % 12)
    }
}

/// ACID flags.
const ACID_ONE_SHOT: u32 = 0x01;
const ACID_ROOT_SET: u32 = 0x02;
const ACID_STRETCH: u32 = 0x04;

/// Payload of an `acid` chunk (24 bytes) for a file of `frames` frames.
pub fn acid_chunk(meta: &WavMeta, frames: u64, sample_rate: u32) -> Vec<u8> {
    let mut flags = 0;
    if meta.one_shot {
        flags |= ACID_ONE_SHOT;
    } else if meta.tempo.is_some() {
        flags |= ACID_STRETCH;
    }
    if meta.root.is_some() {
        flags |= ACID_ROOT_SET;
    }
    let tempo = meta
        .tempo
        .filter(|t| t.is_finite() && *t > 0.0)
        .unwrap_or(0.0);
    let beats = if meta.one_shot || tempo <= 0.0 || sample_rate == 0 {
        0
    } else {
        let secs = frames as f64 / sample_rate as f64;
        ((secs * tempo as f64 / 60.0).round() as u32).max(1)
    };
    let mut out = Vec::with_capacity(24);
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&(meta.midi_root() as u16).to_le_bytes());
    out.extend_from_slice(&0x8000u16.to_le_bytes()); // unknown, always 0x8000 in the wild
    out.extend_from_slice(&0f32.to_le_bytes()); // unknown, always 0
    out.extend_from_slice(&beats.to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes()); // meter denominator
    out.extend_from_slice(&meta.beats_per_bar.unwrap_or(4).to_le_bytes());
    out.extend_from_slice(&tempo.to_le_bytes());
    out
}

/// Payload of a `smpl` chunk with one forward loop, or `None` without loop points.
pub fn smpl_chunk(meta: &WavMeta, sample_rate: u32) -> Option<Vec<u8>> {
    let (start, end) = meta.loop_frames.filter(|(a, b)| b > a)?;
    let period = 1_000_000_000u32.checked_div(sample_rate).unwrap_or(0);
    let mut out = Vec::with_capacity(36 + 24);
    for v in [
        0u32,                    // manufacturer
        0,                       // product
        period,                  // sample period, ns
        meta.midi_root() as u32, // MIDI unity note
        0,                       // pitch fraction
        0,                       // SMPTE format
        0,                       // SMPTE offset
        1,                       // number of loops
        0,                       // sampler data bytes
        // the loop
        0,       // cue point id
        0,       // type: forward
        start,   // first frame
        end - 1, // last frame (inclusive)
        0,       // fraction
        0,       // play count: forever
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    Some(out)
}

/// One chunk of a RIFF file.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub id: [u8; 4],
    pub data: Vec<u8>,
}

fn bad(msg: &str) -> AudioError {
    AudioError::Encode(format!("not a valid WAV: {msg}"))
}

/// Split a RIFF/WAVE file into its top-level chunks. Accepts a missing pad byte on the last
/// chunk (some writers leave it out) and a RIFF size that is too large.
pub fn parse_chunks(bytes: &[u8]) -> Result<Vec<Chunk>> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("no RIFF/WAVE header"));
    }
    let mut out = Vec::new();
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id: [u8; 4] = bytes[pos..pos + 4].try_into().unwrap_or_default();
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap_or_default());
        let start = pos + 8;
        let end = start
            .checked_add(size as usize)
            .filter(|e| *e <= bytes.len())
            .ok_or_else(|| bad("chunk runs past the end of the file"))?;
        out.push(Chunk {
            id,
            data: bytes[start..end].to_vec(),
        });
        pos = end + (size as usize & 1);
    }
    Ok(out)
}

/// Serialise chunks into a RIFF/WAVE file (pad bytes and sizes included).
pub fn build_riff(chunks: &[Chunk]) -> Vec<u8> {
    let body: usize = chunks
        .iter()
        .map(|c| 8 + c.data.len() + (c.data.len() & 1))
        .sum();
    let mut out = Vec::with_capacity(12 + body);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((4 + body) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    for c in chunks {
        out.extend_from_slice(&c.id);
        out.extend_from_slice(&(c.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&c.data);
        if c.data.len() & 1 == 1 {
            out.push(0);
        }
    }
    out
}

/// `(sample_rate, frames)` from the `fmt ` and `data` chunks.
fn format_of(chunks: &[Chunk]) -> Result<(u32, u64)> {
    let fmt = chunks
        .iter()
        .find(|c| &c.id == b"fmt ")
        .filter(|c| c.data.len() >= 16)
        .ok_or_else(|| bad("no fmt chunk"))?;
    let sr = u32::from_le_bytes(fmt.data[4..8].try_into().unwrap_or_default());
    let block = u16::from_le_bytes(fmt.data[12..14].try_into().unwrap_or_default()).max(1);
    let data = chunks
        .iter()
        .find(|c| &c.id == b"data")
        .ok_or_else(|| bad("no data chunk"))?;
    Ok((sr, data.data.len() as u64 / block as u64))
}

/// Replace any `acid` / `smpl` chunks in a WAV file's bytes with ones for `meta`.
pub fn apply_to_bytes(bytes: &[u8], meta: &WavMeta) -> Result<Vec<u8>> {
    let mut chunks = parse_chunks(bytes)?;
    let (sr, frames) = format_of(&chunks)?;
    chunks.retain(|c| &c.id != b"acid" && &c.id != b"smpl");
    if !meta.is_empty() {
        chunks.push(Chunk {
            id: *b"acid",
            data: acid_chunk(meta, frames, sr),
        });
    }
    if let Some(data) = smpl_chunk(meta, sr) {
        chunks.push(Chunk { id: *b"smpl", data });
    }
    Ok(build_riff(&chunks))
}

/// Add (or replace) the metadata of an existing WAV file in place.
pub fn apply_to_file(path: &Path, meta: &WavMeta) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let out = apply_to_bytes(&bytes, meta)?;
    std::fs::write(path, out)?;
    Ok(())
}

/// [`write_wav`] plus metadata chunks (nothing extra when `meta` is empty).
pub fn write_wav_meta(
    path: &Path,
    buf: &AudioBuffer,
    format: WavFormat,
    meta: &WavMeta,
) -> Result<()> {
    write_wav(path, buf, format)?;
    if meta.is_empty() {
        return Ok(());
    }
    append_to_fresh_file(path, meta)
}

/// Append the metadata chunks to a WAV that was just written without any, patching the RIFF
/// size in place. Unlike [`apply_to_file`] this never reads or rewrites the audio, which for a
/// full-length stem is tens of megabytes.
fn append_to_fresh_file(path: &Path, meta: &WavMeta) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head)?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return Err(bad("no RIFF/WAVE header"));
    }
    // Walk the chunk headers (seeking over the bodies) for the format and frame count.
    let len = f.metadata()?.len();
    let (mut pos, mut sr, mut block, mut data_len) = (12u64, None, 1u64, None);
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos))?;
        let mut h = [0u8; 8];
        f.read_exact(&mut h)?;
        let size = u32::from_le_bytes(h[4..8].try_into().unwrap_or_default()) as u64;
        match &h[0..4] {
            b"fmt " if size >= 16 => {
                let mut fmt = [0u8; 16];
                f.read_exact(&mut fmt)?;
                sr = Some(u32::from_le_bytes(fmt[4..8].try_into().unwrap_or_default()));
                block =
                    u16::from_le_bytes(fmt[12..14].try_into().unwrap_or_default()).max(1) as u64;
            }
            b"data" => data_len = Some(size),
            b"acid" | b"smpl" => return apply_to_file(path, meta),
            _ => {}
        }
        pos += 8 + size + (size & 1);
    }
    let sr = sr.ok_or_else(|| bad("no fmt chunk"))?;
    let frames = data_len.ok_or_else(|| bad("no data chunk"))? / block;
    let mut tail = Vec::with_capacity(128);
    if len & 1 == 1 {
        tail.push(0); // pad byte of the previous chunk
    }
    let mut chunks = vec![Chunk {
        id: *b"acid",
        data: acid_chunk(meta, frames, sr),
    }];
    if let Some(data) = smpl_chunk(meta, sr) {
        chunks.push(Chunk { id: *b"smpl", data });
    }
    for c in &chunks {
        tail.extend_from_slice(&c.id);
        tail.extend_from_slice(&(c.data.len() as u32).to_le_bytes());
        tail.extend_from_slice(&c.data);
        if c.data.len() & 1 == 1 {
            tail.push(0);
        }
    }
    let riff = u32::try_from(len + tail.len() as u64 - 8)
        .map_err(|_| AudioError::Encode("WAV over 4 GB".into()))?;
    f.seek(SeekFrom::End(0))?;
    f.write_all(&tail)?;
    f.seek(SeekFrom::Start(4))?;
    f.write_all(&riff.to_le_bytes())?;
    Ok(())
}

/// [`crate::write_audio`] that adds `meta` when the format is WAV (FLAC / MP3 are written
/// as they are).
pub fn write_audio_meta(
    path: &Path,
    buf: &AudioBuffer,
    format: crate::AudioFormat,
    meta: &WavMeta,
) -> Result<()> {
    match format {
        crate::AudioFormat::Wav(w) => write_wav_meta(path, buf, w, meta),
        other => crate::write_audio(path, buf, other),
    }
}

/// Read back the metadata of a WAV file's bytes (`None` without an `acid` or `smpl` chunk).
pub fn read_meta(bytes: &[u8]) -> Result<Option<WavMeta>> {
    let chunks = parse_chunks(bytes)?;
    let u32_at =
        |d: &[u8], i: usize| u32::from_le_bytes(d[i..i + 4].try_into().unwrap_or_default());
    let u16_at =
        |d: &[u8], i: usize| u16::from_le_bytes(d[i..i + 2].try_into().unwrap_or_default());
    let mut meta = WavMeta::default();
    let mut found = false;
    if let Some(a) = chunks
        .iter()
        .find(|c| &c.id == b"acid" && c.data.len() >= 24)
    {
        found = true;
        let flags = u32_at(&a.data, 0);
        meta.one_shot = flags & ACID_ONE_SHOT != 0;
        if flags & ACID_ROOT_SET != 0 {
            meta.root = Some((u16_at(&a.data, 4) % 12) as u8);
        }
        meta.beats_per_bar = Some(u16_at(&a.data, 18));
        let tempo = f32::from_le_bytes(a.data[20..24].try_into().unwrap_or_default());
        meta.tempo = (tempo > 0.0).then_some(tempo);
    }
    if let Some(s) = chunks
        .iter()
        .find(|c| &c.id == b"smpl" && c.data.len() >= 36)
    {
        found = true;
        if u32_at(&s.data, 28) >= 1 && s.data.len() >= 60 {
            let start = u32_at(&s.data, 36 + 8);
            let end = u32_at(&s.data, 36 + 12);
            meta.loop_frames = Some((start, end + 1));
        }
    }
    Ok(found.then_some(meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("stem-wavmeta-{}-{name}", std::process::id()))
    }

    fn tone(frames: usize, channels: usize) -> AudioBuffer {
        let ch: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        AudioBuffer::new(44_100, vec![ch; channels])
    }

    /// Walk the RIFF structure strictly: sizes add up, every chunk is padded to even length.
    fn assert_strict_riff(bytes: &[u8]) {
        assert_eq!(&bytes[0..4], b"RIFF");
        let riff = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff + 8, bytes.len(), "RIFF size matches the file");
        assert_eq!(bytes.len() % 2, 0);
        let mut pos = 12;
        while pos < bytes.len() {
            let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
            pos += 8 + size + (size & 1);
        }
        assert_eq!(pos, bytes.len(), "chunks tile the file exactly");
    }

    fn roundtrip(format: WavFormat, frames: usize, channels: usize) {
        let path = tmp(&format!("{format:?}-{frames}-{channels}.wav"));
        let buf = tone(frames, channels);
        let meta = WavMeta {
            tempo: Some(92.5),
            root: Some(9), // A
            one_shot: false,
            beats_per_bar: Some(4),
            loop_frames: Some((0, frames as u32)),
        };
        write_wav_meta(&path, &buf, format, &meta).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_strict_riff(&bytes);
        let ids: Vec<[u8; 4]> = parse_chunks(&bytes).unwrap().iter().map(|c| c.id).collect();
        assert_eq!(ids.first(), Some(b"fmt "));
        assert!(ids.contains(b"data") && ids.contains(b"acid") && ids.contains(b"smpl"));

        let back = read_meta(&bytes).unwrap().unwrap();
        assert_eq!(back.tempo, Some(92.5));
        assert_eq!(back.root, Some(9));
        assert!(!back.one_shot);
        assert_eq!(back.loop_frames, Some((0, frames as u32)));

        // The audio still decodes, at full length (readers skip the extra chunks).
        let dec = crate::decode_file(&path).unwrap();
        assert_eq!(dec.frames(), frames);
        assert_eq!(dec.n_channels(), channels);
        let mut r = hound::WavReader::open(&path).unwrap();
        assert_eq!(r.duration() as usize, frames);
        let bits = r.spec().bits_per_sample;
        assert_eq!(
            r.samples::<i32>().count(),
            frames * channels,
            "{bits}-bit samples"
        );

        // Writing again replaces the chunks instead of stacking them.
        apply_to_file(
            &path,
            &WavMeta {
                one_shot: true,
                ..Default::default()
            },
        )
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_strict_riff(&bytes);
        let chunks = parse_chunks(&bytes).unwrap();
        assert_eq!(chunks.iter().filter(|c| &c.id == b"acid").count(), 1);
        assert!(!chunks.iter().any(|c| &c.id == b"smpl"));
        assert!(read_meta(&bytes).unwrap().unwrap().one_shot);
        let _ = std::fs::remove_file(&path);
    }

    /// The in-place append gives exactly the bytes the read-rewrite route gives.
    #[test]
    fn fresh_append_matches_rewrite() {
        for (format, frames, channels) in [
            (WavFormat::Pcm24, 4411, 1), // odd data length: needs a pad byte
            (WavFormat::Pcm16, 1000, 2),
            (WavFormat::Float32, 777, 2),
        ] {
            let buf = tone(frames, channels);
            let meta = WavMeta {
                tempo: Some(120.0),
                root: Some(2),
                loop_frames: Some((10, frames as u32)),
                ..Default::default()
            };
            let (a, b) = (tmp("append-a.wav"), tmp("append-b.wav"));
            write_wav_meta(&a, &buf, format, &meta).unwrap();
            write_wav(&b, &buf, format).unwrap();
            apply_to_file(&b, &meta).unwrap();
            let (a_bytes, b_bytes) = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
            assert_strict_riff(&a_bytes);
            assert!(a_bytes == b_bytes, "{format:?} {frames}x{channels}");
            let _ = (std::fs::remove_file(a), std::fs::remove_file(b));
        }
    }

    #[test]
    fn pcm16_stereo_roundtrip() {
        roundtrip(WavFormat::Pcm16, 4410, 2);
    }

    #[test]
    fn pcm24_stereo_roundtrip() {
        roundtrip(WavFormat::Pcm24, 4410, 2);
    }

    /// 24-bit mono with an odd frame count: the data chunk has an odd length, so the pad
    /// byte before our chunks matters.
    #[test]
    fn pcm24_odd_length_stays_aligned() {
        roundtrip(WavFormat::Pcm24, 4411, 1);
    }

    #[test]
    fn acid_layout() {
        let meta = WavMeta {
            tempo: Some(120.0),
            root: Some(2),
            ..Default::default()
        };
        // 4 s at 120 BPM = 8 beats.
        let a = acid_chunk(&meta, 4 * 44_100, 44_100);
        assert_eq!(a.len(), 24);
        assert_eq!(u32::from_le_bytes(a[0..4].try_into().unwrap()), 0x02 | 0x04);
        assert_eq!(u16::from_le_bytes(a[4..6].try_into().unwrap()), 62); // D4
        assert_eq!(u32::from_le_bytes(a[12..16].try_into().unwrap()), 8);
        assert_eq!(u16::from_le_bytes(a[16..18].try_into().unwrap()), 4);
        assert_eq!(u16::from_le_bytes(a[18..20].try_into().unwrap()), 4);
        assert_eq!(f32::from_le_bytes(a[20..24].try_into().unwrap()), 120.0);
        let shot = acid_chunk(
            &WavMeta {
                one_shot: true,
                tempo: Some(90.0),
                ..Default::default()
            },
            1000,
            44_100,
        );
        assert_eq!(u32::from_le_bytes(shot[0..4].try_into().unwrap()), 0x01);
        assert_eq!(u32::from_le_bytes(shot[12..16].try_into().unwrap()), 0);
    }

    #[test]
    fn smpl_layout() {
        let meta = WavMeta {
            loop_frames: Some((100, 200)),
            ..Default::default()
        };
        let s = smpl_chunk(&meta, 48_000).unwrap();
        assert_eq!(s.len(), 60);
        assert_eq!(u32::from_le_bytes(s[8..12].try_into().unwrap()), 20_833);
        assert_eq!(u32::from_le_bytes(s[28..32].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(s[44..48].try_into().unwrap()), 100);
        assert_eq!(u32::from_le_bytes(s[48..52].try_into().unwrap()), 199);
        assert!(smpl_chunk(&WavMeta::default(), 48_000).is_none());
    }

    #[test]
    fn empty_meta_writes_plain_wav() {
        let path = tmp("plain.wav");
        write_wav_meta(
            &path,
            &tone(100, 2),
            WavFormat::Float32,
            &WavMeta::default(),
        )
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(read_meta(&bytes).unwrap().is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_chunks(b"nope").is_err());
        let mut b = build_riff(&[Chunk {
            id: *b"fmt ",
            data: vec![0; 16],
        }]);
        b[16] = 0xFF; // chunk size far past the end
        assert!(parse_chunks(&b).is_err());
    }
}
