//! Export encoders beyond WAV: FLAC (lossless, via `flacenc`) and MP3 CBR (via
//! `audio2mp3-core`). Both encoders are pure Rust, so they build for every target, Windows
//! (gnu and msvc) included.

use std::path::Path;

use crate::{AudioBuffer, AudioError, Result, WavFormat, Xorshift32, sanitize, write_wav};

/// File format for exported audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Wav(WavFormat),
    /// Lossless FLAC at 16 or 24 bits per sample.
    Flac {
        bits: u8,
    },
    /// Constant-bitrate MP3 (MPEG-1 Layer III). `kbps` is one of 64..=320.
    Mp3 {
        kbps: u16,
    },
}

impl AudioFormat {
    /// File extension without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            AudioFormat::Wav(_) => "wav",
            AudioFormat::Flac { .. } => "flac",
            AudioFormat::Mp3 { .. } => "mp3",
        }
    }
}

/// Write `buf` to `path` in `format`.
pub fn write_audio(path: &Path, buf: &AudioBuffer, format: AudioFormat) -> Result<()> {
    match format {
        AudioFormat::Wav(w) => write_wav(path, buf, w),
        AudioFormat::Flac { bits } => write_flac(path, buf, bits),
        AudioFormat::Mp3 { kbps } => write_mp3(path, buf, kbps),
    }
}

fn check_buffer(buf: &AudioBuffer) -> Result<()> {
    if buf.n_channels() == 0 {
        return Err(AudioError::Encode("no channels".into()));
    }
    if buf.sample_rate == 0 {
        return Err(AudioError::Encode("sample rate must be > 0".into()));
    }
    Ok(())
}

/// Interleaved integer samples at `bits` (16 gets TPDF dither, like the WAV writer).
fn interleave_int(buf: &AudioBuffer, bits: u8) -> Vec<i32> {
    let scale = (1i64 << (bits - 1)) as f32;
    let (lo, hi) = (-scale, scale - 1.0);
    let dither = bits <= 16;
    let mut rng = Xorshift32(0x9E37_79B9);
    let mut out = Vec::with_capacity(buf.frames() * buf.n_channels());
    for i in 0..buf.frames() {
        for ch in &buf.channels {
            let mut v = sanitize(ch[i]) * scale;
            if dither {
                v += rng.uniform() - rng.uniform();
            }
            out.push(v.round().clamp(lo, hi) as i32);
        }
    }
    out
}

/// Lossless FLAC. `bits` must be 16 or 24.
pub fn write_flac(path: &Path, buf: &AudioBuffer, bits: u8) -> Result<()> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;

    check_buffer(buf)?;
    if !matches!(bits, 16 | 24) {
        return Err(AudioError::Encode(format!(
            "FLAC: unsupported bit depth {bits}"
        )));
    }
    if buf.n_channels() > 8 {
        return Err(AudioError::Encode("FLAC: at most 8 channels".into()));
    }
    let samples = interleave_int(buf, bits);
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, e)| AudioError::Encode(format!("FLAC config: {e:?}")))?;
    let source = flacenc::source::MemSource::from_samples(
        &samples,
        buf.n_channels(),
        bits as usize,
        buf.sample_rate as usize,
    );
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| AudioError::Encode(format!("FLAC: {e:?}")))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| AudioError::Encode(format!("FLAC: {e:?}")))?;
    let mut bytes = sink.as_slice().to_vec();
    fix_min_block_size(&mut bytes);
    std::fs::write(path, &bytes)?;
    Ok(())
}

/// flacenc stores the (shorter) last block's length as STREAMINFO's minimum block size, but the
/// spec excludes the last block from that field. Readers that use min == max to detect a
/// fixed-blocksize stream (symphonia among them) then reject every frame. Every block but the
/// last has the configured size, so set min = max.
///
/// Layout: "fLaC", a 4-byte metadata block header (type 0 = STREAMINFO must come first), then
/// STREAMINFO starting with u16 min block size and u16 max block size (big endian).
fn fix_min_block_size(bytes: &mut [u8]) {
    if bytes.len() >= 12 && &bytes[..4] == b"fLaC" && bytes[4] & 0x7F == 0 {
        let (min, max) = bytes[8..12].split_at_mut(2);
        min.copy_from_slice(max);
    }
}

