#!/usr/bin/env python3
"""Export a Mel-Band RoFormer checkpoint to ONNX with the STFT/iSTFT *outside* the graph, in
the audiojs/mel-roformer contract that stem-models' `ModelArch::RoFormer` runs
(crates/stem-models/src/roformer.rs, "stacked" layout).

Models (both "personal use" in the app):

    denoise         aufr33's Mel-RoFormer denoise (no stated licence)
                    -> mel_roformer_denoise.onnx        (registry id "mel-roformer-denoise")
    dereverb-echo   Sucial's De-Reverb-Echo v2 (CC BY-NC-SA 4.0)
                    -> mel_roformer_dereverb_echo.onnx  (registry id "mel-roformer-dereverb-echo")

anvuew's de-reverb RoFormers are GPL-3.0 and deliberately not offered.

Graph contract:

    input   mix_spec    float32 [1, 4, 1025, 801]      L.re, L.im, R.re, R.im of the STFT
    output  stems_spec  float32 [1, S, 4, 1025, 801]   the masked spectrogram per source

STFT: periodic Hann, n_fft 2048, hop 441, center=True, reflect pad, unnormalised; one segment
is 352800 samples (8 s). The graph is `MelBandRoformer.forward` between `torch.stft` and
`torch.istft` with the complex mask multiply written out in real arithmetic (ONNX has no complex
tensors) and the DC bin zeroed like `zero_dc`. S is 1 (the registry's first stem; the second is
`mix - first` in Rust) or 2 (both, first stem first): the wrapper puts the registry's primary
stem ("dry") first whatever order the checkpoint predicts in.

Usage:

    pip install torch onnx onnxruntime pyyaml numpy einops rotary-embedding-torch beartype librosa
    python tools/export_mel_roformer_onnx.py denoise out/
    python tools/export_mel_roformer_onnx.py dereverb-echo out/
    ... --ckpt file.ckpt --config file.yaml        # offline

The checkpoint is sha256-checked; the config is taken from the first mirror that answers. The
script checks the wrapper against `model(audio)` and the ONNX graph against PyTorch, then
prints the file's size and sha256. Copy the .onnx into the app's models folder.

    python tools/export_mel_roformer_onnx.py selftest crates/stem-models/tests/fixtures/

builds a tiny random-weight Mel-RoFormer and a reference separation for
`cargo test -p stem-models --test msst_reference`.

The forward below is a port of `MelBandRoformer.forward` from ZFTurbo/Music-Source-Separation-
Training (`models/bs_roformer/mel_band_roformer.py`, MIT, Copyright (c) 2024 Roman Solovyev;
itself from lucidrains/BS-RoFormer, MIT, Copyright (c) 2023 Phil Wang). The model classes are
loaded through msst_models.py.
"""

import argparse
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import msst_models as mm  # noqa: E402

MSST_RELEASE = "https://github.com/ZFTurbo/Music-Source-Separation-Training/releases/download/"
AS_MIRROR = "https://github.com/nomadkaraoke/python-audio-separator/releases/download/model-configs/"

MODELS = {
    "denoise": {
        "out": "mel_roformer_denoise.onnx",
        "ckpt": "denoise_mel_band_roformer_aufr33_sdr_27.9959.ckpt",
        "sha256": "7c1c39191edc34e942ca7f2346ce6b6c0e1208a5f76349ffce6f696bd12910de",
        "urls": [
            MSST_RELEASE + "v.1.0.7/denoise_mel_band_roformer_aufr33_sdr_27.9959.ckpt",
            AS_MIRROR + "denoise_mel_band_roformer_aufr33_sdr_27.9959.ckpt",
        ],
        "config_urls": [
            MSST_RELEASE + "v.1.0.7/model_mel_band_roformer_denoise.yaml",
            AS_MIRROR + "denoise_mel_band_roformer_aufr33_sdr_27.9959_config.yaml",
        ],
        # The registry's first stem ("Clean"): the instrument names it may have in the config.
        "primary": ["dry", "clean", "vocals", "speech", "no noise"],
    },
    "dereverb-echo": {
        "out": "mel_roformer_dereverb_echo.onnx",
        "ckpt": "dereverb-echo_mel_band_roformer_sdr_13.4843_v2.ckpt",
        "sha256": "396432f5af25992fe82d0286634bd879027c073721db6ab10199e75459708b9f",
        "urls": [
            "https://huggingface.co/Sucial/Dereverb-Echo_Mel_Band_Roformer/resolve/main/"
            "dereverb-echo_mel_band_roformer_sdr_13.4843_v2.ckpt",
            AS_MIRROR + "dereverb-echo_mel_band_roformer_sdr_13.4843_v2.ckpt",
        ],
        "config_urls": [
            "https://huggingface.co/Sucial/Dereverb-Echo_Mel_Band_Roformer/resolve/main/"
            "config_dereverb-echo_mel_band_roformer_sdr_13.4843_v2.yaml",
            AS_MIRROR + "config_dereverb-echo_mel_band_roformer_sdr_13.4843_v2.yaml",
        ],
        "primary": ["dry", "no reverb", "noreverb", "vocals"],
    },
}

