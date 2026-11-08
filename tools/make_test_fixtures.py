#!/usr/bin/env python3
"""Build tiny synthetic ONNX graphs with the exact I/O contracts of our separators, for the
stem-models integration tests (crates/stem-models/tests/ort_pipeline.rs).

    pip install onnx numpy
    python tools/make_test_fixtures.py            # -> crates/stem-models/tests/fixtures/
    ORT_DYLIB_PATH=/path/to/libonnxruntime.so cargo test -p stem-models --test ort_pipeline

(`pip download onnxruntime` gives a wheel whose onnxruntime/capi/ holds libonnxruntime.so.)

Fixtures (parameters must match the test):

* mdx_half.onnx - MDX-Net contract: input "input" [batch_size, 4, 400, 64] -> output
  "output" = 0.5 * input. With n_fft 1024 / hop 256 the separator must return
  primary = 0.5 * compensate * mix (band-limited) and secondary = mix - primary.
* htdemucs_linear.onnx - HTDemucs contract with a 30000-sample segment (30 frames):
  inputs mix [1, 2, 30000], spec [1, 4, 2048, 30]; outputs freq = a_s * spec
  [1, 4, 4, 2048, 30] and time = b_s * mix [1, 4, 2, 30000], so stem s = (a_s + b_s) * mix.
"""

import sys
from pathlib import Path

import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper

OPSET = 17
MDX_DIM_F, MDX_DIM_T = 400, 64
DEMUCS_SEG, DEMUCS_FRAMES, BINS = 30000, 30, 2048
A = [0.5, 0.0, 0.25, 1.0]  # freq-branch gain per source
B = [0.5, 1.0, 0.0, 0.0]   # time-branch gain per source


def mdx_half():
    inp = helper.make_tensor_value_info("input", TensorProto.FLOAT, ["batch_size", 4, MDX_DIM_F, MDX_DIM_T])
    out = helper.make_tensor_value_info("output", TensorProto.FLOAT, ["batch_size", 4, MDX_DIM_F, MDX_DIM_T])
    half = numpy_helper.from_array(np.array(0.5, dtype=np.float32), "half")
    g = helper.make_graph([helper.make_node("Mul", ["input", "half"], ["output"])], "mdx_half", [inp], [out], [half])
    return helper.make_model(g, opset_imports=[helper.make_opsetid("", OPSET)])


def htdemucs_linear():
    s = len(A)
    mix = helper.make_tensor_value_info("mix", TensorProto.FLOAT, [1, 2, DEMUCS_SEG])
    spec = helper.make_tensor_value_info("spec", TensorProto.FLOAT, [1, 4, BINS, DEMUCS_FRAMES])
    freq = helper.make_tensor_value_info("freq", TensorProto.FLOAT, [1, s, 4, BINS, DEMUCS_FRAMES])
    time = helper.make_tensor_value_info("time", TensorProto.FLOAT, [1, s, 2, DEMUCS_SEG])
    inits = [
        numpy_helper.from_array(np.array(A, dtype=np.float32).reshape(1, s, 1, 1, 1), "a"),
        numpy_helper.from_array(np.array(B, dtype=np.float32).reshape(1, s, 1, 1), "b"),
        numpy_helper.from_array(np.array([1], dtype=np.int64), "axis1"),
    ]
    nodes = [
        helper.make_node("Unsqueeze", ["spec", "axis1"], ["spec5"]),
        helper.make_node("Mul", ["spec5", "a"], ["freq"]),
        helper.make_node("Unsqueeze", ["mix", "axis1"], ["mix4"]),
        helper.make_node("Mul", ["mix4", "b"], ["time"]),
    ]
    # Outputs deliberately listed time-first: the separator must pick them by rank.
    g = helper.make_graph(nodes, "htdemucs_linear", [mix, spec], [time, freq], inits)
    return helper.make_model(g, opset_imports=[helper.make_opsetid("", OPSET)])


def main():
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent / "crates/stem-models/tests/fixtures"
    out.mkdir(parents=True, exist_ok=True)
    for name, model in [("mdx_half.onnx", mdx_half()), ("htdemucs_linear.onnx", htdemucs_linear())]:
        model.ir_version = 8  # loadable by onnxruntime >= 1.14
        onnx.checker.check_model(model)
        onnx.save(model, str(out / name))
        print("wrote", out / name)


if __name__ == "__main__":
    main()
