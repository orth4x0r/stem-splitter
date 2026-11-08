//! Audio I/O and DSP shared by the rest of the workspace.
//!
//! Everything works on [`AudioBuffer`]: planar `f32` samples in `[-1, 1]`.

pub mod analysis;
mod encode;
pub mod stft;

pub use encode::{AudioFormat, write_audio, write_flac, write_mp3};

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported or corrupt audio: {0}")]
    Decode(String),
    #[error("resample: {0}")]
    Resample(String),
    #[error("encode: {0}")]
    Encode(String),
}

pub type Result<T> = std::result::Result<T, AudioError>;

/// Planar audio: `channels[c][i]` is sample `i` of channel `c`. All channels have equal length.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AudioBuffer {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

/// `other`'s channel to combine with channel `c` of a `n`-channel target: same index, or the
/// only channel of a mono buffer (broadcast), otherwise none.
fn matching_channel(other: &AudioBuffer, c: usize) -> Option<&[f32]> {
    match other.channels.len() {
        1 => Some(&other.channels[0]),
        n if c < n => Some(&other.channels[c]),
        _ => None,
    }
}

impl AudioBuffer {
    pub fn new(sample_rate: u32, channels: Vec<Vec<f32>>) -> Self {
        Self {
            sample_rate,
            channels,
        }
    }
    pub fn silence(sample_rate: u32, n_channels: usize, frames: usize) -> Self {
        Self {
            sample_rate,
            channels: vec![vec![0.0; frames]; n_channels],
        }
    }
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }
    pub fn n_channels(&self) -> usize {
        self.channels.len()
    }
    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / self.sample_rate.max(1) as f64
    }
    /// Mono is duplicated, more than two channels are downmixed.
    ///
    /// The downmix assumes WAV/SMPTE channel order (FL, FR, FC, LFE, then surround pairs as
    /// left/right): centre and surrounds go in at -3 dB, LFE is dropped, and each side is scaled
    /// by the sum of its gains so the result cannot clip. Three channels are taken as FL, FR, FC
    /// and four as FL, FR, BL, BR (quad).
    /// An empty buffer becomes empty stereo.
    pub fn to_stereo(&self) -> AudioBuffer {
        let frames = self.frames();
        let channels = match self.channels.len() {
            0 => vec![Vec::new(), Vec::new()],
            1 => vec![self.channels[0].clone(), self.channels[0].clone()],
            2 => self.channels.clone(),
            n => {
                const G: f32 = std::f32::consts::FRAC_1_SQRT_2;
                // (gain left, gain right) per channel
                let gains: Vec<(f32, f32)> = (0..n)
                    .map(|c| match c {
                        0 => (1.0, 0.0),
                        1 => (0.0, 1.0),
                        2 if n != 4 => (G, G),    // centre (quad has none)
                        3 if n > 4 => (0.0, 0.0), // LFE
                        c if c % 2 == 0 => (G, 0.0),
                        _ => (0.0, G),
                    })
                    .collect();
                let sum_l: f32 = gains.iter().map(|g| g.0).sum();
                let sum_r: f32 = gains.iter().map(|g| g.1).sum();
                let mut l = vec![0.0f32; frames];
                let mut r = vec![0.0f32; frames];
                for (ch, &(gl, gr)) in self.channels.iter().zip(&gains) {
                    for i in 0..frames {
                        l[i] += ch[i] * gl / sum_l;
                        r[i] += ch[i] * gr / sum_r;
                    }
                }
                vec![l, r]
            }
        };
        AudioBuffer::new(self.sample_rate, channels)
    }
    /// Copy of frames `[start, end)`, clamped to the buffer.
    pub fn slice(&self, start: usize, end: usize) -> AudioBuffer {
        let end = end.min(self.frames());
        let start = start.min(end);
        AudioBuffer::new(
            self.sample_rate,
            self.channels
                .iter()
                .map(|c| c[start..end].to_vec())
                .collect(),
        )
    }
    /// `self - other`, sample by sample (used for "instrumental = mix - vocals").
    ///
    /// The result has `self`'s shape. A mono `other` is subtracted from every channel; frames or
    /// channels missing from `other` count as silence.
    pub fn subtract(&self, other: &AudioBuffer) -> AudioBuffer {
        let channels = self
            .channels
            .iter()
            .enumerate()
            .map(|(c, ch)| match matching_channel(other, c) {
                Some(o) => ch
                    .iter()
                    .enumerate()
                    .map(|(i, &s)| s - o.get(i).copied().unwrap_or(0.0))
                    .collect(),
                None => ch.clone(),
            })
            .collect();
        AudioBuffer::new(self.sample_rate, channels)
    }
    /// Min/max envelope for waveform drawing: `buckets` pairs over the whole buffer, all channels.
    ///
    /// When there are more buckets than frames, each bucket holds the nearest sample. An empty
    /// buffer gives `(0.0, 0.0)` buckets.
    pub fn peaks(&self, buckets: usize) -> Vec<(f32, f32)> {
        let frames = self.frames();
        if frames == 0 || self.channels.is_empty() {
            return vec![(0.0, 0.0); buckets];
        }
        (0..buckets)
            .map(|b| {
                let start = (b * frames / buckets).min(frames - 1);
                let end = ((b + 1) * frames / buckets).clamp(start + 1, frames);
                let mut lo = f32::INFINITY;
                let mut hi = f32::NEG_INFINITY;
                for ch in &self.channels {
                    for &s in &ch[start..end] {
                        lo = lo.min(s);
                        hi = hi.max(s);
                    }
                }
                (lo, hi)
            })
            .collect()
    }
    /// Sum of several equal-rate buffers (shorter ones are zero-padded). Used for re-mixing stems.
    ///
    /// The result has the most channels of any input; mono inputs are added to every channel.
    /// The sample rate is taken from the first buffer.
    pub fn mix(buffers: &[&AudioBuffer]) -> AudioBuffer {
        let Some(first) = buffers.first() else {
            return AudioBuffer::default();
        };
        let n_ch = buffers.iter().map(|b| b.n_channels()).max().unwrap_or(0);
        let frames = buffers.iter().map(|b| b.frames()).max().unwrap_or(0);
        let mut out = AudioBuffer::silence(first.sample_rate, n_ch, frames);
        for b in buffers {
            for (c, dst) in out.channels.iter_mut().enumerate() {
                if let Some(src) = matching_channel(b, c) {
                    for (d, s) in dst.iter_mut().zip(src) {
                        *d += s;
                    }
                }
            }
        }
        out
    }
    /// Scale so the peak hits `target` (e.g. 0.99). Silence is left alone.
    pub fn normalize(&mut self, target: f32) {
        let peak = self.peak();
        if peak > 0.0 && peak.is_finite() {
            let g = target / peak;
            for s in self.channels.iter_mut().flatten() {
                *s *= g;
            }
        }
    }
    /// Linear fade in/out over the given number of frames (avoids clicks on exported samples).
    ///
    /// The first frame and the last frame end up at zero. Lengths are clamped to the buffer.
    pub fn fade(&mut self, fade_in: usize, fade_out: usize) {
        let frames = self.frames();
        let fade_in = fade_in.min(frames);
        let fade_out = fade_out.min(frames);
        for ch in &mut self.channels {
            for (i, s) in ch.iter_mut().take(fade_in).enumerate() {
                *s *= i as f32 / fade_in as f32;
            }
            for (k, s) in ch.iter_mut().rev().take(fade_out).enumerate() {
                *s *= k as f32 / fade_out as f32;
            }
        }
    }
    /// Peak absolute sample value.
    pub fn peak(&self) -> f32 {
        self.channels
            .iter()
            .flatten()
            .fold(0.0f32, |m, s| m.max(s.abs()))
    }
}

