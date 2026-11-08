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

Spectrogram-in / spectrogram-out graphs with the exact I/O names and shapes of the real exports
(roformer.rs), so the registry's own parameters are exercised:

* mel_roformer_identity.onnx - audiojs/mel-roformer: mix_spec [1, 4, 1025, 801] -> stems_spec
  (= mix_spec, a mask of 1), so vocals = mix and instrumental = 0.
* bs_roformer_gains.onnx - silverdaw/bs-roformer-rhythm-onnx: spec_real, spec_imag
  [1, 2, 1025, 801] -> out_spec_real, out_spec_imag [1, 4, 2, 1025, 801] = g_s * input.
* scnet_gains.onnx - audiojs/scnet: mix_spec [1, 4, 2049, 476] -> stems_spec [1, 16, 2049, 476]
  = g_s * input (source-major).
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


SPEC_GAINS = [1.0, 0.5, 0.25, 0.0]  # per source, drums / bass / other / vocals


def mel_roformer_identity():
    shape = [1, 4, 1025, 801]
    inp = helper.make_tensor_value_info("mix_spec", TensorProto.FLOAT, shape)
    out = helper.make_tensor_value_info("stems_spec", TensorProto.FLOAT, shape)
    g = helper.make_graph([helper.make_node("Identity", ["mix_spec"], ["stems_spec"])], "mel_identity", [inp], [out])
    return helper.make_model(g, opset_imports=[helper.make_opsetid("", OPSET)])


def bs_roformer_gains():
    s = len(SPEC_GAINS)
    ins = [helper.make_tensor_value_info(n, TensorProto.FLOAT, [1, 2, 1025, 801]) for n in ("spec_real", "spec_imag")]
    outs = [helper.make_tensor_value_info(n, TensorProto.FLOAT, [1, s, 2, 1025, 801]) for n in ("out_spec_real", "out_spec_imag")]
    inits = [
        numpy_helper.from_array(np.array(SPEC_GAINS, dtype=np.float32).reshape(1, s, 1, 1, 1), "g"),
        numpy_helper.from_array(np.array([1], dtype=np.int64), "axis1"),
    ]
    nodes = []
    for i, o in (("spec_real", "out_spec_real"), ("spec_imag", "out_spec_imag")):
        nodes += [helper.make_node("Unsqueeze", [i, "axis1"], [i + "5"]), helper.make_node("Mul", [i + "5", "g"], [o])]
    g = helper.make_graph(nodes, "bs_gains", ins, outs, inits)
    return helper.make_model(g, opset_imports=[helper.make_opsetid("", OPSET)])


def scnet_gains():
    s = len(SPEC_GAINS)
    inp = helper.make_tensor_value_info("mix_spec", TensorProto.FLOAT, [1, 4, 2049, 476])
    out = helper.make_tensor_value_info("stems_spec", TensorProto.FLOAT, [1, 4 * s, 2049, 476])
    inits = [
        numpy_helper.from_array(np.array(SPEC_GAINS, dtype=np.float32).reshape(1, s, 1, 1, 1), "g"),
        numpy_helper.from_array(np.array([1], dtype=np.int64), "axis1"),
        numpy_helper.from_array(np.array([1, 4 * s, 2049, 476], dtype=np.int64), "shape"),
    ]
    nodes = [
        helper.make_node("Unsqueeze", ["mix_spec", "axis1"], ["x5"]),
        helper.make_node("Mul", ["x5", "g"], ["y5"]),
        helper.make_node("Reshape", ["y5", "shape"], ["stems_spec"]),
    ]
    g = helper.make_graph(nodes, "scnet_gains", [inp], [out], inits)
    return helper.make_model(g, opset_imports=[helper.make_opsetid("", OPSET)])


def main():
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent / "crates/stem-models/tests/fixtures"
    out.mkdir(parents=True, exist_ok=True)
    for name, model in [
        ("mdx_half.onnx", mdx_half()),
        ("htdemucs_linear.onnx", htdemucs_linear()),
        ("mel_roformer_identity.onnx", mel_roformer_identity()),
        ("bs_roformer_gains.onnx", bs_roformer_gains()),
        ("scnet_gains.onnx", scnet_gains()),
    ]:
        model.ir_version = 8  # loadable by onnxruntime >= 1.14
        onnx.checker.check_model(model)
        onnx.save(model, str(out / name))
        print("wrote", out / name)


if __name__ == "__main__":
    main()
