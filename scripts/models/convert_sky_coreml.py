#!/usr/bin/env -S uv run --script
# /// script
# requires-python = "==3.12.*"
# dependencies = [
#     "coremltools==9.0",
#     "torch==2.7.0",
#     "transformers==4.57.1",
#     "onnx==1.19.1",
#     "onnxruntime==1.23.2",
#     "numpy==2.2.6",
#     "pillow==11.3.0",
# ]
# ///
"""Converts Select Sky's network (`upernet-convnext-base-sky-fp16.onnx`) into the Core ML
model RAWmakase runs on Apple Silicon's GPU (`rawmakase_selection::core_ml`).

Only what holds no weights is written into the repository
(crates/rawmakase-selection/models/core-ml): the package's `Manifest.json`, the ML Program
`model.mlmodel`, and `tensors.txt`, which of the ONNX file's tensors each weight is.
RAWmakase makes the weights itself, on the computer that runs them, from the installed
ONNX file and `tensors.txt`, and checks them against the SHA-256 printed here
(`WEIGHTS_SHA256` in crates/rawmakase-selection/src/core_ml.rs). Every weight is one of
the ONNX file's tensors in half precision, as it is or transposed (a linear layer, which
the export wrote as a MatMul): the script checks that it rebuilds coremltools' weights
byte for byte.

The network is UperNet with a ConvNeXt-base backbone (openmmlab/upernet-convnext-base,
MIT), as `export_upernet_sky.py` exported it. The PyTorch model is rebuilt around the ONNX
file's weights: ONNX keeps PyTorch's parameter names, except for the linear layers and each
convolution with its BatchNorm folded in, which are found by their nodes' names. The
rebuilt model answers within 1e-7 of ONNX Runtime.

Core ML can't take this network at a size known only when it runs: UperNet's pyramid
pooling and its upsampling need the input's size. So the program has one function per
input size, all sharing one copy of the weights: the long edge at 1024 px, as Select Sky
feeds it, and the short one for 3:2, 4:3 and 16:9 photos, with one pixel either side
(a camera's sensor is a hair off its proportions: a 6000 x 4000 photo gives 682, an
8191 x 5463 one 683), both ways round, and square ones. A photo of any other shape is
selected on the CPU as before. Functions need Core ML's specification 9 (macOS 15).

The script then checks every function on this Mac's GPU against ONNX Runtime's CPU on
macOS's desktop pictures: no sky probability may differ by more than 0.05, nor by more
than 0.01 on average (about 13 and 2.5 of 255 levels), and the class scores Select Sky
reads around the sky must agree as closely after their softmax. The masks' overlap at 0.5
is reported, on pictures with at least 1% sky, but not required.

    scripts/models/convert_sky_coreml.py [upernet-convnext-base-sky-fp16.onnx]

The model defaults to the installed one. Converting takes a few minutes and about 8 GB of
memory.
"""

import gc
import hashlib
import json
import re
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
from onnx import numpy_helper
from PIL import Image
from torch import nn
from transformers import ConvNextConfig, UperNetConfig, UperNetForSemanticSegmentation

ROOT = Path(__file__).resolve().parent.parent.parent
DEFAULT_MODEL = (Path.home() / "Library/Application Support/RAWmakase/models"
                 / "upernet-convnext-base-v1/upernet-convnext-base-sky-fp16.onnx")
SOURCE_SHA256 = "ed75b2ad81ad209949c9580071ed486a72a423037822a49faa4e98991a43b891"
OUT = ROOT / "crates/rawmakase-selection/models/core-ml"
WEIGHTS = "Data/com.apple.CoreML/weights/weight.bin"
SKY = 2  # ADE20K's class
LOGITS = "/model/decode_head/classifier/Conv_output_0"  # SkySpec::logits in manifest.rs
LONG_EDGE = 1024
SHORT_EDGES = [683, 768, 576, 1024]  # 3:2, 4:3, 16:9, 1:1
MEAN = np.array([0.485, 0.456, 0.406], np.float32)
STD = np.array([0.229, 0.224, 0.225], np.float32)
MOST_DIFFERENCE = 0.05
MOST_MEAN_DIFFERENCE = 0.01


