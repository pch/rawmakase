# RAWmakase as a library

`rawmakase-core` is RAWmakase for other apps: one crate an app links to open,
decode, edit, render and export photos as RAWmakase does, without the desktop
interface. This guide is for the programmer writing that app. How the library
is being built, and what comes next, is the [library plan](embedding-plan.md).

**Status: being built.** One photo, start to finish: opening it, decoding,
editing its settings, presets and Auto, rendering a view and exporting a
file. Catalogs, the edit session, the GPU and AI Denoise come in the plan's
later steps. Nothing is stable before 1.0: pin a release tag.

## Depending on it

The library is not on crates.io. Depend on it by path, or by git at a
release tag (the first release that has it is still to come):

```toml
[dependencies]
rawmakase-core = { git = "https://github.com/pch/rawmakase", tag = "vX.Y.Z" }
```

That is the only RAWmakase crate an app names. The crates under it
(`rawmakase-model`, `rawmakase-decode` and the others) are how it is built;
their own public items promise nothing.

## The context

An app makes one [`Context`] when it starts and keeps it. Everything the
library needs to know about where it runs is given to the context as values:
RAWmakase's code in the library reads no environment variable and starts no
program to find it out.

```rust
use rawmakase_core::{CacheLimits, Context};

let ctx = Context::builder()
    .cache(&app.cache_dir, CacheLimits::default(), app.build_id())
    .build()?;
```

What the context takes today:

| Value | What for | Default |
| --- | --- | --- |
| The data folders, in order | Imported lens profiles, camera profiles and XMP presets are looked for in each one's `lens-profiles`, `camera-profiles` and `xmp-presets`; Lensfun's database and the presets' order are kept in the first | None: only RAWmakase's own profiles and presets |
| The raw defaults | What a photo without an edit starts from: Adobe Default or a preset, for every camera or for each | Adobe Default |
| The cache folder, its limits and a build id, given together | Decoded photos and AI Denoise's results are kept there, up to the limits (4 GiB and 10 GiB by default), and Camera Look's fits | No cache |

The build id names the build that made a cache's entries; entries of another
id are not read. There is no default, since a default would let a newer
library read an older one's results: give one that changes whenever your app
or the library changes, such as your app's version and build number.

More values join as the library takes them (fonts, the GPU, the ONNX
Runtime's path); a value is offered only once the library reads it from the
context.

## A photo, start to finish

```rust
use rawmakase_core::{Cancel, Decode, ExportSettings, Parameter, View};

let photo = ctx.open(path, cancel)?;            // metadata and the profiles that fit
let mut recipe = photo.starting_recipe()?;      // the context's raw defaults
photo.set(&mut recipe, Parameter::Temperature, 5200.)?; // white balance follows
recipe.curve.insert([0.25, 0.2]);               // the point curve, by its methods

let image = photo.decode(Decode::Full, cancel)?; // reads the file again
let frame = ctx.render(&image, &recipe, View::Fit(2048), cancel)?;
let rgba = frame.rgba8();

ctx.export(&photo, &recipe, &ExportSettings::default(), "out.jpg", false, cancel, |_| {})?;
```

- **A photo** keeps its metadata and the camera profiles that fit, not a
  RAW's bytes: each decode reads the file again, and refuses one changed
  since it was opened (`Changed`).
- **A recipe** is the model's `Recipe`, read freely. Change a slider with
  `Photo::set`, which brings along what depends on it, and draw it from
  `Photo::control`: its range, step, scale, value and text. Each
  `Parameter` has a name (`exposure`, `temperature`) for a binding.
  Presets apply with `Photo::apply_preset`, measuring their Auto settings
  on a decode, and `Photo::auto_tone` and `auto_white_balance` are the
  Basic panel's Auto.
- **A renderer** (`Context::renderer`) is kept by the thread that renders
  a view: the whole photo at a size, or a region at 100%. Before an edit's
  frame, `Renderer::draft_due` says whether a reduced draft should be shown
  first, as the desktop does, and `render_draft` renders it.
- **An export** develops the photo in full and writes the format its file's
  extension names, as the desktop's Export settings choose (format,
  quality, color space, HDR, size). It never replaces a file unless asked,
  and never the photo's own.

