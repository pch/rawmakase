#!/usr/bin/env python3
"""Writes the ONNX file behind Select Sky from openmmlab/upernet-convnext-base.

    python3 export_upernet_sky.py OUT.onnx    (torch, transformers and onnx installed)

The graph takes `pixel_values` (1, 3, height, width), normalized with ImageNet's mean and
std, and answers `sky` (1, height, width), the probability of ADE20K's sky at each pixel.
Its weights are half precision, cast back to float32 as it loads, so every operator still
computes in float32. Its classifier's output, `LOGITS` (ADE20K's class scores at a quarter
of the input), is an internal tensor that RAWmakase makes a second output as it loads the file.

The checkpoint's pyramid pooling uses adaptive pooling, which ONNX cannot express at a size
known only at run time. It is replaced by the same averages over the same bins, written with
cumulative sums, so the graph takes any size. The auxiliary head, used only in training, is
dropped.
"""
import sys

import numpy as np
import onnx
import torch
from onnx import TensorProto, helper, numpy_helper
from torch import nn
from torch.onnx.operators import shape_as_tensor
from transformers import UperNetForSemanticSegmentation

REPOSITORY = "openmmlab/upernet-convnext-base"
REVISION = "13a9c410ff779f25236a8d24802d2742e4694e9c"
SKY = 2  # ADE20K's class
# The classifier's output. rawmakase-inference (`SkySpec::logits` in manifest.rs) exposes this
# internal tensor as a second output when it loads the file, so a re-export must produce a node
# with this output name, or `SKY` there must be updated with the new name and digest together.
LOGITS = "/model/decode_head/classifier/Conv_output_0"


class Pool(nn.Module):
    """AdaptiveAvgPool2d(k): bin i runs from floor(i*n/k) to ceil((i+1)*n/k)."""

    def __init__(self, k):
        super().__init__()
        self.k = k

    def axis(self, x, dim):
        n = shape_as_tensor(x)[dim]
        i = torch.arange(self.k, dtype=torch.int64)
        start = torch.div(i * n, self.k, rounding_mode="floor")
        end = -torch.div(-(i + 1) * n, self.k, rounding_mode="floor")
        pad = [0, 0] * (x.dim() - dim - 1) + [1, 0]
        total = nn.functional.pad(x.cumsum(dim), pad)
        hi = total.index_select(dim, end)
        lo = total.index_select(dim, start)
        shape = [1] * x.dim()
        shape[dim] = self.k
        return (hi - lo) / (end - start).to(x.dtype).reshape(shape)

    def forward(self, x):
        return self.axis(self.axis(x, 2), 3)


class Sky(nn.Module):
    def __init__(self, model):
        super().__init__()
        self.model = model

    def forward(self, pixel_values):
        logits = self.model(pixel_values=pixel_values, return_dict=True).logits
        return logits.softmax(1)[:, SKY]


def load():
    model = UperNetForSemanticSegmentation.from_pretrained(
        REPOSITORY, revision=REVISION, use_safetensors=True
    ).eval()
    model.auxiliary_head = None
    for block in model.decode_head.psp_modules.blocks:
        block.layers[0] = Pool(block.layers[0].output_size)
        block.add_module("0", block.layers[0])
    return Sky(model).eval()


def half_precision_weights(path):
    """Turns large float32 weights into float16 ones cast back as the graph loads."""
    model = onnx.load(path)
    graph = model.graph
    weights, casts = [], []
    for weight in graph.initializer:
        if weight.data_type == TensorProto.FLOAT and np.prod(weight.dims or [1]) > 16:
            half = weight.name + "_fp16"
            weights.append(numpy_helper.from_array(numpy_helper.to_array(weight).astype(np.float16), half))
            casts.append(helper.make_node("Cast", [half], [weight.name], to=TensorProto.FLOAT, name="cast_" + weight.name))
        else:
            weights.append(weight)
    nodes = casts + list(graph.node)
    del graph.initializer[:]
    graph.initializer.extend(weights)
    del graph.node[:]
    graph.node.extend(nodes)
    onnx.save(model, path)


def main():
    out = sys.argv[1]
    torch.onnx.export(
        load(),
        (torch.zeros(1, 3, 683, 1024),),
        out,
        dynamo=False,
        opset_version=18,
        input_names=["pixel_values"],
        output_names=["sky"],
        dynamic_axes={"pixel_values": {2: "height", 3: "width"}, "sky": {1: "height", 2: "width"}},
        do_constant_folding=True,
    )
    half_precision_weights(out)
    graph = onnx.load(out).graph
    if not any(LOGITS in node.output for node in graph.node):
        sys.exit(f"no node produces {LOGITS}; update SkySpec::logits in manifest.rs together with the digest")


if __name__ == "__main__":
    main()