// ---------------------------------------------------------------------------------------------
// Decoding

/// Decode any format symphonia understands (wav, flac, mp3, aac/m4a, ogg/vorbis, ...).
///
/// Uses the default audio track. Packets that fail to decode are skipped (a short gap instead of
/// a failed file); the whole call fails only if the container can't be read or nothing decodes.
pub fn decode_file(path: &Path) -> Result<AudioBuffer> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::errors::Error as SymError;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::{MediaSourceStream, ReadOnlySource};
    use symphonia::core::meta::MetadataOptions;

    /// Give up after this many consecutive demux errors (corrupt tail, etc.).
    const MAX_CONSECUTIVE_ERRORS: usize = 64;

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probe = |seekable: bool| -> Result<_> {
        let file = std::fs::File::open(path)?;
        let source: Box<dyn symphonia::core::io::MediaSource> = if seekable {
            Box::new(file)
        } else {
            Box::new(ReadOnlySource::new(file))
        };
        let mss = MediaSourceStream::new(source, Default::default());
        symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| AudioError::Decode(format!("{}: {e}", path.display())))
    };
    // Some readers scan the end of a seekable file while probing (to find the duration) and
    // give up on damage there; reading it as a plain stream gets past that.
    let mut format = match probe(true) {
        Ok(f) => f,
        Err(AudioError::Io(e)) => return Err(AudioError::Io(e)),
        Err(e) => probe(false).map_err(|_| e)?,
    };

    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| AudioError::Decode("no audio track".into()))?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| AudioError::Decode("track has no audio codec parameters".into()))?
        .clone();
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| AudioError::Decode(format!("no decoder: {e}")))?;

    let mut sample_rate = params.sample_rate.unwrap_or(0);
    let mut channels: Vec<Vec<f32>> = params
        .channels
        .as_ref()
        .map(|c| vec![Vec::new(); c.count()])
        .unwrap_or_default();
    if let Some(n) = track.num_frames {
        let n = usize::try_from(n).unwrap_or(0).min(1 << 28);
        for c in &mut channels {
            c.reserve(n);
        }
    }

    let mut planes: Vec<Vec<f32>> = Vec::new();
    let mut errors = 0usize;
    let mut decoded_any = false;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break, // chained streams: keep the first one
            Err(SymError::DecodeError(_) | SymError::IoError(_)) => {
                errors += 1;
                if errors > MAX_CONSECUTIVE_ERRORS {
                    break;
                }
                continue;
            }
            Err(e) => return Err(AudioError::Decode(e.to_string())),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                errors = 0;
                if sample_rate == 0 {
                    sample_rate = buf.spec().rate();
                }
                buf.copy_to_vecs_planar::<f32>(&mut planes);
                let frames = buf.frames();
                if channels.is_empty() {
                    channels = vec![Vec::new(); planes.len()];
                }
                // Missing planes (channel layout change mid-stream) become silence.
                for (c, dst) in channels.iter_mut().enumerate() {
                    match planes.get(c) {
                        Some(p) => dst.extend_from_slice(&p[..frames.min(p.len())]),
                        None => dst.resize(dst.len() + frames, 0.0),
                    }
                }
                decoded_any |= frames > 0;
            }
            Err(SymError::DecodeError(_) | SymError::IoError(_)) => continue,
            Err(SymError::ResetRequired) => decoder.reset(),
            Err(e) => return Err(AudioError::Decode(e.to_string())),
        }
    }
    if !decoded_any {
        return Err(AudioError::Decode(format!(
            "{}: no audio could be decoded",
            path.display()
        )));
    }
    if sample_rate == 0 {
        return Err(AudioError::Decode("unknown sample rate".into()));
    }
    // Keep channels equal length (the copy above guarantees it, this is belt and braces).
    let frames = channels.iter().map(Vec::len).min().unwrap_or(0);
    for c in &mut channels {
        c.truncate(frames);
    }
    Ok(AudioBuffer::new(sample_rate, channels))
}

