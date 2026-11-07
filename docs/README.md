# Documentation

Each page has a status:

- **Current**: describes the code on `main`. A change that makes it wrong updates it
  in the same pull request.
- **Record (date)**: what was measured, checked or decided at that date. Kept for its
  evidence; not updated as the code moves on. Records live in [`records/`](records/).
- **Plan (date)**: work proposed at that date. Says how far it was built.

Start with the [code map](code-map.md) to find where a change belongs, the
[architecture guide](architecture.md) for the rules that change must keep, and the
[glossary](glossary.md) for the words the code and these pages use.
[CONTRIBUTING.md](../CONTRIBUTING.md) covers building, testing and sending a change.

## Working on the code

| Page | Status | What it covers |
| --- | --- | --- |
| [Architecture](architecture.md) | Current | Crates and dependency direction, threads and jobs, data flow, boundary rules, lifecycle and compatibility contracts |
| [Code map](code-map.md) | Current | File-by-file map, runtime flows, storage locations, tests |
| [Glossary](glossary.md) | Current | Recipe, raw defaults, stand-in, generation, receipt and the other terms the code uses |
| [Adding an adjustment](howto/add-adjustment.md) | Current | A new develop setting, from the model to tests |
| [Shutdown](shutdown.md) | Current | What each worker blocks on, how it stops, and what quitting waits for |
| [Dependencies](dependencies.md) | Current | Third-party code, models and data, their licences and how they are linked or downloaded |
| [Test corpus](../tests/corpus/README.md) | Current | The colour corpus, the private dataset and how to run parity checks |

## Features

| Page | Status | What it covers |
| --- | --- | --- |
| [Catalogs](catalogs.md) | Current | The Library, catalogs, Lightroom import, relinking, previews, file management |
| [XMP presets](xmp-presets.md) | Current | Presets, raw defaults, supported XMP settings and their limits |
| [Lightroom profiles](lightroom-profiles.md) | Current | RAWmakase's own profiles, imported and community profiles, `.cube` LUTs, Camera Look |
| [Cameras](cameras.md) | Current | `data/cameras.toml`: per-camera facts beyond the RAW file |
| [Masking](masking.md) | Current (experimental feature) | The Masking tool, including Select Subject, Sky and Background |
| [Retouching](retouching.md) | Current (experimental feature) | Remove (Heal and Clone) and Red Eye |
| [Transform](transform.md) | Current | Crop, Straighten, the Transform sliders and Upright |
| [Lens corrections](lens-corrections.md) | Current | Built-in, DNG, Adobe LCP and Lensfun corrections, CA and Defringe |
| [Automation](automation.md) | Current | The local control protocol, `rawmakase control` and scripts |
| [MCP](mcp.md) | Current | `rawmakase mcp`, the local MCP adapter |
| [MIDI](midi.md) | Current | MIDI controllers |
| [Remote inference](remote-inference.md) | Current | Running the selection models on another computer |
| [Subject and Background masks](subject-and-background-masks.md) | Plan (2026-10-08), built | The selection masks' design; its closing notes record where the build differs |
| [Editable previews](editable-previews.md) | Plan (2026-11-06), partly built | Editing an offline photo from a smaller copy built while it was online: decisions, owners and stages |

## Rendering

| Page | Status | What it covers |
| --- | --- | --- |
| [Rendering quality](rendering-quality.md) | Current | The engine, its stages, preview and detail rendering, AI Denoise |
| [Color pipeline](color-pipeline.md) | Current, with a pipeline-1 record | The rendering contract: development, white balance, gamut, display |
| [Scene tone stage](scene-tone-stage.md) | Current | Where the Basic tone controls render, the Camera Raw survey behind it and the photo gates a change must pass |
| [Tone controls](tone-controls.md) | Current | How each Basic tone control, the parametric curve and Auto were measured and render |
| [Color mixer](color-mixer.md) | Current | Color mixer, color grading, Point Color and B&W |
| [Demosaic](demosaic.md) | Current | RAWmakase's Bayer and X-Trans demosaicing |
| [RAW decoding](raw-decoding.md) | Current | RAWmakase's own RAW decoders, per format |
| [Preview performance](preview-performance.md) | Current | The resolution pyramid, stage caching, the GPU stages and the decode cache |
| [Camera parity](camera-parity.md) | Current | Checking camera white-balance calibration and tone against Adobe's converters |

## Records

| Page | Status | What it covers |
| --- | --- | --- |
| [Editable previews, Stage 5 brief](records/editable-previews-stage5-brief.md) | Plan (2026-11-07), not built | What editing an offline photo from its editable preview must change, with code anchors |
| [Parity gaps](records/parity-gaps.md) | Record (2026-10-05, last updated 2026-11-03) | Known differences from Lightroom, with their measurements |
| [Preview performance measurements](records/preview-performance-measurements.md) | Record (2026-09-26 to 2026-10-29) | Timings behind the preview path's choices |
| [Validation](records/validation.md) | Record (September 2026) | What was checked while engines 2 and 3 were current |
| [macOS and Lightroom validation](records/macos-lightroom-validation.md) | Record (2026-09-25 to 2026-09-26) | Photographic comparisons and platform checks of that time |
