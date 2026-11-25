#!/usr/bin/env -S uv run --script
# /// script
# requires-python = "==3.12.*"
# dependencies = [
#     "coremltools==9.0",
#     "torch==2.7.0",
#     "onnx==1.19.1",
#     "onnx2torch==1.5.15",
#     "onnxruntime==1.23.2",
#     "numpy==2.2.6",
# ]
# ///
"""Converts AI Denoise's network (RawNIND UtNet2, `model_bayer.onnx`) into the
half-precision Core ML model RAWmakase runs on Apple's Neural Engine
(`rawmakase_inference::neural_engine`).

Only what holds no weights is written into the repository
(crates/rawmakase-inference/models/neural-engine): the package's `Manifest.json`, the ML
Program `model.mlmodel`, and `scales.txt`, the power of two each weight and bias is
multiplied by. RAWmakase makes the weights itself, on the computer that runs them, from
the installed `model_bayer.onnx` and `scales.txt`, and checks them against the SHA-256
printed here (`WEIGHTS_SHA256` in crates/rawmakase-inference/src/neural_engine.rs).

The Neural Engine computes in half precision only, whose largest value is 65504; the
network's activations reach about 1.3 million, and its first layers' are tiny. Every
operation in it is positively homogeneous (convolutions, LeakyReLU, max pooling,
concatenation, depth-to-space), so each convolution's output can be multiplied by its
own power of two: its weights by c_out / c_in per input channel, its bias by c_out. The
power is chosen so the largest value seen in calibration lands at 2^12, sixteen times
below the limit. A concatenation keeps each branch's own scale, and the convolution
after it rescales its weights per input channel: a scale shared across a skip connection
would push far more of some layers' weights into half precision's subnormals, which the
Neural Engine flushes to zero. (Some small weights stay subnormal; the check below on the
Neural Engine is what holds the result.)

The model takes the input multiplied by `input_scale` and answers the output multiplied
by `output_scale`, both stored in its metadata with the source model's SHA-256.

Calibration uses only the repository's own data: the colour-chart DNGs in
tests/corpus/charts (packed by rawmakase-decode's `packed_planes` example) and synthetic
tiles (white, noise, a checkerboard, single channels). The script then checks, on this
Mac, that no tile overflows on the Neural Engine and that it agrees with ONNX Runtime's
CPU.

    scripts/models/convert_denoise_ane.py [model_bayer.onnx]

The model defaults to the installed one.
"""

import hashlib
import json
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

import coremltools as ct
import numpy as np
import onnx
import onnxruntime as ort
import torch
from onnx import TensorProto, helper, numpy_helper
from onnx2torch import convert

ROOT = Path(__file__).resolve().parent.parent.parent
DEFAULT_MODEL = (Path.home() / "Library/Application Support/RAWmakase/models"
                 / "rawnind-utnet2-bayer-v1/model_bayer.onnx")
SOURCE_SHA256 = "5111f19a66a9377002e2ff4ca87de8e3303085b33e6dc11746e42ce811bf6cc1"
CHARTS = ROOT / "tests/corpus/charts"
OUT = ROOT / "crates/rawmakase-inference/models/neural-engine"
WEIGHTS = "Data/com.apple.CoreML/weights/weight.bin"
TILE = 512
TARGET_LOG2 = 12
INPUT_SCALE = 2.0 ** 8  # keeps the darkest inputs out of half precision's subnormals
WORST_PSNR_DB = 65.0


def chart_tiles() -> list[np.ndarray]:
    """Each Bayer chart's packed planes, mirrored out to one tile."""
    tiles = []
    with tempfile.TemporaryDirectory() as tmp:
        for dng in sorted(CHARTS.glob("*.dng")):
            out = Path(tmp) / (dng.stem + ".f32")
            run = subprocess.run(
                ["cargo", "run", "-q", "--release", "-p", "rawmakase-decode",
                 "--example", "packed_planes", "--", str(dng), str(out)],
                cwd=ROOT, capture_output=True, text=True)
            if run.returncode != 0:
                continue  # not a Bayer mosaic (X-Trans)
            w, h = map(int, run.stdout.split())
            planes = np.fromfile(out, dtype="<f4").reshape(4, h, w)[:, :TILE, :TILE]
            pad = ((0, 0), (0, TILE - planes.shape[1]), (0, TILE - planes.shape[2]))
            tiles.append(np.pad(planes, pad, mode="reflect")[None])
    if not tiles:
        sys.exit("no chart could be packed")
    return tiles


