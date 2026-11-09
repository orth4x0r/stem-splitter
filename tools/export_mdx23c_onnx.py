#!/usr/bin/env python3
"""Export an MDX23C (TFC-TDF v3) checkpoint to ONNX with the STFT/iSTFT *outside* the graph,
for stem-models' `ModelArch::Mdx23c` (crates/stem-models/src/mdx23c.rs).

Models:

    drumsep    aufr33 & jarredou DrumSep, 6 stems: kick, snare, toms, hh, ride, crash
               -> drumsep_mdx23c.onnx       (registry id "drumsep-mdx23c")
    dereverb   aufr33 & jarredou De-Reverb: dry ("No Reverb"), reverb
               -> mdx23c_dereverb.onnx      (registry id "mdx23c-dereverb")

Neither checkpoint has a stated licence: they are "personal use" models in the app.

Graph contract (what the Rust side expects):

    input   mix_spec    float32 [1, 4, dim_f, T]      L.re, L.im, R.re, R.im of the lowest
                                                      dim_f bins of the STFT
    output  stems_spec  float32 [1, S, 4, dim_f, T]   per source, same layout

where the STFT is MSST's `STFT` class: periodic Hann, n_fft, hop, center=True, reflect pad,
unnormalised, and T = dim_t = chunk_size / hop + 1. The exporter replaces the model's `stft`
object with a pass-through, so the graph is exactly `TFC_TDF_net.forward` between the STFT and
the iSTFT. Before writing anything it checks that the wrapped graph plus a torch STFT/iSTFT
reproduces `model(audio)`, and (with onnxruntime) that the ONNX graph matches PyTorch.

Usage (Python 3.10+):

    pip install torch onnx onnxruntime pyyaml numpy
    python tools/export_mdx23c_onnx.py drumsep  out/     # downloads the checkpoint (sha256 checked)
    python tools/export_mdx23c_onnx.py dereverb out/
    python tools/export_mdx23c_onnx.py drumsep  out/ --ckpt path/to/file.ckpt   # offline

Then copy the .onnx into the app's models folder (%LOCALAPPDATA%\\orth4x0r\\Stem Splitter\\data\\
models). The script prints the file size and sha256; paste them into the registry entry
(crates/stem-models/src/catalog.rs) if the file is ever hosted.

    python tools/export_mdx23c_onnx.py selftest crates/stem-models/tests/fixtures/

builds a tiny random-weight MDX23C instead, and writes it with a reference separation
(stem-models' chunking around the *original* `model(audio)` forward) for
`cargo test -p stem-models --test msst_reference` (needs ORT_DYLIB_PATH).

Model code: ZFTurbo/Music-Source-Separation-Training (MIT), loaded through msst_models.py.
"""

import argparse
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import msst_models as mm  # noqa: E402

TFC_TDF = {
    "act": "gelu",
    "bottleneck_factor": 4,
    "growth": 128,
    "norm": "InstanceNorm",
    "num_blocks_per_scale": 2,
    "num_channels": 128,
    "num_scales": 5,
    "num_subbands": 4,
    "scale": [2, 2],
}

# The published configs' model-relevant parts (aufr33-jarredou_DrumSep_model_mdx23c_ep_141_
# sdr_10.8059.yaml and config_dereverb_mdx23c.yaml, from TRvlvr/application_data).
MODELS = {
    "drumsep": {
        "out": "drumsep_mdx23c.onnx",
        "ckpt": "aufr33-jarredou_DrumSep_model_mdx23c_ep_141_sdr_10.8059.ckpt",
        "sha256": "d2a4aa53eb584d21eead358a4e66d1882ad182911be018f052b5da73be9096d0",
        "urls": [
            "https://github.com/openmirlab/mdxnet-infer/releases/download/weights-drumsep-v1/"
            "aufr33-jarredou_DrumSep_model_mdx23c_ep_141_sdr_10.8059.ckpt",
            "https://github.com/nomadkaraoke/python-audio-separator/releases/download/"
            "model-configs/MDX23C-DrumSep-aufr33-jarredou.ckpt",
        ],
        "config": {
            "audio": {"chunk_size": 130560, "dim_f": 1024, "dim_t": 256, "hop_length": 512,
                      "n_fft": 2048, "num_channels": 2, "sample_rate": 44100},
            "model": TFC_TDF,
            "training": {"instruments": ["Kick", "Snare", "Toms", "Hh", "Ride", "Crash"],
                         "target_instrument": None},
        },
    },
    "dereverb": {
        "out": "mdx23c_dereverb.onnx",
        "ckpt": "dereverb_mdx23c_sdr_6.9096.ckpt",
        "sha256": "eae2471b707758d74db38ac1b1d5800e12f57c4e9d1ebbb2faf004b8e086e914",
        "urls": [
            "https://huggingface.co/noblebarkrr/mvsepless_resources/resolve/"
            "cf01b871f6c324c11c828b18ad05658656e9ae7f/mdx23c/mdx23c_dereverb_aufr33_jarredou.ckpt",
            "https://github.com/nomadkaraoke/python-audio-separator/releases/download/"
            "model-configs/MDX23C-De-Reverb-aufr33-jarredou.ckpt",
        ],
        "config": {
            "audio": {"chunk_size": 261120, "dim_f": 4096, "dim_t": 256, "hop_length": 1024,
                      "n_fft": 8192, "num_channels": 2, "sample_rate": 44100},
            "model": TFC_TDF,
            "training": {"instruments": ["No Reverb", "Reverb"], "target_instrument": None},
        },
    },
}