# stem-models' `roformer(0.5)`: n_fft 2048, hop 441, 352800-sample segments (801 frames).
REGISTRY = {"n_fft": 2048, "hop": 441, "segment": 352_800}

SELFTEST = {
    "audio": {"chunk_size": 63 * 128, "hop_length": 128, "n_fft": 512, "sample_rate": 44100},
    "model": {
        "dim": 16, "depth": 1, "stereo": True, "num_stems": 1, "time_transformer_depth": 1,
        "freq_transformer_depth": 1, "num_bands": 8, "dim_head": 8, "heads": 2,
        "attn_dropout": 0.0, "ff_dropout": 0.0, "flash_attn": True, "dim_freqs_in": 257,
        "sample_rate": 44100, "stft_n_fft": 512, "stft_hop_length": 128, "stft_win_length": 512,
        "mask_estimator_depth": 1,
    },
    "training": {"instruments": ["dry", "other"], "target_instrument": "dry"},
}


def spec_core(model, order):
    """`order`: which of the model's mask estimators to output, in registry order."""
    import torch
    from einops import pack, rearrange, unpack

    class Core(torch.nn.Module):
        def __init__(self, m):
            super().__init__()
            self.m = m
            self.order = list(order)
            f = m.num_bands_per_freq.shape[0]
            dc = torch.ones(f)
            if getattr(m, "zero_dc", False):
                dc[0] = 0.0
            self.register_buffer("dc", dc, persistent=False)

        def forward(self, mix_spec):
            m = self.m
            b, cc, f, t = mix_spec.shape
            ch = cc // 2
            # [b, (s c), f, t] -> [b, s, f, t, c] -> [b, (f s), t, c], as forward() arranges
            # torch.view_as_real(torch.stft(...)).
            stft_repr = mix_spec.reshape(b, ch, 2, f, t).permute(0, 1, 3, 4, 2)
            stft_repr = rearrange(stft_repr, "b s f t c -> b (f s) t c")
            x = stft_repr[:, m.freq_indices]
            x = rearrange(x, "b f t c -> b t (f c)")
            x = m.band_split(x)

            store = [None] * len(m.layers)
            for i, transformer_block in enumerate(m.layers):
                if len(transformer_block) == 3:
                    linear_transformer, time_transformer, freq_transformer = transformer_block
                    x, ft_ps = pack([x], "b * d")
                    x = linear_transformer(x)
                    (x,) = unpack(x, ft_ps, "b * d")
                else:
                    time_transformer, freq_transformer = transformer_block
                if getattr(m, "skip_connection", False):
                    for j in range(i):
                        x = x + store[j]
                x = rearrange(x, "b t f d -> b f t d")
                x, ps = pack([x], "* t d")
                x = time_transformer(x)
                (x,) = unpack(x, ps, "* t d")
                x = rearrange(x, "b f t d -> b t f d")
                x, ps = pack([x], "* f d")
                x = freq_transformer(x)
                (x,) = unpack(x, ps, "* f d")
                if getattr(m, "skip_connection", False):
                    store[i] = x

            masks = torch.stack([m.mask_estimators[k](x) for k in self.order], dim=1)
            masks = rearrange(masks, "b n t (f c) -> b n f t c", c=2)
            n = masks.shape[1]
            # Average the masks of overlapping bands: scatter-add by frequency, divide by the
            # number of bands covering each frequency (real and imaginary parts alike).
            fs = stft_repr.shape[1]
            idx = m.freq_indices.view(1, 1, -1, 1, 1).expand(b, n, -1, t, 2)
            summed = torch.zeros(b, n, fs, t, 2, dtype=masks.dtype).scatter_add(2, idx, masks)
            denom = m.num_bands_per_freq.repeat_interleave(ch).clamp(min=1e-8)
            avg = summed / denom.view(1, 1, -1, 1, 1)
            s = stft_repr.unsqueeze(1)
            re = s[..., 0] * avg[..., 0] - s[..., 1] * avg[..., 1]
            im = s[..., 0] * avg[..., 1] + s[..., 1] * avg[..., 0]
            out = torch.stack([re, im], dim=-1)  # b n (f s) t c
            out = rearrange(out, "b n (f s) t c -> b n s c f t", s=ch)
            out = out * self.dc.view(1, 1, 1, 1, -1, 1)
            return out.reshape(b, n, 2 * ch, f, t)

    return Core(model)