A context is cheap to clone, and a clone is the same context. It is `Send` and
`Sync`.

## Errors

Every operation returns `rawmakase_core::Error`. Its `kind()` says what
happened, for the kinds an app treats differently:

| Kind | Means | What an app usually does |
| --- | --- | --- |
| `Cancelled` | The `Cancel` the call was given was set | Nothing: the app asked for it |
| `Unsupported` | A file or request RAWmakase doesn't handle (a CMYK JPEG) | Says so, with the message |
| `Missing` | A file, camera profile or preset that isn't there | Offers to find or install it |
| `Changed` | The file is not the one the work was made for | Opens the photo again |
| `Exists` | The destination is already there | Asks whether to replace it |
| `Invalid` | A recipe, setting or value that doesn't validate | Shows the message |
| `Other` | Anything else | Shows the message |

`{}` prints the outermost message and `{:#}` the whole chain of causes, as
the `rawmakase` command line prints its errors. `ErrorKind` is
`#[non_exhaustive]`: match it with a `_` arm.

## Threads

The API is synchronous. Long operations take a `Cancel`; an export reports
progress through a callback; pixel work runs on rayon's pools. The library
starts no thread of its own.

`Context`, `Photo`, `Image` and `Error` are `Send` and `Sync`; a `Renderer`
is `Send` and belongs to one thread. An editor that renders while its
controls move keeps one render thread and gives it only the newest edit,
through a `Mailbox`: the edit waiting is replaced, the running one finishes.
`Shown` keeps the rules for what is on screen: a frame older than the one
shown is dropped, and a draft never replaces its own edit's frame. It
counts by the mailbox's numbers, not an interface's own: rendering the same
edit again for a new view puts it in the mailbox again, which numbers it
anew, so its frame shows.

## What is shared in a process

Two contexts in one process, with different folders, each have their own
caches. What the library keeps once per process, and why sharing it can't
change a result, is listed in the plan
([What stays process-wide](embedding-plan.md#what-stays-process-wide)) and
moves here as each part is built.

## Changes

The library's public items are listed in
[`crates/rawmakase-core/public-api.txt`](../crates/rawmakase-core/public-api.txt),
which CI compares with the code. Each change to them is noted here.

- **Started**: `Context` with its cache (folder, limits, build id), `Error`
  and `ErrorKind`.
- **Data folders**: `ContextBuilder::data_dirs` and `Context::data_dirs`; the
  cache's build id now keys its entries.
- **One photo, start to finish**: `ContextBuilder::raw_defaults`;
  `Context::open`, `presets`, `load_preset`, `renderer`, `render` and
  `export` (the `export` feature, on by default); `Photo`, `Decode`, `Image`
  and `DecodeStats`; `Parameter`, `Control`, `Scale`, `Preset` and `Applied`;
  `Photo::save_recipe` and `Context::load_recipe`;
  `Renderer`, `Frame` and `View`; `Mailbox`, `Numbered`, `Taken` and `Shown`; and the
  model's `Recipe`, `Metadata`, `CameraProfile`, `ToneCurve` and `Inserted`,
  interop's `RawDefaults` and `ExportSettings`, and `Cancel`, re-exported.
  `ExportSettings::load` and `save` still read and write the desktop's own
  file in its data folder; step 15 gives them a path. The values their
  fields take are re-exported too: `Format`, `Include`, `OutputSpace`,
  `HdrOutput`, `HdrGamut` and `HdrTransfer`, and `DefaultChoice` and
  `CameraDefault`. `Image::output_size` and `size_in` return a `Result`,
  since they resolve the recipe as the renderer does. An export writes the
  format its file's extension names, whatever the settings' `format`.
