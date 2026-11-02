# Remote inference

Optional, off by default. Selections (Select Subject / Sky / Background) normally
run on this computer: the models are downloaded into its data folder and run
through the ONNX Runtime library it ships with. This page describes the
alternative: running the same models on another computer that has the capable
hardware, with RAWmakase asking it over the network. Everything else about the
feature — the model contract, what a selection becomes, how results are saved
and rendered — is unchanged and described in
[subject-and-background-masks](subject-and-background-masks.md).

## When to use this

- This computer has no capable CPU headroom or GPU, and another one does
  (an NVIDIA card through CUDA, or Apple Silicon through CoreML).
- The models should not be downloaded to this computer at all.

When in doubt, use local inference: it is the default, it works offline, and no
photo leaves the computer.

## Security model, read before enabling

- The worker binds the **loopback** by default and has **no authentication**:
  anyone who can connect to its port can ask it to run the models on pixels
  they send, and can also fetch the masks of any photo whose analysis is still
  cached — `segment` is addressed by a deterministic content key over the
  pixels and the pinned model contract, so it identifies the photo, it is not
  a secret or a credential. That is deliberate — the worker is meant to be
  reached through an SSH tunnel, not published, and the binary refuses to bind
  anything but the loopback. Treat everyone who can reach the port as trusted.
- Keep it that way. To use a worker on another computer:

  ```sh
  # on the computer RAWmakase runs on; leave this running while you edit
  ssh -N -L 8765:127.0.0.1:8765 user@photo-server
  ```

  and point RAWmakase at `127.0.0.1:8765`. The photo's pixels travel through
  the tunnel (SSH encrypts them); nothing is sent anywhere else. Pointing
  RAWmakase straight at another host over an untrusted network is not
  supported: the protocol is plaintext TCP with no credentials.

## The worker

On the computer with the models (the same `models/` folder layout RAWmakase
installs; download it there once with any RAWmakase, or copy it):

```sh
rawmakase-remote-worker --model ~/.local/share/rawmakase/models/sam2.1-hiera-small-v1 \
    --sky-model ~/.local/share/rawmakase/models/upernet-convnext-base-v1
```

The worker loads the models once, prints what it loaded — the model, the
panoptic network, the sky model and the provider the runtime **actually registered** — and
serves until stopped. Its options:

| Option | Meaning | Default |
| --- | --- | --- |
| `--model DIR` | the installed models' folder (required) | — |
| `--sky-model DIR` | the installed sky model's folder; without it, Select Sky answers with an error saying so | none |
| `--listen ADDR` | loopback address to serve on | `127.0.0.1:8765` |
| `--provider WHICH` | `cpu` (default), `cuda`, `coreml` | `cpu` |
| `--threads N` | intra-op threads | CPU count, capped at 8 |
| `--runtime-lib FILE` | ONNX Runtime library to load | `RAWMAKASE_ORT_LIB`, then the packaged locations |

A requested accelerator is a requirement, not a preference: if the runtime
cannot register CUDA or CoreML — the library was built without it, the device
is missing, the CUDA libraries are the wrong version — the worker stops with
the runtime's own error instead of quietly running on the CPU. The provider it
prints at startup is what it loaded with, and what `ping` reports. Note that a
GPU provider may still keep some nodes on the CPU; the claim is "the runtime
accepted and registered this provider", nothing stronger.

Provider notes:

- **CUDA** needs an ONNX Runtime build with the CUDA execution provider
  (the official Linux/Windows packages ship it) and a working CUDA/cuDNN
  installation; on Linux the provider libraries sit beside
  `libonnxruntime.so`.
- **CoreML** is in every official macOS package; it applies to Apple
  Silicon's GPU or Neural Engine as ONNX Runtime decides.

Stopping the worker mid-selection is safe: the app reports the connection was
lost, the photo is left as it was, and the next selection starts again.

## The app

Create `remote-inference.json` in RAWmakase's data folder
(`~/.local/share/rawmakase/` on Linux, `~/Library/Application Support/RAWmakase/`
on macOS, `%APPDATA%\RAWmakase\` on Windows; `RAWMAKASE_DATA_DIR` moves it):

```json
{ "endpoint": "127.0.0.1:8765" }
```

That is the whole configuration. With the file present, the Masking drawer
stops asking for the model download or a local ONNX Runtime, shows the worker
it is using while a selection runs, and reports the worker's errors in the
drawer like any other selection failure: unreachable, timed out (a request may
take up to three minutes), cancelled, or the worker forgot the photo (it keeps
only the last two photos' analyses; the app asks for it again once
automatically). Delete the file — or empty it — to go back to local inference;
saved masks never needed the model in the first place and keep rendering.

## What crosses the wire

Pixels and masks only. One request, one reply, length-prefixed JSON frames:
the photo as 8-bit sRGB (the same 1280-pixel-edge render local inference is
given), the answer as 8-bit coverage, and prompts as fractions. Every request
carries an id; a cancel is a tiny separate frame the client sends when the
drawer's Cancel is used, and the worker stops the model run it can; a cancel
that overtakes its request is remembered, so that request starts cancelled. The worker
runs at most one model at a time (a second request waits), refuses oversized
or malformed frames before they reach the models, and bounds every read and
write with timeouts. The definitions live in `crates/rawmakase-remote`; the
protocol version is checked on both ends, and each analyze and sky request
names the processing version of the model it expects, so an old worker and a
new app refuse each other instead of the app storing an old pipeline's mask
under the new version.

## Costs and limits

- The photo's model input (a few MB) travels per photo, not per click; the
  coverage (about a megabyte) travels per result. No latency numbers are
  promised here: measure the link you will use. Over a slow link the first
  selection on a photo waits on the transfer, and the request budget
  (three minutes by default) bounds how slow a link can be and still work.
- The worker keeps the analyses of the last two photos (bounded well under a
  gigabyte) so refining clicks stay quick; everything else is recomputed. Sky keeps
  nothing there: the app remembers the last answer, and the photo travels again for
  another. With `--sky-model` the worker holds that model too (about 800 MB loaded, more
  while it runs).
- Local inference remains the default and the reference: without the
  configuration file nothing in this page is in play.