class Sky(nn.Module):
    """The ONNX file's two answers: the probability of ADE20K's sky at each pixel, and the
    decode head's class scores on its quarter-size grid (`LOGITS`), which Select Sky reads
    to see what surrounds the sky."""

    def __init__(self, model):
        super().__init__()
        self.model = model

    def forward(self, pixel_values):
        features = self.model.backbone.forward_with_filtered_kwargs(pixel_values).feature_maps
        logits = self.model.decode_head(features)
        upsampled = nn.functional.interpolate(
            logits, size=pixel_values.shape[2:], mode="bilinear", align_corners=False)
        return upsampled.softmax(1)[:, SKY], logits


def rebuild(graph: onnx.ModelProto) -> nn.Module:
    """The PyTorch model with the ONNX file's weights."""
    weights = {i.name.removesuffix("_fp16"): numpy_helper.to_array(i).astype(np.float32)
               for i in graph.graph.initializer}
    config = UperNetConfig(
        backbone_config=ConvNextConfig(depths=[3, 3, 27, 3], hidden_sizes=[128, 256, 512, 1024],
                                       out_features=["stage1", "stage2", "stage3", "stage4"]),
        num_labels=150, hidden_size=512, use_auxiliary_head=False)
    model = UperNetForSemanticSegmentation(config).eval()
    modules = dict(model.named_modules())
    state = {}
    for name in model.state_dict():
        for candidate in ("model." + name, name):
            if candidate in weights:
                state[name] = weights[candidate]

    def module(node):
        path = node.name.removeprefix("/model/").rsplit("/", 1)[0].replace("/", ".")
        # The export names pyramid pooling's blocks after their containers twice.
        return re.sub(r"psp_modules\.psp_modules\.(\d+)\.psp_modules\.\d+\.(\d+)",
                      r"psp_modules.\1.\2", path)

    for node in graph.graph.node:
        if len(node.input) < 2 or not node.input[1].startswith("onnx::"):
            continue
        if node.op_type == "MatMul":  # a linear layer, its weight transposed
            state[module(node) + ".weight"] = weights[node.input[1]].T
        elif node.op_type == "Conv":  # a convolution with its BatchNorm folded in
            conv = module(node)
            norm = conv.rsplit(".", 1)[0] + ".batch_norm"
            channels = weights[node.input[1]].shape[0]
            eps = modules[norm].eps
            state[conv + ".weight"] = weights[node.input[1]]
            state[norm + ".weight"] = np.ones(channels, np.float32)
            state[norm + ".bias"] = weights[node.input[2]]
            state[norm + ".running_mean"] = np.zeros(channels, np.float32)
            state[norm + ".running_var"] = np.full(channels, 1 - eps, np.float32)
    missing = [k for k in model.state_dict() if k not in state and not k.endswith("num_batches_tracked")]
    if missing:
        sys.exit(f"weights not found in the ONNX file: {missing[:5]}")
    model.load_state_dict({k: torch.from_numpy(np.ascontiguousarray(v)) for k, v in state.items()},
                          strict=False)
    return Sky(model).eval()


def shapes() -> list[tuple[int, int]]:
    """(width, height) of each function: landscape, portrait, then square."""
    found = []
    for short in SHORT_EDGES:
        for side in (short - 1, short, short + 1):
            if side > LONG_EDGE:
                continue
            for shape in ((LONG_EDGE, side), (side, LONG_EDGE)):
                if shape not in found:
                    found.append(shape)
    return found