def synthetic_tiles(seed: int) -> dict[str, np.ndarray]:
    rng = np.random.default_rng(seed)
    one, zero = np.ones((TILE, TILE)), np.zeros((TILE, TILE))
    yx = np.indices((TILE, TILE))
    return {
        "white": np.ones((1, 4, TILE, TILE)),
        "noise": rng.random((1, 4, TILE, TILE)),
        "dark noise": rng.random((1, 4, TILE, TILE)) ** 4,
        "checker": (yx.sum(0) % 2)[None, None].repeat(4, 1),
        "stripes": (yx[1] % 2)[None, None].repeat(4, 1),
        "blocks": ((yx // 8).sum(0) % 2)[None, None].repeat(4, 1),
        "green": np.stack([zero, one, one, zero])[None],
        "red": np.stack([one, zero, zero, zero])[None],
        "blue": np.stack([zero, zero, zero, one])[None],
    }


def peaks(model: onnx.ModelProto, tiles: list[np.ndarray]) -> dict[str, float]:
    """The largest magnitude of each convolution's output over `tiles`, in float32."""
    probe = onnx.ModelProto()
    probe.CopyFrom(model)
    convs = [n.output[0] for n in model.graph.node if n.op_type in ("Conv", "ConvTranspose")]
    for name in convs:
        probe.graph.output.append(helper.make_tensor_value_info(name, TensorProto.FLOAT, None))
    session = ort.InferenceSession(probe.SerializeToString(), providers=["CPUExecutionProvider"])
    names = [o.name for o in session.get_outputs()]
    found = dict.fromkeys(names, 0.0)
    for tile in tiles:
        for name, out in zip(names, session.run(None, {"input": tile.astype(np.float32)})):
            found[name] = max(found[name], float(np.abs(out).max()))
    return found


def rescale(model: onnx.ModelProto, peak: dict[str, float]) -> tuple[float, list[str]]:
    """Rescales `model` in place, layer by layer; returns the output's scale and the
    lines of `scales.txt`: each weight's and bias's name, the axis of its input channels
    (a bias: `-`) and the power of two each input channel was multiplied by, run-length
    encoded (`-3*32`), in the order of the graph's convolutions."""
    init = {i.name: i for i in model.graph.initializer}
    scale = {"input": np.full(4, INPUT_SCALE)}
    lines = []
    for node in model.graph.node:
        if node.op_type in ("Conv", "ConvTranspose"):
            c_in = scale[node.input[0]]
            c_out = 2.0 ** round(TARGET_LOG2 - np.log2(max(peak[node.output[0]], 1e-30)))
            exps = np.log2(c_out / c_in)
            assert (exps == np.round(exps)).all()
            lines.append(f"{node.input[1]} {1 if node.op_type == 'Conv' else 0} {runs(exps)}")
            if len(node.input) > 2:
                lines.append(f"{node.input[2]} - {int(np.log2(c_out))}")
            w = numpy_helper.to_array(init[node.input[1]]).astype(np.float64)
            if node.op_type == "Conv":  # [out, in, kh, kw]
                w = w * c_out / c_in[None, :, None, None]
                channels = w.shape[0]
            else:  # [in, out, kh, kw]
                w = w * c_out / c_in[:, None, None, None]
                channels = w.shape[1]
            init[node.input[1]].CopyFrom(numpy_helper.from_array(w.astype(np.float32), node.input[1]))
            if len(node.input) > 2:
                b = numpy_helper.to_array(init[node.input[2]]).astype(np.float64) * c_out
                init[node.input[2]].CopyFrom(numpy_helper.from_array(b.astype(np.float32), node.input[2]))
            scale[node.output[0]] = np.full(channels, c_out)
        elif node.op_type == "Concat":
            scale[node.output[0]] = np.concatenate([scale[i] for i in node.input])
        elif node.op_type == "DepthToSpace":
            s = scale[node.input[0]]
            assert (s == s[0]).all(), "depth-to-space over mixed scales"
            scale[node.output[0]] = np.full(len(s) // 4, s[0])
        elif node.op_type in ("LeakyRelu", "MaxPool"):
            scale[node.output[0]] = scale[node.input[0]]
        else:
            sys.exit(f"{node.op_type} is not positively homogeneous: cannot rescale")
    out = scale[model.graph.output[0].name]
    assert (out == out[0]).all()
    return float(out[0]), lines


def runs(exps: np.ndarray) -> str:
    """`exps` as runs: `2*4 -1*28`."""
    out, i = [], 0
    while i < len(exps):
        j = i
        while j < len(exps) and exps[j] == exps[i]:
            j += 1
        out.append(f"{int(exps[i])}*{j - i}")
        i = j
    return " ".join(out)


def weights_from(source: onnx.ModelProto, lines: list[str]) -> bytes:
    """`weight.bin` as RAWmakase makes it (`neural_engine::weights`): each tensor of
    `lines` from `source`, multiplied by its powers of two, rounded to half precision, in
    Core ML's blob file. Checked here against what coremltools wrote."""
    init = {i.name: numpy_helper.to_array(i).astype(np.float32) for i in source.graph.initializer}
    blobs = []
    for line in lines:
        name, axis, *spec = line.split()
        w = init[name]
        if axis == "-":
            f = np.float32(2.0 ** int(spec[0]))
            blobs.append((w * f).astype(np.float16).tobytes())
            continue
        exps = [int(e) for run in spec for e, n in [run.split("*")] for _ in range(int(n))]
        shape = [1] * w.ndim
        shape[int(axis)] = -1
        f = (2.0 ** np.array(exps, dtype=np.float32)).reshape(shape)
        blobs.append((w * f).astype(np.float16).tobytes())
    out = bytearray(struct.pack("<II", len(blobs), 2).ljust(64, b"\0"))
    for i, blob in enumerate(blobs):
        meta = len(out)
        out += struct.pack("<IIQQ", 0xDEADBEEF, 1, len(blob), meta + 64).ljust(64, b"\0")
        out += blob
        if i + 1 < len(blobs):
            out += b"\0" * (-len(out) % 64)
    return bytes(out)


def main() -> None:
    source = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_MODEL

    digest = hashlib.sha256(source.read_bytes()).hexdigest()
    if digest != SOURCE_SHA256:
        sys.exit(f"{source} is not RAWmakase's model_bayer.onnx (SHA-256 {digest})")
    original = onnx.load(source)
    calibration = chart_tiles() + list(synthetic_tiles(seed=1).values())
    model = onnx.load(source)
    output_scale, lines = rescale(model, peaks(original, calibration))

    torch_model = convert(model).eval()
    example = torch.from_numpy(calibration[0].astype(np.float32) * INPUT_SCALE)
    with torch.no_grad():
        traced = torch.jit.trace(torch_model, example)
    mlmodel = ct.convert(
        traced,
        inputs=[ct.TensorType(name="input", shape=(1, 4, TILE, TILE))],
        outputs=[ct.TensorType(name="output")],
        compute_precision=ct.precision.FLOAT16,
        minimum_deployment_target=ct.target.iOS17,
        convert_to="mlprogram",
        compute_units=ct.ComputeUnit.CPU_AND_NE,
    )
    mlmodel.author = "Benoit Brummer (UCLouvain); converted by RAWmakase"
    mlmodel.license = "CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/)"
    mlmodel.short_description = (
        "RawNIND UtNet2 (Bayer), rescaled layer by layer for half precision. Input: packed "
        "RGGB planes times input_scale; output: camera RGB times output_scale.")
    mlmodel.user_defined_metadata.update({
        "source_sha256": SOURCE_SHA256,
        "input_scale": repr(INPUT_SCALE),
        "output_scale": repr(output_scale),
        "target_log2": str(TARGET_LOG2),
    })
    work = tempfile.TemporaryDirectory()
    package = Path(work.name) / "model.mlpackage"
    mlmodel.save(str(package))

    # The check: on this Mac's Neural Engine, nothing overflows and the answer is the CPU's.
    reference = ort.InferenceSession(str(source), providers=["CPUExecutionProvider"])
    engine = ct.models.MLModel(str(package), compute_units=ct.ComputeUnit.CPU_AND_NE)
    checks = {f"chart {i}": t for i, t in enumerate(chart_tiles()[:6])}
    checks.update(synthetic_tiles(seed=7))
    worst = float("inf")
    for name, tile in checks.items():
        tile = tile.astype(np.float32)
        want = reference.run(None, {"input": tile})[0]
        got = engine.predict({"input": tile * INPUT_SCALE})["output"] / output_scale
        if not np.isfinite(got).all():
            sys.exit(f"{name}: the Neural Engine overflowed")
        peak = np.abs(want).max()
        if peak > 0:
            psnr = 10 * np.log10(peak ** 2 / max(float(((want - got) ** 2).mean()), 1e-30))
            worst = min(worst, float(psnr))
    if worst < WORST_PSNR_DB:
        sys.exit(f"the Neural Engine's answer is {worst:.1f} dB from the CPU's")
    weights = (package / WEIGHTS).read_bytes()
    if weights_from(original, lines) != weights:
        sys.exit("scales.txt does not make the weights coremltools wrote")
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "Manifest.json").write_bytes((package / "Manifest.json").read_bytes())
    (OUT / "model.mlmodel").write_bytes(
        (package / "Data/com.apple.CoreML/model.mlmodel").read_bytes())
    (OUT / "scales.txt").write_text("\n".join(lines) + "\n")
    print(json.dumps({
        "out": str(OUT.relative_to(ROOT)), "output_scale": output_scale,
        "worst_psnr_db": round(float(worst), 1),
        "weights": [len(weights), hashlib.sha256(weights).hexdigest()],
    }, indent=1))


if __name__ == "__main__":
    main()
