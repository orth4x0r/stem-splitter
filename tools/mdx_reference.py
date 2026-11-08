#!/usr/bin/env python3
"""Reference MDX-Net demix in numpy + onnxruntime, transcribed from python-audio-separator's
`MDXSeparator.demix` / `run_model` / `STFT` (MIT), to cross-check the Rust port on a real
UVR model.

    pip install numpy onnxruntime
    python tools/mdx_reference.py Kim_Vocal_2.onnx 7680 3072 1.009 ref.f32
    STEM_TEST_REAL_MDX=Kim_Vocal_2.onnx STEM_TEST_REAL_MDX_REF=ref.f32 \
        ORT_DYLIB_PATH=... cargo test -p stem-models --test ort_pipeline real_mdx -- --nocapture

The input is the same deterministic test signal as `tones()` in tests/ort_pipeline.rs
(6 s at 44.1 kHz); the output file is the primary stem, float32 little-endian, channel-major.
torch.stft/istft (center=True, reflect, periodic Hann) are written out with numpy.
"""

import sys

import numpy as np
import onnxruntime as ort

HOP = 1024
DIM_T = 256


def tones(n, rate=44100.0):
    i = np.arange(n, dtype=np.float32)
    s = lambda f, a: (a * np.sin(2 * np.pi * f * i / rate)).astype(np.float32)
    return np.stack([s(440, 0.4) + s(2000, 0.2), s(660, 0.3) + s(3000, 0.1)]).astype(np.float32)


def hann(n):
    return (0.5 - 0.5 * np.cos(2 * np.pi * np.arange(n) / n)).astype(np.float32)


def stft(x, n_fft, hop):
    w = hann(n_fft)
    xp = np.pad(x, (n_fft // 2, n_fft // 2), mode="reflect")
    frames = 1 + (len(xp) - n_fft) // hop
    return np.stack([np.fft.rfft(xp[t * hop : t * hop + n_fft] * w) for t in range(frames)], axis=1)  # [F, T]


def istft(z, n_fft, hop):
    w = hann(n_fft)
    frames = z.shape[1]
    total = n_fft + hop * (frames - 1)
    y = np.zeros(total)
    env = np.zeros(total)
    for t in range(frames):
        y[t * hop : t * hop + n_fft] += np.fft.irfft(z[:, t], n_fft) * w
        env[t * hop : t * hop + n_fft] += w * w
    y = y[n_fft // 2 : total - n_fft // 2]
    env = env[n_fft // 2 : total - n_fft // 2]
    return (y / np.where(env > 1e-11, env, 1)).astype(np.float32)


def main():
    path, n_fft, dim_f, compensate, out = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), float(sys.argv[4]), sys.argv[5]
    sess = ort.InferenceSession(path, providers=["CPUExecutionProvider"])
    mix = tones(44100 * 6)

    trim = n_fft // 2
    chunk_size = HOP * (DIM_T - 1)
    gen_size = chunk_size - 2 * trim
    overlap = 0.25
    pad = gen_size + trim - (mix.shape[-1] % gen_size)
    mixture = np.concatenate((np.zeros((2, trim), dtype="float32"), mix, np.zeros((2, pad), dtype="float32")), 1)
    step = int((1 - overlap) * chunk_size)
    result = np.zeros((2, mixture.shape[-1]), dtype=np.float32)
    divider = np.zeros((2, mixture.shape[-1]), dtype=np.float32)
    for i in range(0, mixture.shape[-1], step):
        start, end = i, min(i + chunk_size, mixture.shape[-1])
        window = np.tile(np.hanning(end - start)[None, :], (2, 1))
        part = mixture[:, start:end]
        if end != i + chunk_size:
            part = np.concatenate((part, np.zeros((2, i + chunk_size - end), dtype="float32")), axis=-1)
        spek = np.zeros((1, 4, dim_f, DIM_T), dtype=np.float32)
        for c in range(2):
            z = stft(part[c], n_fft, HOP)[:dim_f]
            spek[0, 2 * c], spek[0, 2 * c + 1] = z.real, z.imag
        spek[:, :, :3, :] *= 0
        pred = sess.run(None, {"input": spek})[0]
        waves = np.zeros((2, chunk_size), dtype=np.float32)
        for c in range(2):
            z = np.zeros((n_fft // 2 + 1, DIM_T), dtype=np.complex64)
            z[:dim_f] = pred[0, 2 * c] + 1j * pred[0, 2 * c + 1]
            waves[c] = istft(z, n_fft, HOP)
        result[:, start:end] += waves[:, : end - start] * window
        divider[:, start:end] += window
    with np.errstate(invalid="ignore", divide="ignore"):
        tar = result / divider
    source = tar[:, trim:-trim][:, : mix.shape[-1]] * compensate
    source.astype("<f4").tofile(out)
    print("wrote", out, source.shape, "rms", np.sqrt((source**2).mean(axis=1)))


if __name__ == "__main__":
    main()