def convert(graph: onnx.ModelProto, package: Path) -> None:
    model = rebuild(graph)
    descriptor = ct.utils.MultiFunctionDescriptor()
    with tempfile.TemporaryDirectory() as tmp:
        for width, height in shapes():
            with torch.no_grad():
                traced = torch.jit.trace(model, torch.zeros(1, 3, height, width))
            converted = ct.convert(
                traced,
                inputs=[ct.TensorType(name="pixel_values", shape=(1, 3, height, width))],
                outputs=[ct.TensorType(name="sky"), ct.TensorType(name="logits")],
                compute_precision=ct.precision.FLOAT16,
                minimum_deployment_target=ct.target.macOS15,
                convert_to="mlprogram",
            )
            part = Path(tmp) / f"{width}x{height}.mlpackage"
            converted.save(str(part))
            descriptor.add_function(str(part), src_function_name="main",
                                    target_function_name=f"sky_{width}x{height}")
            print(f"converted {width}x{height}", flush=True)
            del converted, traced
            gc.collect()
        descriptor.default_function_name = f"sky_{LONG_EDGE}x{SHORT_EDGES[0]}"
        joined = Path(tmp) / "joined.mlpackage"
        ct.utils.save_multifunction(descriptor, str(joined))
        merged = ct.models.MLModel(str(joined), skip_model_load=True)
        merged.author = "OpenMMLab (UperNet, ConvNeXt); converted by RAWmakase"
        merged.license = "MIT (openmmlab/upernet-convnext-base's card)"
        merged.short_description = (
            "Select Sky's UperNet ConvNeXt-base, half precision, one function per input size. "
            "Input: pixel_values, ImageNet-normalized RGB; outputs: sky, the probability of "
            "sky at each pixel, and logits, the 150 classes' scores on a quarter-size grid.")
        merged.user_defined_metadata.update({
            "source_sha256": SOURCE_SHA256,
            "long_edge": str(LONG_EDGE),
            "functions": ",".join(f"{w}x{h}" for w, h in shapes()),
        })
        merged.save(str(package))


def pictures() -> list[np.ndarray]:
    """macOS's desktop pictures as RGB in 0..1, for the check."""
    folder = Path("/System/Library/Desktop Pictures/.thumbnails")
    found = []
    with tempfile.TemporaryDirectory() as tmp:
        for picture in sorted(folder.glob("*.heic"))[:12]:
            png = Path(tmp) / (picture.stem + ".png")
            if subprocess.run(["sips", "-s", "format", "png", str(picture), "--out", str(png)],
                              capture_output=True).returncode == 0:
                found.append(np.asarray(Image.open(png).convert("RGB"), np.float32) / 255)
    if not found:
        sys.exit("no desktop pictures to check the model with")
    return found