def stft_stacked(model, x):
    import torch

    k = model.stft_kwargs
    win = model.stft_window_fn(device=x.device)
    b, ch, n = x.shape
    spec = torch.stft(x.reshape(b * ch, n), **k, window=win, return_complex=True)
    spec = torch.view_as_real(spec)  # (b ch) f t c
    f, t = spec.shape[1], spec.shape[2]
    return spec.reshape(b, ch, f, t, 2).permute(0, 1, 4, 2, 3).reshape(b, 2 * ch, f, t)


def istft_stacked(model, spec, length):
    import torch

    k = model.stft_kwargs
    win = model.stft_window_fn(device=spec.device)
    b, n, cc, f, t = spec.shape
    s = spec.reshape(b * n * cc // 2, 2, f, t).permute(0, 2, 3, 1).contiguous()
    y = torch.istft(torch.view_as_complex(s), **k, window=win, length=length)
    return y.reshape(b, n, cc // 2, -1)


def build(mel, cfg):
    model = mel.MelBandRoformer(**dict(cfg["model"]))
    model.eval()
    return model


def outputs(cfg, primary_names):
    """Which mask estimators to export, primary first (indices into the model's stems)."""
    tr = cfg["training"]
    names = [tr["target_instrument"]] if tr.get("target_instrument") else list(tr["instruments"])
    if len(names) != cfg["model"].get("num_stems", 1):
        names = names[: cfg["model"].get("num_stems", 1)]
    low = [str(s).lower() for s in names]
    hit = [i for i, s in enumerate(low) if s in primary_names]
    if not hit:
        sys.exit(f"none of the model's stems {names} is the registry's primary ({primary_names})")
    p = hit[0]
    order = [p] + [i for i in range(len(names)) if i != p][:1]
    print(f"stems {names}: exporting {[names[i] for i in order]}")
    return order


def check_config(cfg):
    a, m = cfg["audio"], cfg["model"]
    got = {"n_fft": m.get("stft_n_fft", 2048), "hop": m.get("stft_hop_length", 512),
           "segment": a.get("chunk_size")}
    if got != REGISTRY or m.get("stft_win_length", got["n_fft"]) != got["n_fft"] \
            or m.get("stft_normalized", False) or not m.get("stereo", False):
        sys.exit(f"config {got} (stereo {m.get('stereo')}) does not match the registry's "
                 f"Mel-RoFormer geometry {REGISTRY}: add a matching `ModelArch::RoFormer` to "
                 "crates/stem-models/src/catalog.rs first")


def verify(model, core, chunk):
    import torch

    x = torch.randn(1, 2, chunk) * 0.3
    with torch.no_grad():
        ref = model(x)
        if ref.dim() == 3:
            ref = ref.unsqueeze(1)
        spec = stft_stacked(model, x)
        out = core(spec)
        y = istft_stacked(model, out, chunk)
    ref = ref[:, core.order]
    err = float((y - ref).abs().max())
    peak = float(ref.abs().max())
    print(f"wrapper vs model(audio): max err {err:.3g} (output peak {peak:.3g})")
    if err > 1e-3 * max(peak, 1e-3):
        sys.exit("the spectrogram wrapper does not reproduce the model")
    return spec, out


def selftest(mel, out_dir):
    import torch

    torch.manual_seed(0)
    cfg = SELFTEST
    model = build(mel, cfg)
    chunk = cfg["audio"]["chunk_size"]
    core = spec_core(model, [0])
    core.eval()
    spec, out = verify(model, core, chunk)
    path = mm.export_onnx(core, spec, Path(out_dir) / "mel_roformer_tiny.onnx", "mix_spec", "stems_spec")
    print(f"onnx vs torch: max err {mm.check_onnx(path, spec, out, 'mix_spec')}")
    sr, n = 44100, 40_000
    t = np.arange(n) / sr
    rng = np.random.default_rng(2)
    mix = np.stack([
        0.4 * np.sin(2 * np.pi * 440 * t) + 0.05 * rng.standard_normal(n),
        0.3 * np.sin(2 * np.pi * 660 * t) + 0.05 * rng.standard_normal(n),
    ]).astype(np.float32)

    def run(seg):
        with torch.no_grad():
            y = model(torch.from_numpy(seg)[None])
        return (y if y.dim() == 4 else y.unsqueeze(1))[0].numpy()

    overlap = 0.5
    stems = mm.reference_demix(mix, chunk, overlap, run)
    a = cfg["audio"]
    mm.write_fixture(out_dir, "mel_roformer_tiny", mix, stems, {
        "n_fft": a["n_fft"], "hop": a["hop_length"], "segment_samples": chunk,
        "overlap": overlap, "stems": stems.shape[0],
    })
    mm.report(path, f"  reference: {stems.shape} written next to it")


def load_config(spec, args):
    import yaml

    path = Path(args.config) if args.config else mm.fetch(
        spec["config_urls"], Path(args.out_dir) / "checkpoints" / (spec["out"] + ".yaml"))
    with open(path) as f:
        return yaml.load(f, Loader=yaml.FullLoader)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("model", choices=[*MODELS, "selftest"])
    ap.add_argument("out_dir")
    ap.add_argument("--ckpt", help="use this checkpoint instead of downloading it")
    ap.add_argument("--config", help="use this MSST yaml config instead of downloading it")
    ap.add_argument("--msst", help="a Music-Source-Separation-Training clone to import the model from")
    ap.add_argument("--allow-unpinned", action="store_true",
                    help="use MSST model files that differ from the tested ones")
    args = ap.parse_args()
    _, mel = mm.model_classes(args.msst, args.allow_unpinned)
    if mel is None:
        sys.exit("pip install einops rotary-embedding-torch beartype librosa")
    if args.model == "selftest":
        selftest(mel, args.out_dir)
        return
    spec = MODELS[args.model]
    cfg = load_config(spec, args)
    check_config(cfg)
    ckpt = Path(args.ckpt) if args.ckpt else mm.fetch(
        spec["urls"], Path(args.out_dir) / "checkpoints" / spec["ckpt"], spec["sha256"])
    if args.ckpt and mm.sha256(ckpt) != spec["sha256"]:
        print(f"warning: {ckpt} is not the tested checkpoint (sha256 {spec['sha256']})")
    model = build(mel, cfg)
    mm.load_state_dict(model, ckpt)
    core = spec_core(model, outputs(cfg, spec["primary"]))
    core.eval()
    example, out = verify(model, core, cfg["audio"]["chunk_size"])
    path = mm.export_onnx(core, example, Path(args.out_dir) / spec["out"], "mix_spec", "stems_spec")
    err = mm.check_onnx(path, example, out, "mix_spec")
    if err is not None:
        print(f"onnx vs torch: max err {err:.3g}")
    mm.report(path)


if __name__ == "__main__":
    main()