// ---------------------------------------------------------------------------------------------
// Resampling

/// High-quality resample. Returns a clone when the rate already matches.
///
/// Output length is `round(frames * target_rate / sample_rate)` and the resampler's delay is
/// compensated exactly, so the output is time-aligned with the input. Uses rubato's synchronous
/// FFT resampler (exact rational ratio, Blackman-Harris anti-aliasing).
pub fn resample(buf: &AudioBuffer, target_rate: u32) -> Result<AudioBuffer> {
    use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
    use rubato::{Fft, FixedSync, Resampler, WindowFunction};

    fn run<R: Resampler<f32>>(
        mut r: R,
        input: &[Vec<f32>],
        frames: usize,
    ) -> Result<Vec<Vec<f32>>> {
        let n_ch = input.len();
        let err = |e: &dyn std::fmt::Display| AudioError::Resample(e.to_string());
        let inp = SequentialSliceOfVecs::new(input, n_ch, frames).map_err(|e| err(&e))?;
        let out_len = r.process_all_needed_output_len(frames);
        let mut out = vec![vec![0.0f32; out_len]; n_ch];
        let mut outp =
            SequentialSliceOfVecs::new_mut(&mut out, n_ch, out_len).map_err(|e| err(&e))?;
        let (_, produced) = r
            .process_all_into_buffer(&inp, &mut outp, frames, None)
            .map_err(|e| err(&e))?;
        for c in &mut out {
            c.truncate(produced);
        }
        Ok(out)
    }

    if target_rate == 0 || buf.sample_rate == 0 {
        return Err(AudioError::Resample("sample rate must be > 0".into()));
    }
    if buf.sample_rate == target_rate {
        return Ok(buf.clone());
    }
    let frames = buf.frames();
    let ratio = target_rate as f64 / buf.sample_rate as f64;
    let out_frames = (frames as f64 * ratio).round() as usize;
    if frames == 0 || buf.channels.is_empty() {
        return Ok(AudioBuffer::silence(
            target_rate,
            buf.n_channels(),
            out_frames,
        ));
    }
    let n_ch = buf.n_channels();
    let (rin, rout) = (buf.sample_rate as usize, target_rate as usize);
    let g = gcd(rin, rout);
    let block_in = rin / g;
    // FFT block = m minimal blocks. m is even so both FFT sizes are even: the resampler's delay
    // is then exactly fft_size_out / 2 output frames, which rubato trims. (With an odd input
    // block it would be off by half an input sample; rubato's sinc resampler likewise rounds
    // its delay, so it is not used.) Blocks of >= 512 input frames keep the anti-aliasing
    // cutoff close to Nyquist. Awkward rate pairs just get big FFT blocks, which costs memory
    // (a few MB) but stays O(log n) per sample.
    let mut m = 512usize.div_ceil(block_in).max(2);
    m += m % 2;
    let sub = m * block_in;
    let n_sub = (8192 / sub).max(1);
    let r = Fft::<f32>::new_custom(
        rin,
        rout,
        sub * n_sub,
        n_sub,
        n_ch,
        WindowFunction::BlackmanHarris2,
        FixedSync::Input,
    )
    .map_err(|e| AudioError::Resample(e.to_string()))?;
    debug_assert_eq!(r.fft_size_in(), sub);
    let mut channels = run(r, &buf.channels, frames)?;
    for c in &mut channels {
        c.resize(out_frames, 0.0);
    }
    Ok(AudioBuffer::new(target_rate, channels))
}

fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

// ---------------------------------------------------------------------------------------------
// WAV writing

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WavFormat {
    Pcm16,
    #[default]
    Pcm24,
    Float32,
}

/// Extra options for [`write_wav_with`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavOptions {
    /// TPDF dither (±1 LSB) when writing [`WavFormat::Pcm16`]. Ignored for other formats.
    pub dither: bool,
}

impl Default for WavOptions {
    fn default() -> Self {
        Self { dither: true }
    }
}

/// Write a WAV file. Integer formats are clamped to full scale; 16-bit gets TPDF dither.
pub fn write_wav(path: &Path, buf: &AudioBuffer, format: WavFormat) -> Result<()> {
    write_wav_with(path, buf, format, WavOptions::default())
}

/// [`write_wav`] with explicit options.
pub fn write_wav_with(
    path: &Path,
    buf: &AudioBuffer,
    format: WavFormat,
    opts: WavOptions,
) -> Result<()> {
    use hound::{SampleFormat, WavSpec, WavWriter};

    let enc = |e: hound::Error| match e {
        hound::Error::IoError(io) => AudioError::Io(io),
        e => AudioError::Encode(e.to_string()),
    };
    let n_ch = buf.n_channels();
    if n_ch == 0 || n_ch > u16::MAX as usize {
        return Err(AudioError::Encode(format!(
            "unsupported channel count {n_ch}"
        )));
    }
    if buf.sample_rate == 0 {
        return Err(AudioError::Encode("sample rate must be > 0".into()));
    }
    let (bits, sample_format) = match format {
        WavFormat::Pcm16 => (16, SampleFormat::Int),
        WavFormat::Pcm24 => (24, SampleFormat::Int),
        WavFormat::Float32 => (32, SampleFormat::Float),
    };
    let spec = WavSpec {
        channels: n_ch as u16,
        sample_rate: buf.sample_rate,
        bits_per_sample: bits,
        sample_format,
    };
    let mut w = WavWriter::create(path, spec).map_err(enc)?;
    let frames = buf.frames();
    let mut rng = Xorshift32(0x9E37_79B9);
    match format {
        WavFormat::Pcm16 => {
            let mut w16 = w.get_i16_writer((frames * n_ch) as u32);
            for i in 0..frames {
                for ch in &buf.channels {
                    let mut v = sanitize(ch[i]) * 32768.0;
                    if opts.dither {
                        v += rng.uniform() - rng.uniform();
                    }
                    w16.write_sample(v.round().clamp(-32768.0, 32767.0) as i16);
                }
            }
            w16.flush().map_err(enc)?;
        }
        WavFormat::Pcm24 => {
            const SCALE: f32 = 8_388_608.0;
            for i in 0..frames {
                for ch in &buf.channels {
                    let v = (sanitize(ch[i]) * SCALE).round().clamp(-SCALE, SCALE - 1.0);
                    w.write_sample(v as i32).map_err(enc)?;
                }
            }
        }
        WavFormat::Float32 => {
            for i in 0..frames {
                for ch in &buf.channels {
                    w.write_sample(sanitize(ch[i])).map_err(enc)?;
                }
            }
        }
    }
    w.finalize().map_err(enc)
}