def tensor(picture: np.ndarray, width: int, height: int) -> np.ndarray:
    """`picture` cropped to the shape's proportions, resized and normalized as Select Sky does."""
    h, w = picture.shape[:2]
    if w / h > width / height:
        crop = round(h * width / height)
        picture = picture[:, (w - crop) // 2:(w - crop) // 2 + crop]
    else:
        crop = round(w * height / width)
        picture = picture[(h - crop) // 2:(h - crop) // 2 + crop]
    image = Image.fromarray((picture * 255).astype(np.uint8)).resize((width, height), Image.BILINEAR)
    pixels = np.asarray(image, np.float32) / 255
    return ((pixels - MEAN) / STD).transpose(2, 0, 1)[None].astype(np.float32)


def check(source: Path, package: Path) -> dict:
    """Each function on this Mac's GPU against ONNX Runtime's CPU."""
    graph = onnx.load(str(source))
    graph.graph.output.append(onnx.helper.make_tensor_value_info(LOGITS, onnx.TensorProto.FLOAT, None))
    reference = ort.InferenceSession(graph.SerializeToString(), providers=["CPUExecutionProvider"])
    samples = pictures()
    worst, farthest, mean, score_gap = 1.0, 0.0, 0.0, 0.0

    def soft(v):
        e = np.exp(v - v.max(axis=1, keepdims=True))
        return e / e.sum(axis=1, keepdims=True)

    for width, height in shapes():
        gpu = ct.models.MLModel(str(package), compute_units=ct.ComputeUnit.CPU_AND_GPU,
                                function_name=f"sky_{width}x{height}")
        for picture in samples:
            x = tensor(picture, width, height)
            probability, scores = reference.run(["sky", LOGITS], {"pixel_values": x})
            answer = gpu.predict({"pixel_values": x})
            got, logits = answer["sky"], answer["logits"]
            if not (np.isfinite(got).all() and np.isfinite(logits).all()):
                sys.exit(f"{width}x{height}: the GPU answered with values that are not finite")
            if got.shape != probability.shape or logits.shape != scores.shape:
                sys.exit(f"{width}x{height}: answers of shape {got.shape} and {logits.shape}")
            score_gap = max(score_gap, float(np.abs(soft(logits) - soft(scores)).max()))
            difference = np.abs(got - probability)
            farthest = max(farthest, float(difference.max()))
            mean = max(mean, float(difference.mean()))
            want, have = probability > 0.5, got > 0.5
            # Select Sky finds no sky below 1% of the frame, where a few pixels crossing 0.5
            # would swing the overlap: it is measured above that.
            if (want | have).mean() >= 0.01:
                worst = min(worst, float((want & have).sum() / (want | have).sum()))
        print(f"checked {width}x{height}: largest difference {farthest:.3f}, mean {mean:.4f}",
              flush=True)
        # Core ML crashes loading many functions into one process while the others are
        # still alive: each is let go before the next.
        del gpu
        gc.collect()
    if score_gap > MOST_DIFFERENCE:
        sys.exit(f"the GPU's class scores are too far from the CPU's: shares within {score_gap:.3f}")
    if farthest > MOST_DIFFERENCE or mean > MOST_MEAN_DIFFERENCE:
        sys.exit(f"the GPU's answers are too far from the CPU's: largest difference "
                 f"{farthest:.3f}, mean {mean:.4f}")
    return {"largest_difference": round(farthest, 4), "mean_difference": round(mean, 5),
            "class_shares_within": round(score_gap, 4), "worst_iou": round(worst, 4),
            "pictures": len(samples)}


def tensors_of(graph: onnx.ModelProto, weights: bytes) -> list[str]:
    """Each weight in `weights` (Core ML's blob file) as a line of `tensors.txt`: the name of
    the ONNX tensor it is, and `T` when it is transposed or `-` when it is as it is."""
    found = {}
    for init in graph.graph.initializer:
        half = numpy_helper.to_array(init).astype(np.float16)
        found.setdefault(hashlib.sha256(half.tobytes()).digest(), f"{init.name} -")
        if half.ndim == 2:
            found.setdefault(hashlib.sha256(np.ascontiguousarray(half.T).tobytes()).digest(),
                             f"{init.name} T")
    count, version = struct.unpack_from("<II", weights, 0)
    if version != 2:
        sys.exit(f"coremltools wrote a weights file of version {version}")
    lines, at = [], 64
    for i in range(count):
        sentinel, kind, size, offset = struct.unpack_from("<IIQQ", weights, at)
        if sentinel != 0xDEADBEEF or kind != 1:
            sys.exit(f"weight {i} is not a half-precision blob")
        line = found.get(hashlib.sha256(weights[offset:offset + size]).digest())
        if line is None:
            sys.exit(f"weight {i} ({size} bytes) is none of the ONNX file's tensors")
        lines.append(line)
        at = (offset + size + 63) // 64 * 64
    return lines


def weights_from(graph: onnx.ModelProto, lines: list[str]) -> bytes:
    """`weight.bin` as RAWmakase makes it (`core_ml::weights`): each tensor of `lines` from
    the ONNX file in half precision, transposed where marked, in Core ML's blob file."""
    init = {i.name: numpy_helper.to_array(i).astype(np.float16) for i in graph.graph.initializer}
    blobs = []
    for line in lines:
        name, how = line.split()
        w = init[name]
        blobs.append(np.ascontiguousarray(w.T if how == "T" else w).tobytes())
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
        sys.exit(f"{source} is not RAWmakase's Sky model (SHA-256 {digest})")
    graph = onnx.load(str(source))
    with tempfile.TemporaryDirectory() as work:
        package = Path(work) / "model.mlpackage"
        convert(graph, package)
        results = check(source, package)
        weights = (package / WEIGHTS).read_bytes()
        lines = tensors_of(graph, weights)
        if weights_from(graph, lines) != weights:
            sys.exit("tensors.txt does not make the weights coremltools wrote")
        OUT.mkdir(parents=True, exist_ok=True)
        (OUT / "Manifest.json").write_bytes((package / "Manifest.json").read_bytes())
        (OUT / "model.mlmodel").write_bytes(
            (package / "Data/com.apple.CoreML/model.mlmodel").read_bytes())
        (OUT / "tensors.txt").write_text("\n".join(lines) + "\n")
    print(json.dumps({
        "out": str(OUT.relative_to(ROOT)), **results,
        "weights": [len(weights), hashlib.sha256(weights).hexdigest()],
    }, indent=1))


if __name__ == "__main__":
    main()
