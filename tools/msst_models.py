"""Shared helpers for the MSST-based ONNX export scripts (export_mdx23c_onnx.py,
export_mel_roformer_onnx.py).

The model classes come from ZFTurbo's Music-Source-Separation-Training (MSST, MIT licence,
Copyright (c) 2024 Roman Solovyev). Nothing of MSST is copied into this repository: either
pass `--msst DIR` (a clone of github.com/ZFTurbo/Music-Source-Separation-Training) or let
`model_classes()` fetch the three model files it needs from raw.githubusercontent.com into
`tools/.msst-cache/` (checked against the sha256 below; a changed upstream file is reported
and used only with `--allow-unpinned`). MSST's `utils/model_utils.py` pulls in its training
stack, so the cache gets a stand-in with the one function the model files import.

Also here: checkpoint download with sha256 checks, a tiny attribute-dict config, and a NumPy
port of stem-models' chunking (crates/stem-models/src/roformer.rs, `SpecParams::plan` /
`weight` / `cut_segment`) used to build the reference output the Rust integration test
compares against.
"""

import hashlib
import importlib
import shutil
import sys
import urllib.request
from pathlib import Path

import numpy as np

MSST_RAW = "https://raw.githubusercontent.com/ZFTurbo/Music-Source-Separation-Training/main/"

# sha256 of the MSST files these scripts were written and tested against (fetched 2026-11-09).
MSST_FILES = {
    "models/mdx23c_tfc_tdf_v3.py": "5b29c37cbba4b06e49dcd6bef668d372501df2517392b71b93804355cb7d535e",
    "models/bs_roformer/attend.py": "c7abbc40a3fd20ff7f6001fa9f8ee9ad5df6452272712a5e719b8f8468bf2223",
    "models/bs_roformer/mel_band_roformer.py": "a28fce62020faad6461176660e012cf5799ae8a26c76fe4b2bc636f5b44b9fc5",
}

# Stand-in for MSST's utils/model_utils.py (only what the model files import).
MODEL_UTILS_SHIM = '''
def prefer_target_instrument(config):
    if getattr(config.training, "target_instrument", None):
        return [config.training.target_instrument]
    return config.training.instruments
'''


class Cfg(dict):
    """`config.audio.n_fft`-style access over nested dicts (like ml_collections.ConfigDict)."""

    def __getattr__(self, k):
        try:
            v = self[k]
        except KeyError as e:
            raise AttributeError(k) from e
        return Cfg(v) if isinstance(v, dict) else v

    def get(self, k, d=None):
        v = super().get(k, d)
        return Cfg(v) if isinstance(v, dict) else v


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def fetch(urls, dest, want_sha=None):
    """Download the first URL that works to `dest` (skipped when it is there with the right
    hash). Returns `dest`."""
    dest = Path(dest)
    if dest.is_file() and (want_sha is None or sha256(dest) == want_sha):
        return dest
    dest.parent.mkdir(parents=True, exist_ok=True)
    errors = []
    for url in urls:
        part = dest.with_suffix(dest.suffix + ".part")
        try:
            print(f"downloading {url}")
            with urllib.request.urlopen(url) as r, open(part, "wb") as f:
                shutil.copyfileobj(r, f, 1 << 20)
        except Exception as e:  # noqa: BLE001 - try the next mirror
            errors.append(f"{url}: {e}")
            part.unlink(missing_ok=True)
            continue
        got = sha256(part)
        if want_sha and got != want_sha:
            errors.append(f"{url}: sha256 {got}, expected {want_sha}")
            part.unlink(missing_ok=True)
            continue
        part.replace(dest)
        return dest
    sys.exit("could not download " + dest.name + ":\n  " + "\n  ".join(errors))


def model_classes(msst_dir=None, allow_unpinned=False):
    """Import MSST's model modules. Returns (mdx23c_module, mel_band_roformer_module)."""
    if msst_dir:
        root = Path(msst_dir).resolve()
        if not (root / "models" / "mdx23c_tfc_tdf_v3.py").is_file():
            sys.exit(f"{root} is not a Music-Source-Separation-Training clone")
    else:
        root = Path(__file__).resolve().parent / ".msst-cache"
        for rel, want in MSST_FILES.items():
            dest = root / rel
            if not dest.is_file():
                fetch([MSST_RAW + rel], dest)
            got = sha256(dest)
            if got != want:
                msg = (
                    f"{rel} changed upstream (sha256 {got}, tested {want}); check the diff, "
                    "then rerun with --allow-unpinned, or pass --msst with a clone"
                )
                if not allow_unpinned:
                    sys.exit(msg)
                print("warning: " + msg)
        for pkg in ["models", "models/bs_roformer", "utils"]:
            (root / pkg).mkdir(parents=True, exist_ok=True)
            (root / pkg / "__init__.py").touch()
        (root / "utils" / "model_utils.py").write_text(MODEL_UTILS_SHIM)
    sys.path.insert(0, str(root))
    mdx23c = importlib.import_module("models.mdx23c_tfc_tdf_v3")
    try:
        mel = importlib.import_module("models.bs_roformer.mel_band_roformer")
    except ImportError as e:
        print(f"note: Mel-RoFormer unavailable ({e}); pip install einops rotary-embedding-torch beartype librosa")
        mel = None
    return mdx23c, mel