/// MP3 CBR at `kbps`. Input with more than two channels is folded to stereo; sample rates other
/// than 32 / 44.1 / 48 kHz are resampled to 44.1 kHz first. The file starts with a Xing "Info"
/// frame carrying the exact length and encoder delay. Like every MP3, the decoded audio may
/// start ~1100 samples late in players that ignore it, so WAV/FLAC suit tight samples better.
pub fn write_mp3(path: &Path, buf: &AudioBuffer, kbps: u16) -> Result<()> {
    use audio2mp3_core::{Bitrate, EncoderConfig, Mp3Encoder};

    check_buffer(buf)?;
    let bitrate = Bitrate::from_kbps(kbps)
        .map_err(|_| AudioError::Encode(format!("MP3: unsupported bitrate {kbps} kbps")))?;
    let stereo;
    let mut src = buf;
    if src.n_channels() > 2 {
        stereo = src.to_stereo();
        src = &stereo;
    }
    let resampled;
    if !matches!(src.sample_rate, 32_000 | 44_100 | 48_000) {
        resampled = crate::resample(src, 44_100)?;
        src = &resampled;
    }
    let enc_err = |e: audio2mp3_core::Error| AudioError::Encode(format!("MP3: {e:?}"));
    let mut enc = Mp3Encoder::new(EncoderConfig {
        sample_rate: src.sample_rate,
        channels: src.n_channels() as u8,
        bitrate,
    })
    .map_err(enc_err)?;
    enc.enable_info_frame().map_err(enc_err)?;

    let pcm: Vec<i16> = interleave_int(src, 16)
        .into_iter()
        .map(|s| s as i16)
        .collect();
    let mut out = Vec::with_capacity(pcm.len() / 4);
    // Moderate blocks keep the encoder's scratch output buffer small.
    for block in pcm.chunks(4096 * src.n_channels()) {
        enc.encode(block, &mut out).map_err(enc_err)?;
    }
    enc.flush(&mut out).map_err(enc_err)?;
    let info = enc.info_frame_size();
    if info > 0 && out.len() >= info {
        enc.info_frame_into(&mut out[..info]).map_err(enc_err)?;
    }
    std::fs::write(path, &out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(sr: u32, secs: f32) -> AudioBuffer {
        let n = (sr as f32 * secs) as usize;
        let l: Vec<f32> = (0..n)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / sr as f32).sin() * 0.5)
            .collect();
        let r: Vec<f32> = l.iter().map(|v| v * 0.5).collect();
        AudioBuffer::new(sr, vec![l, r])
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("stem-audio-enc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn flac_roundtrip_is_lossless_at_24_bit() {
        let buf = tone(44_100, 0.5);
        let p = tmp("t24.flac");
        write_flac(&p, &buf, 24).unwrap();
        let back = crate::decode_file(&p).unwrap();
        assert_eq!(back.sample_rate, 44_100);
        assert_eq!(back.n_channels(), 2);
        assert_eq!(back.frames(), buf.frames());
        let err = buf.channels[0]
            .iter()
            .zip(&back.channels[0])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(err < 1e-5, "max error {err}");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn flac_16_bit_is_smaller_than_wav() {
        let buf = tone(48_000, 0.5);
        let p = tmp("t16.flac");
        write_audio(&p, &buf, AudioFormat::Flac { bits: 16 }).unwrap();
        let size = std::fs::metadata(&p).unwrap().len();
        assert!(size < (buf.frames() * 4) as u64, "{size} bytes");
        assert_eq!(crate::decode_file(&p).unwrap().frames(), buf.frames());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn mp3_decodes_back_with_about_the_same_length() {
        let buf = tone(44_100, 1.0);
        let p = tmp("t.mp3");
        write_audio(&p, &buf, AudioFormat::Mp3 { kbps: 320 }).unwrap();
        let back = crate::decode_file(&p).unwrap();
        assert_eq!(back.sample_rate, 44_100);
        assert_eq!(back.n_channels(), 2);
        // MP3 adds encoder delay + padding (decoders that read the Info frame trim it; ours
        // doesn't): at most two extra 1152-sample frames.
        assert!(
            back.frames() >= buf.frames() && back.frames() <= buf.frames() + 2 * 1152,
            "frames {} vs {}",
            back.frames(),
            buf.frames()
        );
        let peak = back.channels[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.08, "peak {peak}");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn mp3_resamples_odd_rates_and_folds_channels() {
        let mut buf = tone(22_050, 0.3);
        buf.channels.push(buf.channels[0].clone());
        let p = tmp("odd.mp3");
        write_mp3(&p, &buf, 128).unwrap();
        let back = crate::decode_file(&p).unwrap();
        assert_eq!(back.sample_rate, 44_100);
        assert_eq!(back.n_channels(), 2);
        let _ = std::fs::remove_file(&p);
        assert!(write_mp3(&p, &buf, 100).is_err());
    }

    #[test]
    fn extensions() {
        assert_eq!(AudioFormat::Wav(WavFormat::Pcm24).extension(), "wav");
        assert_eq!(AudioFormat::Flac { bits: 16 }.extension(), "flac");
        assert_eq!(AudioFormat::Mp3 { kbps: 320 }.extension(), "mp3");
    }
}
