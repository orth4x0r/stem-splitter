#!/usr/bin/env python3
"""Make a RoFormer ONNX export compute in fp16 (about 1.5-2x faster on an RDNA2 GPU via
DirectML), for the app's "(fast)" models.

The hosted Mel-RoFormer (audiojs/mel-roformer, mel-roformer.fp16.onnx) and BS-RoFormer
(silverdaw/bs-roformer-rhythm-onnx, bs_roformer_4stem_rhythm_fp16.onnx) store their weights in
fp16 but Cast every weight to float32, so every MatMul runs in fp32. This script:

1. folds each `fp16 initializer -> Cast(to=FLOAT)` into a plain fp32 initializer,
2. removes `Cast(to=FLOAT)` nodes on values that already are FLOAT (onnxconverter-common
   retypes their outputs to fp16 but keeps `to=FLOAT`, which makes the graph invalid),
3. runs onnxconverter-common's `convert_float_to_float16(keep_io_types=True)` with its default
   op block list. Inputs and outputs stay float32 with the same names and shapes, so the file
   is a drop-in replacement.

Measured on an RX 6700S (onnxruntime-directml 1.24.4), one 8 s segment, against the original:

    mel-roformer-kim   6.98 s -> 4.79 s (1.46x), output SNR 54.4 dB
    bs-roformer-4stem 16.09 s -> 7.45 s (2.16x), output SNR 45.0 dB
                       (drums 44.1, bass 40.5, other 42.0, vocals 47.6)

Keeping the norm / softmax ops in fp32 was tried: the extra casts cancel the speed-up.

Usage (onnxconverter-common is MIT; install it with --no-deps, its pins downgrade protobuf):

    pip install onnx numpy && pip install --no-deps onnxconverter-common
    python tools/convert_roformer_fp16.py mel-roformer.fp16.onnx mel_roformer_kim_fast.onnx
    python tools/convert_roformer_fp16.py bs_roformer_4stem_rhythm_fp16.onnx bs_roformer_4stem_fast.onnx

and put the output in the app's models folder. Convert twice and compare the hashes before
trusting (or pinning) a result: a machine with flaky RAM gives a few flipped weight bits.
"""

import sys
import time

import numpy as np
import onnx
from onnx import TensorProto, numpy_helper
from onnxconverter_common import float16


def cast_to(n: onnx.NodeProto) -> int:
    return next(a.i for a in n.attribute if a.name == "to")


def fold_weight_casts(m: onnx.ModelProto) -> onnx.ModelProto:
    """Turn every `fp16 initializer -> Cast(to=FLOAT)` into an fp32 initializer named like the
    Cast's output."""
    g = m.graph
    inits = {i.name: i for i in g.initializer}
    # Track nodes by position: iterating a protobuf repeated field makes temporary wrappers
    # whose id() values get reused, so an id() set silently drops or keeps the wrong nodes.
    new_inits, drop = [], set()
    for idx, n in enumerate(g.node):
        if n.op_type != "Cast" or len(n.input) != 1 or n.input[0] not in inits:
            continue
        src = inits[n.input[0]]
        if cast_to(n) != TensorProto.FLOAT or src.data_type != TensorProto.FLOAT16:
            continue
        arr = numpy_helper.to_array(src).astype(np.float32)
        new_inits.append(numpy_helper.from_array(arr, n.output[0]))
        drop.add(idx)
    kept = [n for idx, n in enumerate(g.node) if idx not in drop]
    del g.node[:]
    g.node.extend(kept)
    g.initializer.extend(new_inits)
    # Drop the fp16 initializers nothing reads any more.
    used = {x for n in g.node for x in n.input} | {o.name for o in g.output}
    keep = [i for i in g.initializer if i.name in used]
    del g.initializer[:]
    g.initializer.extend(keep)
    print(f"  folded {len(drop)} weight casts")
    return m


def drop_redundant_float_casts(m: onnx.ModelProto) -> onnx.ModelProto:
    """Remove Cast(to=FLOAT) nodes whose input is already FLOAT, rewiring their consumers."""
    m = onnx.shape_inference.infer_shapes(m)
    g = m.graph
    types = {
        v.name: v.type.tensor_type.elem_type
        for v in list(g.value_info) + list(g.input) + list(g.output)
    }
    types.update({i.name: i.data_type for i in g.initializer})
    outputs = {o.name for o in g.output}
    rename, kept = {}, []
    for n in g.node:
        for k, x in enumerate(n.input):
            if x in rename:
                n.input[k] = rename[x]
        if (
            n.op_type == "Cast"
            and cast_to(n) == TensorProto.FLOAT
            and types.get(n.input[0]) == TensorProto.FLOAT
            and n.output[0] not in outputs
        ):
            rename[n.output[0]] = n.input[0]
            continue
        kept.append(n)
    del g.node[:]
    g.node.extend(kept)
    keep_vi = [v for v in g.value_info if v.name not in rename]
    del g.value_info[:]
    g.value_info.extend(keep_vi)
    print(f"  dropped {len(rename)} redundant FLOAT->FLOAT casts")
    return m


def main(src: str, dst: str) -> None:
    t = time.time()
    m = onnx.load(src)
    print(f"loaded {src} ({len(m.graph.node)} nodes) in {time.time() - t:.1f}s")
    m = drop_redundant_float_casts(fold_weight_casts(m))
    t = time.time()
    m = float16.convert_float_to_float16(m, keep_io_types=True)
    print(f"  converted to fp16 in {time.time() - t:.1f}s")
    onnx.save_model(m, dst, save_as_external_data=False)
    print(f"saved {dst}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
