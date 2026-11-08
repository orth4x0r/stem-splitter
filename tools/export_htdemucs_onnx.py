#!/usr/bin/env python3
"""Export HTDemucs (Demucs v4) to ONNX with the STFT/iSTFT *outside* the graph.

torch.stft / torch.istft with complex outputs do not export to ONNX, so - like
sevagh/demucs.onnx (MIT) - we export only the network between HTDemucs' `_spec`/`_magnitude`
and its `_mask`/`_ispec`. stem-models does the spectrogram maths in Rust (crates/stem-models/
src/demucs.rs mirrors `standalone_spec` / `standalone_ispec` below).

Graph contract (what the Rust side expects):

    inputs   mix   float32 [B, 2, 343980]          the (normalised) stereo segment
             spec  float32 [B, 4, 2048, 336]       _magnitude(_spec(mix)), cac channels
                                                   (L.re, L.im, R.re, R.im)
    outputs  freq  float32 [B, S, 4, 2048, 336]    frequency branch, de-normalised
             time  float32 [B, S, 2, 343980]       time branch, de-normalised

    stem = time + _ispec(_mask(freq))

B is fixed to 1 unless --dynamic-batch is given (the batch-dim symbol is then "batch").

Usage (needs torch, demucs, onnx, onnxruntime; downloads the weights on first run):

    pip install torch demucs onnx onnxruntime
    python tools/export_htdemucs_onnx.py out/            # -> out/htdemucs.onnx
    python tools/export_htdemucs_onnx.py out/ --six      # -> out/htdemucs_6s.onnx

The script checks the export against the original PyTorch model (`model(mix)`) and, if
onnxruntime is installed, the ONNX graph against the PyTorch wrapper. Copy the .onnx file into
the app's models folder (next to the MDX models).

The forward body of `CoreHTDemucs` is copied from facebookresearch/demucs `htdemucs.py`
(MIT, Copyright (c) Meta Platforms, Inc. and affiliates).
"""

import argparse
import math
import sys
from pathlib import Path

import torch
from torch import nn
from torch.nn import functional as F

SEGMENT = 343980  # int(7.8 * 44100), HTDemucs' training segment
NFFT = 4096
HOP = 1024


def standalone_spec(x, nfft=NFFT, hop_length=HOP):
    """HTDemucs._spec: reflect-pad so frames == ceil(len / hop), normalised STFT."""
    from demucs.hdemucs import pad1d
    from demucs.spec import spectro

    hl = hop_length
    assert hl == nfft // 4
    le = int(math.ceil(x.shape[-1] / hl))
    pad = hl // 2 * 3
    x = pad1d(x, (pad, pad + le * hl - x.shape[-1]), mode="reflect")
    z = spectro(x, nfft, hl)[..., :-1, :]
    assert z.shape[-1] == le + 4, (z.shape, x.shape, le)
    return z[..., 2 : 2 + le]


def standalone_magnitude(z):
    """HTDemucs._magnitude with cac=True: complex -> 2 channels per audio channel."""
    B, C, Fr, T = z.shape
    m = torch.view_as_real(z).permute(0, 1, 4, 2, 3)
    return m.reshape(B, C * 2, Fr, T)


def standalone_mask(m):
    """HTDemucs._mask with cac=True: [B, S, C*2, F, T] -> complex [B, S, C, F, T]."""
    B, S, C, Fr, T = m.shape
    out = m.view(B, S, -1, 2, Fr, T).permute(0, 1, 2, 4, 5, 3)
    return torch.view_as_complex(out.contiguous())


def standalone_ispec(z, length, hop_length=HOP):
    """HTDemucs._ispec (scale=0)."""
    from demucs.spec import ispectro

    hl = hop_length
    z = F.pad(z, (0, 0, 0, 1))
    z = F.pad(z, (2, 2))
    pad = hl // 2 * 3
    le = hl * int(math.ceil(length / hl)) + 2 * pad
    x = ispectro(z, hl, length=le)
    return x[..., pad : pad + length]


class CoreHTDemucs(nn.Module):
    """HTDemucs.forward without _spec/_magnitude (input) and _mask/_ispec (output)."""

    def __init__(self, model):
        super().__init__()
        self.m = model

    def forward(self, mix, mag):
        from einops import rearrange

        self_ = self.m
        x = mag
        B, C, Fq, T = x.shape
        length = mix.shape[-1]

        # unlike previous Demucs, we always normalize because it is easier.
        mean = x.mean(dim=(1, 2, 3), keepdim=True)
        std = x.std(dim=(1, 2, 3), keepdim=True)
        x = (x - mean) / (1e-5 + std)

        xt = mix
        meant = xt.mean(dim=(1, 2), keepdim=True)
        stdt = xt.std(dim=(1, 2), keepdim=True)
        xt = (xt - meant) / (1e-5 + stdt)

        saved = []
        saved_t = []
        lengths = []
        lengths_t = []
        for idx, encode in enumerate(self_.encoder):
            lengths.append(x.shape[-1])
            inject = None
            if idx < len(self_.tencoder):
                lengths_t.append(xt.shape[-1])
                tenc = self_.tencoder[idx]
                xt = tenc(xt)
                if not tenc.empty:
                    saved_t.append(xt)
                else:
                    inject = xt
            x = encode(x, inject)
            if idx == 0 and self_.freq_emb is not None:
                frs = torch.arange(x.shape[-2], device=x.device)
                emb = self_.freq_emb(frs).t()[None, :, :, None].expand_as(x)
                x = x + self_.freq_emb_scale * emb
            saved.append(x)

        if self_.crosstransformer:
            if self_.bottom_channels:
                b, c, f, t = x.shape
                x = rearrange(x, "b c f t-> b c (f t)")
                x = self_.channel_upsampler(x)
                x = rearrange(x, "b c (f t)-> b c f t", f=f)
                xt = self_.channel_upsampler_t(xt)

            x, xt = self_.crosstransformer(x, xt)

            if self_.bottom_channels:
                x = rearrange(x, "b c f t-> b c (f t)")
                x = self_.channel_downsampler(x)
                x = rearrange(x, "b c (f t)-> b c f t", f=f)
                xt = self_.channel_downsampler_t(xt)

        for idx, decode in enumerate(self_.decoder):
            skip = saved.pop(-1)
            x, pre = decode(x, skip, lengths.pop(-1))
            offset = self_.depth - len(self_.tdecoder)
            if idx >= offset:
                tdec = self_.tdecoder[idx - offset]
                length_t = lengths_t.pop(-1)
                if tdec.empty:
                    assert pre.shape[2] == 1, pre.shape
                    pre = pre[:, :, 0]
                    xt, _ = tdec(pre, None, length_t)
                else:
                    skip = saved_t.pop(-1)
                    xt, _ = tdec(xt, skip, length_t)

        assert len(saved) == 0
        assert len(lengths_t) == 0
        assert len(saved_t) == 0

        S = len(self_.sources)
        x = x.view(B, S, -1, Fq, T)
        x = x * std[:, None] + mean[:, None]

        xt = xt.view(B, S, -1, length)
        xt = xt * stdt[:, None] + meant[:, None]
        return x, xt


