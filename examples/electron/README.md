# A photo editor in Electron on RAWmakase's library

A reference app, not a product: one window that opens a photo, edits it with
the Basic sliders (Temp, Tint, Exposure, Contrast, Highlights, Shadows,
Whites, Blacks) and a point tone curve, and exports it, built on
[`rawmakase-core`](../../crates/rawmakase-core/src/lib.rs) (its
[guide](../../docs/embedding.md)) as an app outside RAWmakase would be. It
is one of the library plan's reference apps
([`docs/embedding-plan.md`](../../docs/embedding-plan.md), "Proof").

It is a Cargo workspace of its own, with its own `Cargo.lock`, and
`rawmakase-core` is the only RAWmakase crate it depends on. The photo, its
recipe and the renderer stay on the Rust side; what crosses into JavaScript
is a control's name and value, a curve point added, moved or removed, the
controls' state as JSON and frames as RGBA.

## Build and run

Rust (stable) and Node are needed; `npm install` fetches Electron.

```sh
npm install
npm run build                # the Node addon (release), into native/
npm start                    # builds it, then opens the window
npm start -- --photo=../../tests/corpus/charts/synthetic-d65.dng
```

Open a photo, drag the sliders (double-click a label to reset it), click the
curve to add a point, drag it, double-click it to remove it, and Export. The
format is the extension you choose: JPEG, 16-bit TIFF or PNG, in sRGB. A file
already there is replaced, as the save dialog asked.

## How it is put together

| File | What it is |
| --- | --- |
| [`src/editor.rs`](src/editor.rs) | The editor: the photo, its recipe and the renderer, all on the Rust side. An edit arrives as a control's name and a value, or a curve point added, moved or removed; the controls' state goes back as JSON and frames as RGBA. No Node in it. |
| [`src/worker.rs`](src/worker.rs) | The render thread, on the library's `Mailbox`, draft rule (`Renderer::draft_due`) and stale-frame rule (`Shown`). The newest edit replaces the one waiting, the running render finishes and shows, a draft shows first when the library says one is due, and a render is cancelled only when its photo or view is gone. |
| [`src/binding.rs`](src/binding.rs) | The Node addon (napi-rs) over the editor, built with the `addon` feature. Opening and exporting return promises and run on Node's thread pool; frames come back through a thread-safe function. |
| [`main.js`](main.js), [`preload.js`](preload.js) | Electron's main process, which loads the addon and passes edits and frames over IPC, and the small API the page gets. Context isolation and the sandbox are on; the page has no Node. |
| [`renderer/`](renderer/app.js) | The page. Sliders are built from what the library says of each control (range, step, scale, text); nothing about a control is written in JavaScript. |
| [`src/bin/headless.rs`](src/bin/headless.rs), [`renderer/probe.js`](renderer/probe.js) | The measures' tools: the editor without a window, and a scripted run of the window. |

Where the app keeps RAWmakase's data (`main.js`, `folders`) is given to the
library as values: a data folder, and a cache folder with the addon's build
as the cache's build id. The app sets no environment variable and writes no
`unsafe`; its root denies it, since what `#[napi]` expands to allows it for
itself, and every other module forbids it.

## Tests and measures

```sh
cargo test                   # the editor and the render thread, without Node
cargo test -- --ignored      # the desktop's pixels; needs the desktop binary:
                             #   cargo build --bin rawmakase             (repository root)
npm run probe                # the scripted window: timings and screenshots in probe-out/
cargo run --release --bin headless -- bench --home <dir> <photo> --edge <pixels>
```

- **Scheduling and cancellation** ([`tests/scheduling.rs`](tests/scheduling.rs)):
  the render thread with a renderer the tests hold and release, so nothing
  depends on how fast the computer renders: the newest edit replaces the one
  waiting, the running render shows, a due draft shows first and its frame
  follows when quiet or gives way to a newer edit, a cancelled render is never
  shown or reported, a panic fails its edit alone, and a stopped worker runs
  nothing pending. One test uses the library's renderer to see a cancelled
  render return the cancelled kind.
- **The editor** ([`tests/editor.rs`](tests/editor.rs)): on the color-chart
  DNG, as the addon drives it: controls by name, the curve, a new view or
  photo.
- **The desktop's pixels** ([`tests/pixels.rs`](tests/pixels.rs)): the
  example's 16-bit TIFF and `rawmakase render --recipe` of the same photo with
  the recipe the example saved decode to equal pixels, each program with an
  empty data folder of its own.
- **Live editing** (`npm run probe`, `headless bench`): the time from a
  slider's change to pixels in the page, in parts, beside the same render
  called from Rust, and the editor itself timed from a slider's change to its
  frame's RGBA with no Node. Each records the frame's size.

Node and Electron are not built in CI; its "Reference apps" job formats,
lints (with and without the addon) and tests the Rust side.

## What it does not do

No Undo or Redo (the edit session is still in the desktop app), no GPU (no
wgpu backend is compiled into this build), no crop, masks or presets, no 100%
view, and no raw defaults other than Adobe Default.
