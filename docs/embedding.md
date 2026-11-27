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
| The cache folder, its limits and a build id, given together | Decoded photos and AI Denoise's results are kept there, up to the limits (4 GiB and 10 GiB by default), and Camera Look's fits | No cache |

The build id names the build that made a cache's entries; entries of another
id are not read. There is no default, since a default would let a newer
library read an older one's results: give one that changes whenever your app
or the library changes, such as your app's version and build number.

More values join as the library takes them (fonts, the GPU, the ONNX
Runtime's path); a value is offered only once the library reads it from the
context.

What a photo without an edit starts from is not the context's, since an
app's preferences change it while photos are open: `Context::raw_defaults`
reads the raw defaults the app's preferences choose (Adobe Default or a
preset, for every camera or for each), and each photo is given them. An app
makes new ones when its preferences change; a photo started from the old
ones keeps what it started from.

## A photo, start to finish

```rust
use rawmakase_core::{Cancel, Decode, ExportSettings, Parameter, RawDefaults, View};

let defaults = ctx.raw_defaults(&RawDefaults::default()); // Adobe Default
let photo = ctx.open(path, cancel)?;            // metadata and the profiles that fit
let mut recipe = photo.starting_recipe(&defaults)?;
photo.set(&mut recipe, Parameter::Temperature, 5200.)?; // white balance follows
recipe.curve_mut().insert([0.25, 0.2]);         // the point curve, by its methods

let image = photo.decode(Decode::Full, cancel)?; // reads the file again
let frame = ctx.render(&image, &recipe, View::Fit(2048), cancel)?;
let rgba = frame.rgba8();

ctx.export(&photo, &recipe, &ExportSettings::default(), "out.jpg", false, cancel, |_| {})?;
```

- **A photo** keeps its metadata and the camera profiles that fit, not a
  RAW's bytes: a decode reads the file again, unless the app still holds
  the photo's last full decode, and refuses one changed since it was
  opened (`Changed`).
- **A recipe** is the library's `Recipe`, changed by the library's rules.
  Change a slider with
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
- **An export** develops the photo in full, using a full decode the app
  holds, and writes the format its file's extension names, as its
  `ExportSettings` choose (quality, color space, HDR, size, metadata). It
  never replaces a file unless asked, and never the photo's own.

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
controls move runs a `RenderLoop` on a render thread of its own
(`RenderLoop::run`, which blocks) and submits each edit to it from any
thread as a `RenderJob`. The edit waiting is replaced and the running one
finishes and shows; a draft renders first when `Renderer::draft_due` asks
for one, and its frame follows once no newer edit has come for the quiet
time (`RenderLoop::QUIET`, 50 ms, unless `quiet` sets another). A frame
older than the one shown, and a draft after its own edit's frame, never
reach the app, and a job whose `cancel` the app sets, because its photo or
view is gone, is dropped without a word. A render that panics is reported
as its job's failure, and the loop goes on. `RenderLoop::close` ends `run`
after the render it is in; nothing joins the thread.

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
- **The model's Recipe from main**: `Recipe::lens_file_correction` and its
  three methods, which came with the re-exported `Recipe`.
- **Export settings of the library's own**: `ExportSettings` is the
  library's, with only what an export honours: the color space or HDR
  encoding, quality, lossy JPEG XL, a JPEG's size limit, a long edge, the
  resolution and which metadata goes in. The desktop dialog's settings it
  replaces (their folder, naming, watermark, Send To, and `load` and
  `save` of the desktop's own file) are no longer shown, nor `Format`: an
  export's format is its file's extension.
- **A photo's metadata and profiles of the library's own**: `Photo::metadata`
  is the library's `Metadata`, plain values (camera, lens, exposure, the
  decoded size, whether it is a rendered file or monochrome), and
  `Photo::profiles` gives `Profile`s, with their names and what kind they
  are. The decoder's `Metadata` and the model's `CameraProfile`, whose
  matrices, tables and renderer methods they showed, are no longer
  re-exported, and `Image::metadata` is gone.
- **The rest of the surface the library's own**: `Recipe` is the
  library's, over the model's: its point curve through `curve` and
  `curve_mut`, its profile's name through `profile`, HDR through `hdr`
  and `set_hdr`, `validate`, and `uses_ai_denoise`; its sliders through
  `Photo::set`, and its profile, one of `Photo::profiles`, through
  `Photo::set_profile`, which starts it at 100% and has the white balance
  and Black & White follow it, as the desktop's Profile menu does. `Parameter` is the library's list of the model's settings,
  with `ALL`, `name` and `named`. `Preset` is a handle with its id, name,
  group and kind; `Applied`, `RawDefaults`, `DefaultChoice` and
  `CameraDefault` are the library's values. `Cancelled`, which
  `Cancel::check` returns, is re-exported. The model's methods on these,
  which took the decoder's metadata and camera profiles, are no longer
  shown, and the listing check refuses any item that names a type an app
  can't name from the library.
- **Raw defaults given to each photo**: `ContextBuilder::raw_defaults` is
  gone. `Context::raw_defaults` reads raw defaults into `Defaults`, which
  `Photo::starting_recipe` takes, so an app whose preferences change gives
  new ones to the photos it opens next.
- **A held full decode used again**: while an app holds an `Image` of a
  photo's last full decode, the photo's next full decode and its export
  use it, through the same context.
- **A render loop in place of its parts**: `RenderLoop`, `RenderJob` and
  `Rendered` run what an editor's render thread does, which the Electron
  example had written out; `Mailbox`, `Numbered`, `Taken` and `Shown` are
  no longer shown.