def full_from_core(core, mix):
    """What the Rust side computes: spec -> core -> mask -> ispec + time branch."""
    mag = standalone_magnitude(standalone_spec(mix))
    freq, time = core(mix, mag)
    return time + standalone_ispec(standalone_mask(freq), mix.shape[-1])


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dest_dir", type=Path)
    ap.add_argument("--six", action="store_true", help="export htdemucs_6s (default: htdemucs)")
    ap.add_argument("--dynamic-batch", action="store_true", help="make the batch dimension dynamic")
    ap.add_argument("--opset", type=int, default=17)
    ap.add_argument("--skip-check", action="store_true")
    args = ap.parse_args()

    from demucs.htdemucs import HTDemucs
    from demucs.pretrained import get_model

    name = "htdemucs_6s" if args.six else "htdemucs"
    bag = get_model(name)
    if isinstance(bag, HTDemucs):
        model = bag
    elif hasattr(bag, "models") and len(bag.models) == 1 and isinstance(bag.models[0], HTDemucs):
        model = bag.models[0]
    else:
        sys.exit(f"{name}: expected a single HTDemucs model, got {type(bag)}")
    model.eval()
    assert model.samplerate == 44100 and model.audio_channels == 2
    assert model.cac, "only complex-as-channels HTDemucs is supported"
    assert model.nfft == NFFT and model.hop_length == HOP
    segment = int(model.segment * model.samplerate)
    assert segment == SEGMENT, segment
    print(f"{name}: sources {model.sources}, segment {segment}")

    core = CoreHTDemucs(model).eval()
    torch.manual_seed(0)
    mix = torch.randn(1, 2, SEGMENT) * 0.3
    mag = standalone_magnitude(standalone_spec(mix))
    assert tuple(mag.shape) == (1, 4, NFFT // 2, math.ceil(SEGMENT / HOP)), mag.shape

    if not args.skip_check:
        with torch.no_grad():
            ref = model(mix)
            got = full_from_core(core, mix)
        err = (ref - got).abs().max().item()
        print(f"wrapper vs original model: max abs error {err:.3e}")
        assert err < 1e-4, "wrapper does not reproduce HTDemucs.forward"

    args.dest_dir.mkdir(parents=True, exist_ok=True)
    out = args.dest_dir / f"{name}.onnx"
    dynamic_axes = None
    if args.dynamic_batch:
        dynamic_axes = {n: {0: "batch"} for n in ("mix", "spec", "freq", "time")}
    export_kwargs = dict(
        export_params=True,
        opset_version=args.opset,
        do_constant_folding=True,
        input_names=["mix", "spec"],
        output_names=["freq", "time"],
        dynamic_axes=dynamic_axes,
    )
    # The TorchScript exporter is the one known to handle HTDemucs (newer torch defaults to
    # the dynamo exporter).
    try:
        torch.onnx.export(core, (mix, mag), str(out), dynamo=False, **export_kwargs)
    except TypeError:  # torch < 2.5 has no `dynamo` argument
        torch.onnx.export(core, (mix, mag), str(out), **export_kwargs)
    print(f"wrote {out} ({out.stat().st_size / 1e6:.1f} MB)")

    if args.skip_check:
        return
    try:
        import onnxruntime as ort
    except ImportError:
        print("onnxruntime not installed; skipping the ONNX check")
        return
    sess = ort.InferenceSession(str(out), providers=["CPUExecutionProvider"])
    print("inputs:", [(i.name, i.shape) for i in sess.get_inputs()])
    print("outputs:", [(o.name, o.shape) for o in sess.get_outputs()])
    freq, time = sess.run(["freq", "time"], {"mix": mix.numpy(), "spec": mag.numpy()})
    with torch.no_grad():
        rf, rt = core(mix, mag)
    ef = abs(freq - rf.numpy()).max()
    et = abs(time - rt.numpy()).max()
    print(f"onnx vs torch: freq {ef:.3e}, time {et:.3e}")
    assert ef < 1e-3 and et < 1e-3, "ONNX output differs from PyTorch"
    print("ok")


if __name__ == "__main__":
    main()