# What stem-models' registry says (crates/stem-models/src/mdx23c.rs DRUMSEP / DEREVERB).
REGISTRY = {
    "drumsep": {"n_fft": 2048, "hop": 512, "dim_f": 1024, "dim_t": 256, "stems": 6},
    "dereverb": {"n_fft": 8192, "hop": 1024, "dim_f": 4096, "dim_t": 256, "stems": 2},
}

# Tiny random-weight model for the self-test (must match tests/msst_reference.rs).
SELFTEST = {
    "audio": {"chunk_size": 63 * 128, "dim_f": 192, "dim_t": 64, "hop_length": 128,
              "n_fft": 512, "num_channels": 2, "sample_rate": 44100},
    "model": dict(TFC_TDF, growth=4, num_channels=8, num_scales=2),
    "training": {"instruments": ["Kick", "Snare", "Toms"], "target_instrument": None},
}


class PassThrough:
    """Stands in for MSST's `STFT`: the graph's input already is the cropped spectrogram and its
    output stays one (`STFT.inverse` runs in Rust)."""

    def __call__(self, x):
        return x

    def inverse(self, x):
        return x


def spec_core(model):
    import torch

    class Core(torch.nn.Module):
        def __init__(self, m):
            super().__init__()
            self.m = m
            self.m.stft = PassThrough()

        def forward(self, mix_spec):
            out = self.m(mix_spec)
            if out.dim() == 4:  # one target instrument: [B, 4, F, T]
                out = out.unsqueeze(1)
            return out

    return Core(model)


def build(mdx23c, cfg):
    model = mdx23c.TFC_TDF_net(mm.Cfg(cfg))
    model.eval()
    return model


def check_config(name, cfg):
    a, want = cfg["audio"], REGISTRY[name]
    got = {"n_fft": a["n_fft"], "hop": a["hop_length"], "dim_f": a["dim_f"],
           "dim_t": a["chunk_size"] // a["hop_length"] + 1,
           "stems": 1 if cfg["training"].get("target_instrument") else len(cfg["training"]["instruments"])}
    if got != want:
        sys.exit(f"config {got} does not match the registry {want}: update "
                 "crates/stem-models/src/mdx23c.rs and catalog.rs first")


def verify(model, cfg):
    """The wrapped graph + torch STFT / iSTFT must equal the original model on audio."""
    import torch

    stft = model.stft
    x = torch.randn(1, 2, cfg["audio"]["chunk_size"]) * 0.3
    with torch.no_grad():
        ref = model(x)
        spec = stft(x)
        core = spec_core(model)
        out = core(spec)
        y = stft.inverse(out).reshape(ref.shape)
    err = float((y - ref).abs().max())
    scale = float(ref.abs().max())
    print(f"wrapper vs model(audio): max err {err:.3g} (output peak {scale:.3g})")
    if err > 1e-3 * max(scale, 1e-3):
        sys.exit("the spectrogram wrapper does not reproduce the model")
    return core, spec, out


def decompose_instance_norms(model):
    """Swap every InstanceNorm2d for the same maths written as ReduceMean/Sub/Mul/Sqrt.

    DirectML's InstanceNormalization returns NaN on RDNA2 GPUs for some shapes (seen on an
    RX 6700S and a Radeon 680M with [1, 256, 128, 512] in the de-reverb model), so the exported
    graph avoids that operator altogether. Output is identical up to float rounding.
    """
    import torch

    class Manual(torch.nn.Module):
        def __init__(self, norm):
            super().__init__()
            self.eps = norm.eps
            self.affine = norm.affine
            if norm.affine:
                self.weight = norm.weight
                self.bias = norm.bias

        def forward(self, x):
            mean = x.mean(dim=(2, 3), keepdim=True)
            centred = x - mean
            var = (centred * centred).mean(dim=(2, 3), keepdim=True)
            y = centred / torch.sqrt(var + self.eps)
            if self.affine:
                y = y * self.weight[None, :, None, None] + self.bias[None, :, None, None]
            return y

    swapped = 0
    for parent in list(model.modules()):
        for name, child in list(parent.named_children()):
            if isinstance(child, torch.nn.InstanceNorm2d):
                if child.track_running_stats:
                    sys.exit(f"{name}: InstanceNorm2d with running stats is not supported here")
                setattr(parent, name, Manual(child))
                swapped += 1
    print(f"replaced {swapped} InstanceNorm2d layers with plain ops (DirectML workaround)")
    return model