/// NaN becomes silence; everything else passes through (integer paths clamp afterwards).
fn sanitize(s: f32) -> f32 {
    if s.is_nan() { 0.0 } else { s }
}

/// Tiny deterministic PRNG for dither (quality of randomness is irrelevant here).
struct Xorshift32(u32);

impl Xorshift32 {
    /// Uniform in `[0, 1)`.
    fn uniform(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }
}

// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "stem-audio-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sine(rate: u32, freq: f32, amp: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    fn test_buffer(rate: u32, frames: usize) -> AudioBuffer {
        AudioBuffer::new(
            rate,
            vec![
                sine(rate, 440.0, 0.5, frames),
                sine(rate, 1000.0, 0.25, frames),
            ],
        )
    }

    fn max_diff(a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len());
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f32::max)
    }

    /// Estimate a pure tone's frequency from zero crossings, ignoring the edges.
    fn estimate_freq(x: &[f32], rate: u32) -> f32 {
        let x = &x[x.len() / 10..x.len() * 9 / 10];
        let crossings: Vec<f32> = x
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
            .map(|(i, w)| i as f32 + w[0] / (w[0] - w[1]))
            .collect();
        let periods = (crossings.len() - 1) as f32;
        rate as f32 * periods / (crossings.last().unwrap() - crossings[0])
    }

    #[test]
    fn wav_round_trip_all_formats() {
        let dir = TempDir::new();
        let mut buf = test_buffer(44100, 4410);
        // include out-of-range samples to exercise clamping
        buf.channels[0][10] = 1.5;
        buf.channels[1][10] = -1.5;
        for (fmt, tol) in [
            (WavFormat::Pcm16, 2.5 / 32768.0),
            (WavFormat::Pcm24, 1.0 / 8_388_608.0),
            (WavFormat::Float32, 0.0),
        ] {
            let p = dir.path(&format!("{fmt:?}.wav"));
            write_wav(&p, &buf, fmt).unwrap();
            let back = decode_file(&p).unwrap();
            assert_eq!(back.sample_rate, 44100, "{fmt:?}");
            assert_eq!(back.n_channels(), 2, "{fmt:?}");
            assert_eq!(back.frames(), 4410, "{fmt:?}");
            let mut expect = buf.clone();
            let full_scale = match fmt {
                WavFormat::Pcm16 => Some(32768.0),
                WavFormat::Pcm24 => Some(8_388_608.0),
                WavFormat::Float32 => None,
            };
            if let Some(fs) = full_scale {
                expect.channels[0][10] = (fs - 1.0) / fs;
                expect.channels[1][10] = -1.0;
            }
            for c in 0..2 {
                let d = max_diff(&back.channels[c], &expect.channels[c]);
                assert!(d <= tol + 1e-7, "{fmt:?} ch{c}: diff {d}");
            }
        }
    }

    #[test]
    fn pcm16_without_dither_is_exact_on_grid() {
        let dir = TempDir::new();
        let buf = AudioBuffer::new(8000, vec![(0..100).map(|i| i as f32 / 32768.0).collect()]);
        let p = dir.path("nodither.wav");
        write_wav_with(&p, &buf, WavFormat::Pcm16, WavOptions { dither: false }).unwrap();
        let back = decode_file(&p).unwrap();
        assert_eq!(back, buf);
    }

    #[test]
    fn decode_multichannel_wav() {
        let dir = TempDir::new();
        let buf = AudioBuffer::new(48000, (0..6).map(|c| vec![c as f32 / 10.0; 480]).collect());
        let p = dir.path("six.wav");
        write_wav(&p, &buf, WavFormat::Float32).unwrap();
        assert_eq!(decode_file(&p).unwrap(), buf);
    }

    #[test]
    fn write_rejects_empty_channel_list() {
        let dir = TempDir::new();
        let r = write_wav(
            &dir.path("x.wav"),
            &AudioBuffer::default(),
            WavFormat::Pcm24,
        );
        assert!(matches!(r, Err(AudioError::Encode(_))));
    }

    #[test]
    fn decode_garbage_fails_cleanly() {
        let dir = TempDir::new();
        let p = dir.path("junk.mp3");
        std::fs::write(&p, b"this is definitely not audio").unwrap();
        assert!(decode_file(&p).is_err());
        assert!(matches!(
            decode_file(&dir.path("missing.wav")),
            Err(AudioError::Io(_))
        ));
    }

    /// Lossy/compressed formats: encoded with ffmpeg when it is installed, skipped otherwise.
    #[test]
    fn decode_compressed_formats_via_ffmpeg() {
        let ffmpeg_ok = std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success());
        if !ffmpeg_ok {
            eprintln!("ffmpeg not found, skipping compressed-format decode test");
            return;
        }
        let dir = TempDir::new();
        let rate = 44100;
        let frames = 6 * 44100;
        let buf = test_buffer(rate, frames);
        let src = dir.path("src.wav");
        write_wav(&src, &buf, WavFormat::Pcm16).unwrap();
        for (ext, args, lossless) in [
            ("flac", &["-c:a", "flac"][..], true),
            ("mp3", &["-c:a", "libmp3lame", "-b:a", "192k"][..], false),
            ("m4a", &["-c:a", "aac", "-b:a", "192k"][..], false),
            ("ogg", &["-c:a", "libvorbis", "-q:a", "6"][..], false),
        ] {
            let out = dir.path(&format!("enc.{ext}"));
            let status = std::process::Command::new("ffmpeg")
                .args(["-y", "-loglevel", "error", "-i"])
                .arg(&src)
                .args(args)
                .arg(&out)
                .status()
                .unwrap();
            if !status.success() {
                eprintln!("ffmpeg could not encode {ext}, skipping it");
                continue;
            }
            let dec = decode_file(&out).unwrap_or_else(|e| panic!("{ext}: {e}"));
            assert_eq!(dec.sample_rate, rate, "{ext}");
            assert_eq!(dec.n_channels(), 2, "{ext}");
            let len_err = dec.frames().abs_diff(frames);
            assert!(len_err <= 2048 + 1024, "{ext}: {} frames", dec.frames());
            if lossless {
                assert_eq!(dec.frames(), frames);
                assert!(max_diff(&dec.channels[0], &buf.channels[0]) < 3.0 / 32768.0);
            }
            let f0 = estimate_freq(&dec.channels[0], rate);
            let f1 = estimate_freq(&dec.channels[1], rate);
            assert!((f0 - 440.0).abs() < 1.0, "{ext}: ch0 {f0} Hz");
            assert!((f1 - 1000.0).abs() < 2.0, "{ext}: ch1 {f1} Hz");
            // (the abrupt start/end can make lossy codecs overshoot, so look at the middle)
            let mid = dec.slice(frames / 4, frames * 3 / 4).peak();
            assert!((mid - 0.5).abs() < 0.05, "{ext}: peak {mid}");

            // Damage the middle of the file: decoding must still succeed (bad packets skipped)
            // and come back roughly as long as the original.
            if ext != "flac" {
                let mut bytes = std::fs::read(&out).unwrap();
                let n = bytes.len();
                for (i, b) in bytes[n * 7 / 10..n * 7 / 10 + 600].iter_mut().enumerate() {
                    *b = (i * 37 % 251) as u8;
                }
                let bad = dir.path(&format!("bad.{ext}"));
                std::fs::write(&bad, bytes).unwrap();
                let dec = decode_file(&bad).unwrap_or_else(|e| panic!("damaged {ext}: {e}"));
                assert!(dec.frames() > frames / 2, "damaged {ext}: {}", dec.frames());
            }
        }
    }

    #[test]
    fn resample_length_and_frequency() {
        for &(from, to, frames) in &[
            (44100u32, 48000u32, 44100usize),
            (48000, 44100, 48000),
            (22050, 44100, 12345),
            (44100, 16000, 30000),
            (44100, 44099, 20000), // awkward ratio -> sinc path
        ] {
            let buf = AudioBuffer::new(from, vec![sine(from, 997.0, 0.5, frames); 2]);
            let out = resample(&buf, to).unwrap();
            let want = (frames as f64 * to as f64 / from as f64).round() as usize;
            assert_eq!(out.sample_rate, to);
            assert_eq!(out.n_channels(), 2);
            assert_eq!(out.frames(), want, "{from}->{to}");
            let f = estimate_freq(&out.channels[0], to);
            assert!((f - 997.0).abs() < 0.5, "{from}->{to}: {f} Hz");
            // no leading delay: output should line up with an ideal sine at the new rate
            let ideal = sine(to, 997.0, 0.5, want);
            let mid = &out.channels[0][want / 4..want * 3 / 4];
            let d = max_diff(mid, &ideal[want / 4..want * 3 / 4]);
            assert!(d < 0.01, "{from}->{to}: misaligned, max diff {d}");
        }
    }

    #[test]
    fn resample_edge_cases() {
        let buf = test_buffer(44100, 100);
        assert_eq!(resample(&buf, 44100).unwrap(), buf);
        let empty = AudioBuffer::silence(44100, 2, 0);
        let r = resample(&empty, 48000).unwrap();
        assert_eq!((r.sample_rate, r.n_channels(), r.frames()), (48000, 2, 0));
        // very short input still gives the right length
        let short = test_buffer(44100, 10);
        assert_eq!(resample(&short, 48000).unwrap().frames(), 11);
        assert!(resample(&buf, 0).is_err());
    }

    #[test]
    fn to_stereo_variants() {
        let mono = AudioBuffer::new(8000, vec![vec![0.1, 0.2]]);
        assert_eq!(mono.to_stereo().channels, vec![vec![0.1, 0.2]; 2]);
        let st = test_buffer(8000, 10);
        assert_eq!(st.to_stereo(), st);
        let surround = AudioBuffer::new(8000, (0..6).map(|_| vec![0.5; 4]).collect());
        let s = surround.to_stereo();
        assert_eq!(s.n_channels(), 2);
        assert!(s.channels.iter().flatten().all(|v| (v - 0.5).abs() < 1e-6));
        let lr = AudioBuffer::new(8000, vec![vec![1.0], vec![-1.0], vec![0.0]]);
        let s = lr.to_stereo();
        assert!(s.channels[0][0] > 0.5 && s.channels[1][0] < -0.5);
        assert_eq!(AudioBuffer::default().to_stereo().n_channels(), 2);
    }

    #[test]
    fn slice_clamps() {
        let b = AudioBuffer::new(8000, vec![vec![1.0, 2.0, 3.0, 4.0]]);
        assert_eq!(b.slice(1, 3).channels, vec![vec![2.0, 3.0]]);
        assert_eq!(b.slice(2, 100).channels, vec![vec![3.0, 4.0]]);
        assert_eq!(b.slice(10, 20).frames(), 0);
        assert_eq!(b.slice(3, 1).frames(), 0);
    }

    #[test]
    fn subtract_and_mix() {
        let a = AudioBuffer::new(8000, vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]);
        let b = AudioBuffer::new(8000, vec![vec![1.0, 1.0], vec![2.0, 2.0]]);
        assert_eq!(
            a.subtract(&b).channels,
            vec![vec![0.0, 1.0, 3.0], vec![2.0, 3.0, 6.0]]
        );
        let m = AudioBuffer::new(8000, vec![vec![1.0, 1.0, 1.0]]);
        assert_eq!(
            a.subtract(&m).channels,
            vec![vec![0.0, 1.0, 2.0], vec![3.0, 4.0, 5.0]]
        );
        let mixed = AudioBuffer::mix(&[&a.subtract(&b), &b]);
        assert_eq!(mixed, a);
        assert_eq!(AudioBuffer::mix(&[]), AudioBuffer::default());
        let mixed = AudioBuffer::mix(&[&m, &b]);
        assert_eq!(
            mixed.channels,
            vec![vec![2.0, 2.0, 1.0], vec![3.0, 3.0, 1.0]]
        );
    }

    #[test]
    fn peaks_buckets() {
        let b = AudioBuffer::new(
            8000,
            vec![vec![0.0, 0.5, -0.5, 1.0], vec![0.1, -0.9, 0.0, 0.0]],
        );
        assert_eq!(b.peaks(2), vec![(-0.9, 0.5), (-0.5, 1.0)]);
        assert_eq!(b.peaks(1), vec![(-0.9, 1.0)]);
        let many = b.peaks(8);
        assert_eq!(many.len(), 8);
        assert_eq!(many[0], (0.0, 0.1));
        assert_eq!(many[7], (0.0, 1.0));
        assert!(b.peaks(0).is_empty());
        assert_eq!(AudioBuffer::default().peaks(3), vec![(0.0, 0.0); 3]);
    }

    #[test]
    fn normalize_peak_fade() {
        let mut b = AudioBuffer::new(8000, vec![vec![0.25, -0.5], vec![0.1, 0.0]]);
        assert_eq!(b.peak(), 0.5);
        b.normalize(0.99);
        assert!((b.peak() - 0.99).abs() < 1e-6);
        assert!((b.channels[0][0] - 0.495).abs() < 1e-6);
        let mut s = AudioBuffer::silence(8000, 1, 4);
        s.normalize(0.99);
        assert_eq!(s.peak(), 0.0);

        let mut f = AudioBuffer::new(8000, vec![vec![1.0; 10]]);
        f.fade(4, 2);
        assert_eq!(
            f.channels[0],
            vec![0.0, 0.25, 0.5, 0.75, 1.0, 1.0, 1.0, 1.0, 0.5, 0.0]
        );
        let mut f = AudioBuffer::new(8000, vec![vec![1.0; 3]]);
        f.fade(100, 100); // clamped, must not panic
        assert_eq!(f.channels[0][0], 0.0);
        assert_eq!(f.channels[0][2], 0.0);
    }
}