def load_state_dict(model, ckpt_path):
    import torch

    state = torch.load(ckpt_path, map_location="cpu", weights_only=False)
    for key in ("state_dict", "state", "model"):
        if isinstance(state, dict) and key in state and isinstance(state[key], dict):
            state = state[key]
    state = {k.removeprefix("module."): v for k, v in state.items()}
    missing, unexpected = model.load_state_dict(state, strict=False)
    # Non-persistent buffers (freq_indices etc.) are rebuilt by __init__.
    missing = [k for k in missing if not k.endswith(("freq_indices", "freqs_per_band"))]
    if missing or unexpected:
        sys.exit(f"checkpoint does not fit the config: missing {missing[:8]}, unexpected {unexpected[:8]}")


def export_onnx(core, example, path, in_name, out_name):
    import torch

    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with torch.no_grad():
        torch.onnx.export(
            core,
            (example,),
            str(path),
            input_names=[in_name],
            output_names=[out_name],
            opset_version=17,
            do_constant_folding=True,
            dynamo=False,
        )
    return path


def check_onnx(path, example, expected, in_name):
    """Run the graph with onnxruntime (if installed); returns the max abs error, or None."""
    try:
        import onnxruntime as ort
    except ImportError:
        print("onnxruntime not installed: skipped the ONNX check")
        return None
    sess = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    got = sess.run(None, {in_name: example.numpy()})[0]
    return float(np.abs(got.reshape(expected.shape) - expected.numpy()).max())


def report(path, extra=""):
    size = Path(path).stat().st_size
    print(f"\nwrote {path}\n  size_bytes: {size}\n  sha256:     {sha256(path)}")
    if extra:
        print(extra)


# ---- stem-models' chunking, in NumPy (for the reference output) ----------------------------


def reflect_index(i, n):
    if n == 1:
        return 0
    period = 2 * (n - 1)
    j = i % period
    return period - j if j >= n else j


def rust_plan(n, segment, overlap):
    """`SpecParams::new` + `plan` for a RoFormer / MDX23C arch: (step, fade, border, starts)."""
    step = min(max(int((1.0 - overlap) * segment), 1), segment)
    fade = min(segment // 10, segment - step)
    border = segment - step
    if not (border > 0 and n > 2 * border):
        border = 0
    length = n + 2 * border
    starts, s = [], 0
    while True:
        starts.append(s)
        if s + segment >= length:
            break
        s += step
    return step, fade, border, starts


def rust_weight(k, count, i, segment, fade):
    w = 1.0
    if fade >= 2:
        d = fade - 1
        if k > 0 and i < fade:
            w = i / d
        if k + 1 < count and i >= segment - fade:
            w = min(w, (segment - 1 - i) / d)
    return w


def reference_demix(x, segment, overlap, run_segment):
    """Overlap-add `run_segment([2, L] array) -> [S, 2, L]` over `x` ([2, n]) exactly as
    stem-models does. Returns [S, 2, n]."""
    n = x.shape[1]
    step, fade, border, starts = rust_plan(n, segment, overlap)
    if border:
        idx = [reflect_index(i - border, n) for i in range(n + 2 * border)]
        src = x[:, idx]
    else:
        src = x
    length = src.shape[1]
    count = len(starts)
    acc, wsum = None, np.zeros(length)
    for k, s in enumerate(starts):
        real = min(segment, length - s)
        seg = np.zeros((2, segment), dtype=np.float32)
        seg[:, :real] = src[:, s : s + real]
        if real > segment // 2:
            for i in range(real, segment):
                seg[:, i] = src[:, s + reflect_index(i, real)]
        out = run_segment(seg)
        if acc is None:
            acc = np.zeros((out.shape[0], 2, length))
        w = np.array([rust_weight(k, count, i, segment, fade) for i in range(real)])
        acc[:, :, s : s + real] += out[:, :, :real] * w
        wsum[s : s + real] += w
    out = acc[:, :, border : border + n] / np.maximum(wsum[border : border + n], 1e-12)
    return out.astype(np.float32)


def write_fixture(dir_, name, mix, stems, params):
    """`<name>.onnx` sits next to `<name>_mix.f32` ([2, n] planar little-endian float32),
    `<name>_stems.f32` ([S, 2, n]) and `<name>.json` (the arch parameters)."""
    import json

    dir_ = Path(dir_)
    mix.astype("<f4").tofile(dir_ / f"{name}_mix.f32")
    stems.astype("<f4").tofile(dir_ / f"{name}_stems.f32")
    (dir_ / f"{name}.json").write_text(json.dumps(params, indent=2))