def selftest(mdx23c, out_dir):
    import torch

    torch.manual_seed(0)
    cfg = SELFTEST
    model = build(mdx23c, cfg)
    # Random affine norms too, so nothing is accidentally an identity.
    for mod in model.modules():
        if isinstance(mod, torch.nn.InstanceNorm2d):
            mod.weight.data.uniform_(0.5, 1.5)
            mod.bias.data.uniform_(-0.1, 0.1)
    stft = model.stft
    core, spec, out = verify(model, cfg)
    path = mm.export_onnx(core, spec, Path(out_dir) / "mdx23c_tiny.onnx", "mix_spec", "stems_spec")
    print(f"onnx vs torch: max err {mm.check_onnx(path, spec, out, 'mix_spec')}")
    # A 1.3 s stereo mix of tones and noise, separated with the original forward (STFT inside).
    sr, n = 44100, 57_000
    t = np.arange(n) / sr
    rng = np.random.default_rng(1)
    mix = np.stack([
        0.4 * np.sin(2 * np.pi * 220 * t) + 0.1 * rng.standard_normal(n),
        0.3 * np.sin(2 * np.pi * 330 * t + 1) + 0.1 * rng.standard_normal(n),
    ]).astype(np.float32)

    def run(seg):
        with torch.no_grad():
            return model_full(torch.from_numpy(seg)[None])[0].numpy()

    model_full = build(mdx23c, cfg)
    model_full.load_state_dict(core.m.state_dict())
    model_full.stft = stft
    model_full.eval()
    overlap = 0.5
    stems = mm.reference_demix(mix, cfg["audio"]["chunk_size"], overlap, run)
    a = cfg["audio"]
    mm.write_fixture(out_dir, "mdx23c_tiny", mix, stems, {
        "n_fft": a["n_fft"], "hop": a["hop_length"], "dim_f": a["dim_f"],
        "segment_samples": a["chunk_size"], "overlap": overlap, "stems": stems.shape[0],
    })
    mm.report(path, f"  reference: {stems.shape} written next to it")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("model", choices=[*MODELS, "selftest"])
    ap.add_argument("out_dir")
    ap.add_argument("--ckpt", help="use this checkpoint instead of downloading it")
    ap.add_argument("--msst", help="a Music-Source-Separation-Training clone to import the model from")
    ap.add_argument("--allow-unpinned", action="store_true",
                    help="use MSST model files that differ from the tested ones")
    ap.add_argument("--keep-instance-norm", action="store_true",
                    help="export InstanceNormalization nodes as-is (they give NaN on DirectML/RDNA2)")
    ap.add_argument("--random-weights", action="store_true",
                    help="skip the checkpoint (checks the export and its size, not a usable model)")
    args = ap.parse_args()
    mdx23c, _ = mm.model_classes(args.msst, args.allow_unpinned)
    if args.model == "selftest":
        selftest(mdx23c, args.out_dir)
        return
    spec = MODELS[args.model]
    cfg = spec["config"]
    check_config(args.model, cfg)
    model = build(mdx23c, cfg)
    if args.random_weights:
        print("random weights: the file is only good for checking shapes and size")
    else:
        ckpt = Path(args.ckpt) if args.ckpt else mm.fetch(
            spec["urls"], Path(args.out_dir) / "checkpoints" / spec["ckpt"], spec["sha256"])
        if args.ckpt and mm.sha256(ckpt) != spec["sha256"]:
            print(f"warning: {ckpt} is not the tested checkpoint (sha256 {spec['sha256']})")
        mm.load_state_dict(model, ckpt)
    if not args.keep_instance_norm:
        decompose_instance_norms(model)
    core, example, out = verify(model, cfg)
    path = mm.export_onnx(core, example, Path(args.out_dir) / spec["out"], "mix_spec", "stems_spec")
    err = mm.check_onnx(path, example, out, "mix_spec")
    if err is not None:
        print(f"onnx vs torch: max err {err:.3g}")
    mm.report(path)


if __name__ == "__main__":
    main()
