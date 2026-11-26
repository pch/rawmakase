# A library for other apps: plan

Status: Plan (2026-11-26), being built on one branch; no step has landed.
Its API's shape was tried in a prototype (stage 0), and the plan was
corrected from it. One library that another app links to open, decode,
edit, render and export photos as RAWmakase does, without the desktop
interface. The [headless plan](headless-plan.md) covers a command-line
binary and a server; [the last section](#beside-the-headless-plan) says
where the two meet.

## Why

Safelight, an iPad app, already builds on RAWmakase
([#1287](https://github.com/pch/rawmakase/pull/1287#issuecomment-7168830238)). Its
Rust core depends on twelve workspace crates by path, from a fork of eight
commits, and keeps hand copies of thirteen pieces of the desktop app
([its fork notes](https://github.com/ljbergmann/safelight/blob/main/docs/FORK.md)).
That it worked at all shows the domain code never needed a window. That it
took twelve crates, a fork and thirteen copies shows there is no library:
there are the desktop app's parts, which happen to be crates.

## Goal

A library a Rust programmer would choose to use:

- **One crate to depend on**, with an API designed as one thing, documented,
  and with examples.
- **Modular.** Everyone gets opening, decoding and rendering. Editing and
  writing files are on by default and can be left out. Catalogs, AI Denoise,
  Merge to HDR, installers and the GPU are chosen by feature, and what is not
  chosen is not compiled.
- **Configured by values.** Where data lives, which fonts, which GPU and
  which ONNX Runtime are given when the library is set up. RAWmakase's code
  in the library reads no environment variable and starts no program.
- **Safe.** Nothing in its API is `unsafe` or needs `unsafe` to use.
- **Errors a caller can act on**: cancelled, unsupported, missing, changed on
  disk, invalid, and the rest, as a type and not as text.
- **The desktop app and the command line use it too**, so it is the way
  RAWmakase itself is put together, not a second path beside it.

Not goals:

- A promise of stability. Nothing is promised before 1.0, as for
  [extensions](extensions-plan.md#decisions-taken); an app pins a release
  tag, and the library's public items are listed and tracked.
- Publishing to crates.io.
- A C or Swift interface. An app writes its own bindings over the library,
  as Safelight does with UniFFI.
- Extensions on iPadOS. The extension host starts programs and passes shared
  memory, and it is not part of the library.

## What is there today

Checked against `main` at `9d6dc2f7d`; the prototype's corrections against
`d1085d3ff`.

- **The crates build without the app and pull no git dependency or GUI
  crate.** They are the app's parts: `0.1.0`, `publish = false`, no README,
  broad `pub` surfaces, `anyhow` errors nearly everywhere.
- **Configuration is read from the environment, deep inside.** The crates a
  library would be made of read the environment in about forty places
  outside tests. The data folders go through `rawmakase-protocol`'s `paths`
  (`RAWMAKASE_DATA_DIR`, `HOME`, `XDG_DATA_HOME`, `APPDATA`), which the model
  crate depends on that crate for; the cache folder reads its own
  (`decode_cache::cache_dir`); export settings, keyword sets and font folders
  ask the `directories` crate, two of them from a `Default`
  (`ExportSettings::default`, `KeywordSets::default`); fonts read
  `XDG_DATA_DIRS` and fontconfig's variables; the GPU's backend, flags and
  power preference come from wgpu's variables
  (`crates/rawmakase-engine/src/develop/gpu/mod.rs:186-222`); ONNX Runtime
  reads `RAWMAKASE_ORT_LIB` and looks beside the executable; a cache entry is
  keyed by the executable's size and time (`decode_cache::build_identity`);
  the engine dumps a tone rule when `RAWMAKASE_RULE_DUMP` is set. Callers far
  from any entry point depend on it: `photo::open` reads the imported lens
  profiles from the ambient folder (`LENS_PROFILES`,
  `crates/rawmakase-decode/src/photo.rs:18`), `camera_profiles::installed`
  the profile library, `Catalog::open` this computer's id, the catalog's
  file operations the folders they must never move
  (`file_ops/plan.rs:419`), export the watermark folder,
  `camera_look::with_default` its cache, `DevelopDefaults::load` the preset
  library. User assets are searched in two folders, the data folder and the
  older Linux one (`storage::asset_dirs`). Some lookups have a public twin
  that takes the folder or the value (`Catalog::open_as`,
  `Computer::load_from`, watermarks' `presets_in` and `ready_in`, Lensfun's
  `Handle::Given`, `DevelopDefaults::with_presets`); others don't
  (`listed_from` is for tests, `ExportSettings::load_from`,
  `lcp::import_into` and Camera Look's `default_profile` are private, the
  preset library has none). Outside tests, `photo::open` and `open_source`
  are called from about forty places, `camera_profiles::installed` from
  about fifteen, `data_dir()` from about sixty-five.
- **Three places start a program**: a watermark's system font on Linux
  (`fc-match`, `crates/rawmakase-export/src/watermark/fonts.rs:274`), the
  computer's name (`scutil` and `hostname`,
  `crates/rawmakase-catalog/src/catalog/locations.rs:104-123`), and AI
  Denoise's benchmark, compiled into its crate (`ps`,
  `crates/rawmakase-denoise/src/bench.rs:45`).
- **Process-wide state holds more than caches.** The lens profile library,
  the installed Lensfun database, the camera profile headers
  (`CAMERAS`, which each scan trims to its own folder) and the fonts
  (`FAMILIES` and the system faces, `watermark/fonts.rs:60,202`) are one per
  process and belong to a folder. The catalog's file readers and the
  thumbnail budget are set through global hooks
  (`catalog::install_file_readers`, `preview_cache::set_thumbnail_budget`),
  and `export::remove_unfinished` deletes every export in flight in the
  process. The ONNX Runtime is loaded once and does not record from where.
- **The mask store is process-wide on purpose.** A raster is named by the
  digest of its pixels. A catalog's reader is registered when the catalog is
  opened for showing (`mask_assets::add_loader`, from two places in `src/`)
  and is kept after the app moves to another catalog, so an export started
  under the first still finds its rasters
  (`crates/rawmakase-model/src/storage/mask_assets.rs:96`).
- **A cancelled job is an error with a text**, in seven wordings
  ("Development cancelled", "Opening cancelled" and others). An invalid
  recipe, a file changed since a job was made and a missing profile are
  texts too; only some causes are types (`raster::Unsupported`,
  `InferenceError`, `AssetError`, `InstallError`, Merge to HDR's).
- **Unsafe code is where the system needs it.** In the crates a library
  would be made of: decoding (the system HEIC decoders, the AV1 decoder's
  interface, thread priority, one cache record's byte layout),
  `rawmakase-fs`, Core ML in inference, and one memory-mapped font in export.
  The engine has none written, but derives `bytemuck`'s traits for its GPU
  uniforms, which expand to `unsafe impl`. Fourteen of the twenty crates
  have none written; five say so with `forbid(unsafe_code)`.
- **Threads.** The crates start threads in several places: AI Denoise's
  worker; scoped threads that end before the call returns (export lanes,
  the catalog's ingest readers); a loader thread for Core ML that is left
  behind if Core ML hangs past a minute; the export queue's worker; the
  benchmark's sampler. `ai_denoise::CommandLine` waits up to three seconds
  for its service in `Drop`, as [shutdown](shutdown.md) records.
- **Logic with no window in it lives in the app crate.**
  - `edit_session` needs `rawmakase-model` and one string helper of
    `rawmakase-interop`. The sequences that keep it consistent are the
    editor's: the save before navigation (`Editor::flush`,
    `src/app/develop/workflow.rs:201`), autosave (`:271`), the save with a
    deadline (`:338`), a finished background save (`:404`), a frame bound to
    the photo's load (`EditFrame`, `src/app/develop/editing.rs:7`), export
    options counted as a change (`editing.rs:175`), opening a stored edit
    (`src/app/events.rs:672-749`).
  - `src/lensfun_install.rs` installs Lensfun's database into a folder it is
    given. `crates/rawmakase-model-install` installs model packs and needs
    `rawmakase-inference` for one plain record. Each has its own lock, HTTP
    client settings, bounded reads and cancellation. `rawmakase-denoise`
    and the AI masks extension depend on the model installer, and so on its
    HTTP client, to ask whether a pack is installed.
  - The Library's sort (`src/app/library/sort.rs`, with the label order it
    takes from `photo_metadata`), reading a photo's info
    (`src/catalog_session/backfill.rs:303-373`, with `can_read` from
    `background.rs:69`), importing profiles and presets from files
    (`src/app/presets/bulk_import.rs`, 1,330 lines with the editor's task
    around three importers).
  - `render` and `render-batch` (`src/render_command/`), which use AI
    Denoise, HDR masters and gain maps, Auto, presets with measures, and two
    developer outputs (`--tap` and a line of decode diagnostics).
  - Tool and panel math in view code: some pure, some pure but in
    `egui::Vec2`, `Pos2` and `Rect`, some a rule inline in a function that
    draws.
- **Decoding does not compile for iOS** (`raster/heic.rs` has no arm for an
  Apple platform that isn't macOS, and its ImageIO dependencies are in a
  macOS-only table), and AI Denoise prefers the CPU on an iPad
  (`preferred_provider`, `crates/rawmakase-denoise/src/lib.rs:279`).
- **The engine's GPU backends are chosen by accident.** `rawmakase-engine`
  depends on wgpu with only `wgsl` on; eframe turns the backends on in the
  same build. Decoding depends on the engine, so every user of the crates
  compiles wgpu. The preview renderer and its GPU processor are used from
  one thread, the app's render worker. That worker, not the engine, holds
  what makes editing feel live: when a reduced draft is shown before the
  full frame and at what size (`QUICK`, `DRAFT_PIXELS` and `draft_edge`,
  `src/app/worker/renderer.rs:30-46`), and that a new edit of the same view
  replaces the one waiting and lets the running render finish
  ([preview performance](preview-performance.md) records why cancelling it
  showed no frame during a drag). A frame presented on the GPU is finished
  there; a frame asked for as pixels is read back as floats and finished on
  the CPU, measured at three to six times the presented frame's time.
- **A recipe is data, but not every field stands alone.** It serializes
  whole, a camera profile's tables with it: hundreds of kilobytes for
  RAWmakase's own profile, megabytes for an imported one. The white balance
  the renderer multiplies by (`wb`) is worked out from Temp and Tint and
  the photo (`Recipe::update_wb`). A tone curve keeps its points sorted,
  spaced and counted through its own methods. The model already has the
  form a front end wants: each slider is a parameter with an id, a range
  and its formatting (`model::params`), changed through
  `model::edit::change_setting`, which applies what the change brings with
  it.
- **The app never links the selection models**, by rule
  (`scripts/deps-crates.txt`: `rawmakase never rawmakase-selection
  rawmakase-remote rawmakase-ai-masks`); the AI masks extension's program
  runs them. `scripts/deps.py` reads optional dependencies as dependencies,
  and Cargo builds a workspace with its members' features united.
- **CI builds nothing as an embedder does.** Export, AI Denoise, Merge to
  HDR, the installer and the selection models are tested only in a build
  with the app, no target but the three desktop systems is built, and no
  example renders a photo with the crates alone.

## The design

### One library, built from the crates

A new crate, `rawmakase-core`, is the library. An app depends on it and, for
the selection models, on one crate beside it. The crates under it stay as
they are: they are how the library is built and how its layering is checked
(`scripts/deps-crates.txt`, `scripts/crate-closures.sh`), not what anyone
outside the workspace depends on, and their own `pub` items promise nothing.

The library is three things:

- **Handles of its own** for what an app does things with: a photo, a
  decoded image, a renderer, a catalog. They are the library's types, with
  the library's methods, and the crates' own (`Raw`, `CameraImage`,
  `PreviewRenderer`, `FullSize`) are not shown through them. A decoded
  image's buffers and its recovered copy are not an app's to reach into;
  what a caller may want to know of a decode (its size, how many samples
  clipped as it was scaled to integers, its highest level) the handle gives
  as plain numbers. The decode records no highest level: the handle measures
  it with one pass over the pixels, as `render`'s line of numbers does
  today. Its scale factor is not shown: every decode sets it to 1.0
  (`CameraImage::scale_factor`; only a test sets another), so it says
  nothing yet.
- **A photo handle that knows its file.** Decoding consumes the opened file
  (`Raw::develop` takes `self`), so a photo handle keeps what it opened
  and opens the file again for each later decode: the desktop's staged
  open, Half then Full, reads the file twice, as the app's does today.
  Export needs the source file too (the overwrite check, the camera's
  EXIF), so it takes the photo, or a decoded image that knows the photo it
  came from, and never a bare buffer.
- **The edit's values, as they are.** A recipe, a photo's metadata, export
  settings and the edit session are the model's own types, and wrapping
  them would only copy them. An app reads them freely. It changes a recipe
  through the model's rules where a setting has them: a slider as a
  parameter, by its id, so that what depends on it follows (white balance
  from Temp and Tint), and a curve through the curve's methods. The guide
  says which fields are safe to assign. They are
  re-exported from the model, interop and editing crates, and the types
  their fields and methods name come with them: that part of those crates
  is the library's surface, and is treated so. A module joins the surface
  when its items are documented (`missing_docs` is turned on for it where
  it is defined; the lint in the library crate covers only what the
  library defines).
- **A context**, described below, which every operation that needs
  configuration goes through.

`docs/embedding.md` is its guide. A listing of the library's public items,
the re-exported ones among them, is kept in the repository and compared in
CI, so a change to the surface is seen in review and written into the
guide's changes list.

### Parts are chosen by feature

| Part | Feature | Brings |
| --- | --- | --- |
| Opening and decoding photos; recipes, XMP, presets, profiles; rendering to pixels on the CPU | always | model, interop, raw, decode, engine |
| The edit session, History and the tools' rules | `edit`, on by default | `rawmakase-editing` |
| Writing files: JPEG, TIFF, PNG, JPEG XL, HDR, watermarks | `export`, on by default | `rawmakase-export` |
| GPU previews | `gpu-metal`, `gpu-vulkan`, `gpu-dx12` | wgpu's backend |
| Catalogs | `catalog` | `rawmakase-catalog`, SQLite |
| AI Denoise | `ai-denoise` | `rawmakase-denoise`, the ONNX Runtime binding |
| Merge to HDR | `hdr-merge` | `rawmakase-hdr-merge` |
| Downloading model packs and Lensfun's database | `install` | `rawmakase-install`, an HTTPS client |

A feature is an optional dependency on a crate, so the boundaries between
the parts are the ones Cargo already keeps: none of the base's crates may
reach the catalog, inference or the installer today, by
`scripts/deps-crates.txt`. The app turns every feature on, and links all of
these crates already.

Two of today's dependencies cut across the table and are removed:

- `rawmakase-denoise` links the installer to ask whether its pack is
  installed. That question is about files and digests. The record of a
  pinned file and the receipt that says a folder's files were checked (its
  format, writing it, comparing it) get one owner, `rawmakase-model`'s
  `storage`, which the installer, AI Denoise and the AI masks extension all
  reach already. AI Denoise then links no HTTP client, and installing its
  pack is the `install` feature's. `rawmakase-inference` keeps its
  `ModelFile` records, and their users turn them into the model's.
- Decoding depends on the engine, so the smallest build compiles wgpu with
  no backend. The engine's GPU port becomes a feature of the engine in the
  last steps of this plan; until then the cost is build time, not behavior.

### The selection models stay beside the library

Select Subject, Sky and Background are not a feature of the library. The
desktop app must never link them, the library is what the app is built on,
and a feature of a crate the app depends on is a dependency of the app, to
the rule's check and to Cargo's builds of the workspace. More than that,
they need nothing of the library and it needs nothing of theirs: pixels go
in and coverage comes out.

So `rawmakase-selection` is the one other crate an app may depend on, with
its own documented surface, which re-exports the types of the crates under
it that its API names (the load options, the provider, its error, a model's
files, and the same `Cancel` the library re-exports), so an app names no
third crate. It takes the ONNX Runtime's path as a value, as the library
does, and an app that uses both gives both the same one, since a process
loads one runtime. The library's side is what it already is for
the AI masks extension: it renders the image a selection is made from, and
takes coverage back as a mask's raster with what the mask records of its
source. An app that runs the models links both; an app that gets coverage
some other way links the library alone.

### A context holds the configuration

An app makes one `Context` and keeps it. It is given, as values:

- the data folders, in order. The first is written to, and holds Lensfun's
  database and the watermarks; all are searched for camera profiles, lens
  profiles and presets, as the desktop searches its data folder and the
  older Linux one for those three today;
- the local data folder (models, this computer's id);
- the cache folder with its limits and a build id, given together: cache
  entries are keyed by the id, and there is no default for it, since a
  default would let a newer library read an older one's results. The desktop
  keeps passing what it keys by today, so its caches stay valid;
- the system font file and the font folders, where the app wants watermarks
  to use them;
- the raw defaults, as a value: today they are read from the desktop's
  `session.json` in the app crate (`app::user_raw_defaults`), and the
  library can open a photo "as the app opens it" only when it is given
  them. An app that gives none gets Adobe Default;
- the GPU, when a `gpu-*` feature is on: the backend, flags and power
  preference to open one with, or the app's own device and queue;
- the ONNX Runtime library's path, for AI Denoise;
- this computer's name, and the folders a catalog's file operations must
  never move, for catalogs.

It owns what is made from them, each shareable between threads: the camera
profile, lens profile and preset libraries with their caches, a handle on
the installed Lensfun database that sees an install made while the app
runs, the raw defaults, the fonts, the decode and AI Denoise caches, AI
Denoise's service, and the list of its own exports in flight. What the
desktop changes while it runs stays changeable: a cache's limit and folder,
the Neural Engine choice, and a cache moved back to its default folder goes
to the context's, not to one looked up. A renderer is not owned by the
context but made by it: it holds the pyramid and, with a `gpu-*` feature,
the GPU's state for one view, and belongs to the thread that renders, as
the app's does. It gives a view what the desktop's gives its own: the whole
photo at a size, a region at 100%, and a reduced draft first when the full
frame will be slow, since a CPU render of a whole photo takes a few tenths
of a second. The rule for the draft moves from the app's worker to the
engine and has one owner, which the app's worker then asks too. Being
synchronous, the renderer does not show two frames from one call: it says
whether a draft is due and at what size, and the caller renders the draft
and then the frame. Which edit is rendered next is the caller's, since it
owns the thread, but the rule for it is the library's to carry, because the
prototype needed it whole: a single-slot mailbox, as the app's
`app::worker::Latest`, where a new edit replaces the one waiting and the
running render finishes, with the draft-then-frame sequence over it; and
the rules for stale frames every front end repeats: a frame older than the
one shown is dropped, a draft never replaces its own frame, and a new
photo's first frame is numbered after the edits made before it. Step 5
builds them, with what a front end needs beside them: the frame as 8-bit
RGBA (the engine gives float RGB), the tone curve sampled for drawing, and
the size a decoded image renders to at a view's size, without which a
caller can't tell whether a draft is due.

Operations are methods on the context or on what it hands out. Two contexts
in one process, with different folders, each see their own libraries,
caches and fonts.

**RAWmakase's code in the library reads no environment and starts no
program.** Each function that does so today gets a form that takes the
value, callers below an entry point take it as an argument, and when the
last caller is moved the ambient form leaves the library. Functions take
what they need (a lens library, a folder), not the context: the context is
the library crate's, and the crates under it stay free of it. The app's
workers hold the context and pass from it.

What that means case by case:

- Folders, fonts, the runtime's place, the computer's name: values of the
  context. `rawmakase-model` stops depending on `rawmakase-protocol`.
- The `directories` crate leaves the library's crates. An export setting's
  default destination is unset until the app fills it, and the Lightroom
  keyword-set folder is found by the app.
- The GPU's options are the context's; the engine stops asking wgpu to read
  the environment. What wgpu, a graphics driver or the dynamic linker read
  on their own is theirs.
- Developer tools go where developers run them: AI Denoise's benchmark
  leaves its crate for the app's binary, and the tone rule's dump becomes an
  argument the developer command passes.
- Temporary files are made beside their destination or in the cache folder.
  Two are not today: a downloaded profile is staged in the system's
  temporary folder (`camera_profiles::catalogue::Entry::install_into`), and
  so is the snapshot a Lightroom catalog is imported from.

The desktop's policies stay the desktop's: `RAWMAKASE_DATA_DIR`, each
platform's default folders, where fonts and the runtime are looked for, the
GPU variables, the developer switches. They move to one window-free module
of the app crate (`environment`), which reads them once at startup and
builds the context, and which the command line shares.
`rawmakase-protocol`'s `paths` stays what it was written as: where a control
client finds the running app.

A check in CI holds this, in the style of `scripts/deps.py`
(`scripts/ambient.py`, which `deps.py check` runs). It names the crates the
library is made of, and the selection crate, and searches their code
outside tests for `std::env`, the `directories` crate, wgpu's `from_env` and
`with_env`, `Command::new`, the ambient data and cache folders
(`data_dir()` and the like called with no folder, and
`rawmakase_protocol::paths`) and a temporary file made with no folder given,
leaving out what is compiled only for tests (`cfg(test)` and the
`test-support` features). It compares what it finds with a list in the
repository (`scripts/deps-ambient.txt`, a count per file and kind), which
may only shrink, and is empty when this plan is built.

### What stays process-wide

Shared state is kept only where sharing cannot change a result, and each
case is written into the guide:

- **The mask store**, with its readers. A raster is named by the digest of
  its pixels and checked against it when read, so whichever reader has it
  gives the same pixels; and an export in flight must outlive the catalog
  it started under. A reader is registered through the library's catalog
  handle for the catalog an app shows, as the app does today, and stays
  registered; `Catalog::open` itself never registers, because Export as
  Catalog and the app's workers open connections that must not. Two
  contexts therefore share rasters, and nothing else.
- **The recipe asset cache and the look sources**, content addressed and
  verified the same way. The look sources are never freed, which a
  long-running process will notice before a desktop does.
- **Pure caches**: a color transform, the parsed profile catalogue, a
  Camera Look fit being made.
- **The ONNX Runtime library**, which a process loads once. Inference
  records the path it loaded, and a context that names another is refused
  with the reason when it first needs a model.
- **The lock that keeps GPU work off a surface being resized.**
- **The last AI Remove patches decoded**, four at most and up to 192 MiB
  together, kept by their content.
- **Two things a process can only do once.** Where HEIC is decoded by the
  system's libheif (Unix other than Apple's), the library is opened once
  and a handler is registered for the process's exit, which waits up to two
  seconds for decodes still running and then ends the process at once
  rather than let libheif's own teardown crash under them. On Apple
  platforms, a Core ML load that stalls past a minute turns the Neural
  Engine off for the rest of the process, for every context.

Everything else moves: the libraries, the Lensfun database, the camera
profile headers and the fonts to the context; the catalog's file readers to
an argument of opening a catalog, which the library passes itself, having
both the catalog and decoding; the thumbnail budget to the preview cache;
the exports in flight to the context, so one context's cleanup never
deletes another's.

### A safe API

No item of the library is `unsafe`, and using it needs none. The library
crate and `rawmakase-editing` forbid unsafe code (`forbid`, which no
attribute inside can lift), and so does every crate
that has none today. The engine forbids it too: the lint accepts what
`bytemuck`'s derive expands to for its GPU uniforms.
What is left is where the operating system or a decoder requires it, each
block with its `SAFETY` note: decoding's system HEIC decoders, AV1
interface, thread priority and cache record; `rawmakase-fs`; Core ML; one
memory-mapped font in export.

A crate over the library that uses a binding generator can only deny unsafe
code: napi-derive's expansion carries `allow(unsafe_code)`, which `forbid`
rejects, and UniFFI's scaffolding does the same. The reference apps say
`deny(unsafe_code)` and write none; the library crate keeps `forbid`.

### Errors a caller can act on

The library returns one error type. It says which kind of thing happened,
for the kinds a caller treats differently, and carries the chain of causes:

- cancelled;
- unsupported, with the reason (a CMYK JPEG, a HEIC on a system with no
  decoder);
- missing: a profile, a preset, a model, a mask's raster, a file;
- changed: the file is not the one the job was made for;
- exists: the destination is already there, and the call was not told to
  replace it;
- invalid: a recipe or setting that doesn't validate;
- anything else, as its message.

The crates under it keep `anyhow`, with a typed cause at each point a kind
comes from. Unsupported, a missing raster or model, and an install's
failures have one. Five are added before the library can tell them apart:
`Cancelled`, in `rawmakase-cancel` with a check that returns it; an invalid
recipe or setting; a file's identity that no longer matches; a profile or
preset that isn't there; a destination that already exists. Each carries the wording used today as its own
message, as `raster::Unsupported` does, and not as a context laid over a
second cause: the command line prints errors with their chain, and a chain
one link longer would print differently.

### Threads

The library's API is synchronous. Its types are `Send` and `Sync` where an
app would share them, and say so; long operations take a `Cancel` and
report progress through a callback; pixel work uses rayon's pools. The
guide says which threads it starts and when each ends:

- AI Denoise's worker, the context's own, stopped through a call with a
  deadline;
- threads inside one call that end before it returns (the lanes of a batch
  export, a catalog's ingest readers);
- on Apple platforms, a thread that loads a model into Core ML, which the
  library stops waiting for after a minute and which then outlives the call.

The library adds no join in `Drop`. The export queue's worker and the
command line's service wrapper, which waits up to three seconds as it is
dropped, are the app's and the command line's, and are not in it.

### Editing is a crate above the model

`rawmakase-editing` owns editing a photo apart from any window: the edit
session (History, frames, gestures, save state) and what the tools do, in
plain types. It depends on `rawmakase-model` and `rawmakase-interop` and
never reaches rendering, decoding, the catalog or inference.

Neither existing crate could hold it and stay what it is. The model is what
an edit is, as values: a session is state that changes over time, with a
clock in its save state. Interop is file formats, and nothing here is one.

Tool math is split by what it is:

| Kind | Example | Owner |
| --- | --- | --- |
| A rule about a setting's values: limits, order, defaults, names, and reading them | Point Color's range handles, the curve splits' spacing, Duplicate's name and the group limit, a new spot's fallback source offset, a Red Eye correction's size from the last one, a raster mask's extent | `rawmakase-model`, beside the setting |
| What a tool does with a pointer, and the geometry it works in, whatever draws it | the wheel's fine drag and its lock to hue or saturation, crop handle resizing, the crop's aspect presets, the Straighten ruler, the crop guides' lines, brush stroke spacing, Before and After's transfers | `rawmakase-editing`, one module per tool |
| Drawing | colors, strokes, hit areas in points, anything in `egui` types | the app |

What needs the photo's pixels stays the engine's (where a spot's source is
found, where a pupil is). Positions and sizes cross the boundary as
`[f32; 2]` and `[f32; 4]`, as `model::image_frame` takes them. `Space`, a
brush mask's long-edge units, moves from the engine to `model::image_frame`
before the tool that uses it moves.

The second row is the part of this plan most likely to be shaped by one
toolkit. It is built tool by tool, when a second front end needs the tool,
and not ahead of that.

### The session's consistency is its own

The sequences the editor performs become session operations, so a second
front end cannot get their order wrong:

- **Opening a stored edit.** The recipe, its History, then clean; or
  protected, in each of the four cases the editor protects today: an edit
  that can't be read, a History that can't be read or is a later release's,
  a mask whose raster is missing or the wrong size
  (`mask_assets::ensure_shaped`), and an AI Remove patch that is
  (`ensure_patches`).
- **A save, in two phases.** Starting one counts a held gesture as changed
  and takes the snapshot; finishing one records the outcome and, when
  nothing changed meanwhile, ends the gesture's step. The save before
  navigation is the two phases around a write on the calling thread;
  autosave and the save with a deadline are the same two around the app's
  thread. A save that can't start is a failed one: a file that couldn't be
  identified when it opened, or no saver thread as the app closes. A photo
  that belongs to no catalog has nothing to save, and that is not a failure.
- **Whether autosave may run now.**
- **The save state's other transitions, each by name**: a failed save tried
  again at once after a relink; what was analysed from the photo stored in
  every History state, to be saved without a step; a starting edit that
  arrives after the photo opened, settled as clean; unsaved changes
  discarded. An Undo or Redo from the shared log marks the edit changed
  after `restore` or `set` (`src/app/shell/undo.rs:546`), and that mark does
  real work: `restore` compares the recipe with the log's target before it
  moves, and misses a move between two History states with equal settings
  and a state analysis brought up to date since. `restore` and `set` take
  the mark over by comparing what they did.
- **One way to set the settings as a whole as a named step**, right inside a
  frame and outside one, counting a change of masks. Today the caller must
  know which of `name_next_step` with `replace`, or `change`, fits where it
  stands, and the wrong one records two steps or none.
- **What is saved with the edit is the session's**: the export options are
  written in the same transaction and move the same save state.
- **Nothing outside the session sets its save state.** After the above,
  `save_state_mut` is removed.
- **A frame belongs to one load of one photo**, identified uniquely in the
  process, so a frame finished against the next photo's session records
  nothing.

### One crate for what is downloaded on request

`rawmakase-model-install` becomes `rawmakase-install`: what RAWmakase
fetches when the user asks, checked and put in place in a folder its caller
names. It takes the model's record of a pinned file and depends on the
model alone. Lensfun's installer moves in beside the model packs' as a
second module, since it is no part of the engine and can't go into the model
or interop either: an HTTPS client brings `ring`, built from C, into trees
decoding shares.

The two modules share what is the same in both: the lock file and how it is
taken, how the HTTP client is built, and the bounded read. What differs
stays each one's own: Lensfun is HTTPS only, since the transport is its one
check, where model packs are checked against pinned digests; Lensfun gives
up on the lock after five minutes, where a model pack waits for the other
install; and each keeps its progress and its errors.

### A model's contract is apart from what runs it

AI Denoise has this: its service takes a loader. `rawmakase-selection` gets
the same shape: for each model, the input it is fed and what its answers
mean are functions of their own, and the ONNX Runtime session is one caller
of them. RAWmakase needs this itself to run the selection models on Core ML.

What a saved mask records of its model and its input has one owner: the
model's identity in the selection crate's manifest, the input's beside
`SELECTION_INPUT_VERSION` in the engine as functions of plain values, with
the app turning an extension's request into them. The texts are pinned by
tests before the move, since saved masks are matched by them.

### Platform checks say what they mean

A check is `target_vendor = "apple"` when it is about Apple's frameworks,
and `target_os = "macos"` only for what macOS alone has. Every platform
switch has an arm that compiles everywhere and fails honestly: a HEIC on a
system with no decoder is refused with a reason. With folders, fonts, the
runtime's place and the computer's name given to the context, most of the
desktop-only assumptions the crates have today leave them altogether.

### The engine names its GPU backends

`rawmakase-engine` gets a feature per wgpu backend it renders with (`metal`,
`vulkan`, `dx12`; not OpenGL, which the renderer refuses), none on by
default. The library's `gpu-*` features turn them on, and so does the app
for the ones it ships. `ctx.renderer()` is always there. Without a `gpu-*`
feature the context has no GPU to configure and its renderers are the
CPU's: there is no way to ask for a GPU that was not compiled.

### The app and the command line are its first users

A library only its guests use decays. So:

- The app builds one context at startup from its `environment` module, and
  its workers take libraries, caches and services from it. The rest of the
  app keeps using the crates directly where it needs more than the library
  shows; it is in the workspace and may.
- `render` and `render-batch` are written over the library once it has what
  they use, AI Denoise last. Their tests are its acceptance tests, and they
  are the headless binary's commands. `--tap`, which writes a stage of the
  pipeline, is RAWmakase's own tool and not a library's API: a tap run
  stays written on the engine from decode to file. The line of numbers a
  normal run prints comes from the decoded image's statistics.
- `examples/embed` is a crate that depends on the library and nothing else,
  as a stranger's would.

### RAWmakase does this work

The work is RAWmakase's own. Safelight's fork and copies are evidence of
what an embedder needs and a reference for how one solved it, not patches
to merge. Its part is to say whether a release lets it delete a copy, a
fork commit or a dependency on a crate under the library.

## What using it looks like

A sketch of the shape, not the names:

```rust
let ctx = Context::builder()
    .data_dirs([&app.data])
    .local_data_dir(&app.local)
    .cache(&app.cache, CacheLimits::default(), app.build_id())
    .build()?;

let photo = ctx.open(path, cancel)?;            // header, metadata, profiles that fit
let mut recipe = photo.starting_recipe()?;      // raw defaults, as the app opens it
photo.set(&mut recipe, Parameter::Temperature, 5200.)?; // white balance follows

let image = photo.decode(Decode::Full, cancel)?; // reads the file again each time
let pixels = ctx.render(&image, &recipe, Size::fit(2048), cancel)?;

ctx.export(&photo, &recipe, &ExportSettings::jpeg(90), out, cancel)?;
```

With `edit`, a session holds the recipe, History and what is to be saved.
With `catalog`, `ctx.catalog(path)` opens one, and a photo's edit comes from
it and is saved to it. `ctx.renderer()` makes a renderer for a view, on the
thread that will use it, on the GPU when a `gpu-*` feature is on.

## Proof

A plan can't show that a design is good; only building on it can. So the
design is tested from outside, early, by programs that are not RAWmakase,
against measures that pass or fail. "Elegant" stays a judgment: the measures
bound it.

### The API first, as a prototype

Before any crate is refactored, the library's API is written as a thin layer
over the crates as they are, on a branch, and the first reference app is
built on it. The prototype still reads its folders from the environment
underneath, so it proves nothing about configuration or iPadOS; it proves
the shape. If an app built on it is awkward, the API changes while that
costs nothing. The refactor then makes the same API true, and how little the
reference apps' source has to change when it does is itself a measure.

### Reference apps

Three programs, each in `examples/` as a Cargo workspace of its own outside
the root one, depending on the library by path as a stranger's would:

- **`examples/embed`**: Rust, no interface, tested on the color-chart DNGs.
- **`examples/electron`**: a single-view editor in Electron. Open a photo;
  Exposure, Contrast, Highlights, Shadows, Whites, Blacks, Temp and Tint; a
  point tone curve; Export; Undo and Redo once the library has `edit`. A
  Node addon over the library. The photo, its recipe and the renderer stay
  on the Rust side; what crosses is small: a parameter's id and value, a
  curve point added, moved or removed, the controls' current values and
  ranges back as JSON, and pixels as an RGBA buffer. A whole recipe is not
  sent on every change: it carries its profile's tables.
- **`examples/swift`**: the same editor in SwiftUI over UniFFI bindings,
  built for macOS and run in the iOS simulator.

They share nothing but the library. A helper both would want is an API the
library is missing.

### What each must pass

1. **One dependency.** Its manifest names one RAWmakase crate. No file is
   copied from `src/`, and nothing is forked.
2. **No environment, no `unsafe`.** It sets no environment variable and
   writes no `unsafe`, and denies it. What its binding generator expands to
   is the generator's.
3. **Small glue.** The Rust between the library and the interface is
   counted. The count after the prototype becomes a budget, and growing
   past it means the library is missing something.
4. **The desktop's pixels.** The app and `rawmakase render` export the
   same photo with the same saved recipe to a lossless format, in sRGB,
   each with an empty data folder of its own, and the decoded pixels are
   equal. Files are not compared: their metadata may differ.
5. **Live editing.** The time from a slider's change to pixels in the app
   is measured beside the same render called from Rust, returning pixels,
   on the same computer, photo, frame size and backend: the CPU from stage
   1, the GPU with its readback from stage 2. The render is also timed from
   inside the app, and each result records the frame's size. The prototype
   found the difference was not all the binding's: the same render took
   15–20% longer inside Electron's main process than in a plain Rust
   process (about a tenth longer in plain Node), and the reason was not found.
   When the measure is taken again, that is to be explained. A
   draft shows first when the full frame is slow. `examples/slider_benchmark`
   is not the baseline: it times a frame presented on the GPU, which an app
   that needs pixels doesn't get.
6. **Scheduling and cancellation.** Dragging fast queues nothing: the
   newest edit replaces the one waiting, and the running render finishes
   and shows, as on the desktop. A render cancelled because the photo or
   the view changed returns the cancelled kind and is never shown as an
   error.
7. **History across the binding.** Undo, Redo and whether there are unsaved
   changes come from the edit session.
8. **Features.** Its tree has no SQLite, no HTTP client and no ONNX Runtime
   binding.
9. **iOS.** The Swift app's test renders one photo in the iOS simulator in
   CI, which shows it runs there and not only that it compiles.

A measure is taken from the stage that makes it possible: 1 to 4, 6 and 8,
and 5 on the CPU, from stage 1; 5 on the GPU and 9 from stage 2; 7 from
stage 3.

### Three more tests

- **A cold start.** A programmer or an agent given the guide and the
  rustdoc, and nothing of the repository, writes the Electron app's Rust
  side. Needing to read a crate under the library is a failure of the API
  or its guide. It is cheap to repeat at every stage.
- **The app itself.** The desktop builds its context through the library
  and `render` is written over it, as above.
- **Safelight.** The count of its fork's commits, its copies and its
  dependencies on crates under the library, kept in the guide. The plan is
  done for an embedder when its author can bring each to zero, or one for
  the selection crate.

## Work, in order

The steps are commits in one pull request, each a commit or a short run of
them, in this order. Moves are mechanical
and separate from any change in behavior. Stage 0 proves the API's shape
before anything is refactored; stage 1 is a slice through the whole design,
so the design is proved on one path before it is spread.

### 0. The prototype

- The library's API over the crates as they are, and `examples/electron` on
  it, on a branch that is not merged: sliders, white balance, the tone
  curve and Export, on the CPU, without Undo and Redo, since the edit
  session is still in the app crate and nothing is copied out of it. The
  desktop's draft rule is in the app crate too, so the prototype shows a
  draft by a rule of its own, a fixed reduced size while a control is
  held: enough to try the shape of asking for a draft and then the frame,
  and replaced by the real rule in step 5. It reports measures 1 and 3 to
  6, the part of 2 that is about `unsafe`, and what the API lacked. Until
  step 2 a cancelled render is told apart by asking the `Cancel` it was
  given, as the app's worker does, and until step 4 measure 4's empty data
  folder is two variables set by the test (`RAWMAKASE_DATA_DIR` and
  `XDG_DATA_HOME`, since the older Linux folder is searched too). The plan
  is corrected from what the prototype finds before stage 1 starts.

  Built (2026-11-25), on a branch not merged. Measures 1, 2's `unsafe`
  part, 4 and 6 passed; 3 counted 679 lines of Rust glue and 152 of
  JavaScript that talks to the addon; 5 was taken (a 0.7 MP chart: 38 ms
  from a slider to pixels in the page, a 480-pixel draft 10 ms; a 24 MP
  photo at 1431×954: 85 and 10 ms). What it found is written into this
  plan: `open` takes a `Cancel`; decoding consumes the opened file and
  export needs it; the raw defaults are a value of the context; the
  parameters' names, reading and controls, the curve's results, the
  mailbox and stale-frame rules, RGBA, the sampled curve and the output
  size in step 5; the "exists" kind; the build id first in step 4; the
  decode's statistics; `deny` for binding crates; and why measure 5
  records the render's time inside the app.

### 1. One photo, start to finish

1. The guard: the check of environment reads and programs started, with
   today's list; `forbid(unsafe_code)` in the crates that have none, the
   engine as its uniforms allow; `docs/architecture.md` gains the rules (the
   app crate holds what needs a window; a library crate reads no
   environment and starts no program).

   Built. The check found 83 sites in 45 files and kinds; counting the
   ambient folders, which the plan's first list of patterns left out, is
   what makes it see the sixty-odd `data_dir()` calls. Twenty-one crate
   roots gained `forbid(unsafe_code)`, the engine's among them, and the
   extensions', the tools' and the remote worker's binaries.
2. Typed causes: `Cancelled` in `rawmakase-cancel`, an invalid recipe or
   setting, a file's identity changed, a missing profile or preset. Each
   has today's text as its message.

   Built. `rawmakase_cancel::Cancelled`, made by `Cancel::check` with the
   work's own wording, at every point decoding, the engine, export and AI
   Denoise's service stopped with a text; the RAW adapter turns the
   decoders' own `Error::Cancelled` into it, since nothing above
   `rawmakase-decode` may name `rawmakase-raw`. `rawmakase_model::error`
   holds `Invalid`, `Missing`, `Changed` and `Exists`, each printing what
   the error it replaces printed (its message, or the error it was made
   from with that error's causes), so a chain reads the same: an invalid
   recipe (`Recipe::validate`, its local edits too), export output or
   options (every refusal of `ExportSettings` and `ExportOptions`, typed
   where it is made, and an HDR export of an edit with HDR off); a missing camera profile named by
   settings and a missing saved naming preset; a photo's file changed since
   it was shown (export) or since its edit was saved (the catalog, and
   reading a stored edit); an export's destination and a new catalog that
   already exist. AI Denoise's failures reach export and the command line
   as one message, "AI Denoise: " and the chain, as before, except a
   cancelled run, which stays `Cancelled`. Merge to HDR, inference and the installer keep the
   cancelled and exists variants of their own error types, which the
   library maps where it shows them. An enhanced profile that is "missing
   or unsupported" stays untyped, since it may be either.
3. `rawmakase-core` with the context and its error type. The context holds
   the configuration's values and the two things that can be made from
   values today, the decode and AI Denoise caches from a folder and a
   limit; the rest of one photo's path joins it in step 4 and the optional
   parts in their own steps, as each is made explicit.
   The app's `environment` module; the app building the context at startup
   and its workers holding it. The guide (`docs/embedding.md`) and the
   listing of public items start here.

   Built. The context holds the cache folder, its limits and its build id,
   and makes the decode and AI Denoise caches from them; the other values
   join with the change that first reads them, so the context never offers
   a setting the crates under it still look up for themselves (the
   prototype's context recorded folders it couldn't honor). The build id is
   required and stored, and keys the caches from step 4's first change. The app's
   `Editor` holds the context and reads its decode cache from it; the
   command line's AI Denoise cache comes from the same `environment`
   module. The listing is read from rustdoc's JSON with a pinned nightly
   (`scripts/public-api.py`, tested on fixture crates beside it), in a CI
   job of its own, and follows re-exports into the workspace's crates,
   naming a re-exported type by the library's path. The crates' cache
   handles the app's workers need are in a `workspace` module behind a
   feature of that name, which only the app turns on, so they are no part
   of the surface; the library's own conversion from the crates' errors is
   private to it, so `anyhow` isn't either. The error kinds also read the
   crates' existing typed causes (a missing mask raster, patch or catalog
   asset; AI Denoise's `Unsupported`), keep a cancelled job cancelled
   whatever else its chain says, and keep their own kind through an
   `anyhow` error and back; "already exists" from the system is not taken
   for an existing destination, which is typed where it is refused (a
   destination taken between a check and a write among them). The map's
   tiles take the context's cache folder too.
4. Explicit inputs for the path one photo takes, one change each, each call
   site passing from the context. First the caches' build id and the
   default folder they return to: the key comes from `current_exe`
   (`decode_cache::build_identity`), which in an Electron app is Electron's
   own binary, so every build of an addon shares one key and reads another
   build's results. Then the lens profile library into
   `photo::open`; the camera profile library, with its header cache its
   own, with loading a look and the profile manager's and the community
   catalogue's lookups; the preset library and its layout file; the
   Lensfun database, through a handle that follows its folder; Camera
   Look's cache; the watermark folder; the fonts, with their cache the context's; a
   downloaded profile staged beside its destination; an export setting's
   default destination and the keyword sets' Lightroom folder, the last
   uses of `directories`.

   Built so far: the caches' build id. A decode and an AI Denoise cache are
   made with the build they key entries by and the folder they return to
   when a chosen folder is given up (`set_folder(None)`), both from the
   context; the keys keep their form, and the desktop passes the
   executable's identity as before, so its entries are still read. Camera
   Look's fit cache still keys by the executable until its own change
   below. The ambient check counts `build_identity()` calls now, and no
   longer counts a function's definition as a call to it.

   The lens profile library: `lens::lcp::Profiles`, a handle on the
   profiles in a set of folders that a clone shares, made by the context
   from its data folders (`ContextBuilder::data_dirs`), replaces the
   process-wide one, and every opener takes it (`photo::open`,
   `open_source`, `header_metadata`, `lens_profiles`, the editable
   preview, Camera Look's fit and default, the developer tools). The
   app's workers take it from the context when they are made or a job is
   captured; the Library gets it beside its AI Denoise cache, before
   anything is shown; the command line from the `environment` module's
   context. Opening a photo also names a lens a RAW gives only by its id
   (Pentax's, an older Nikon's) after the profile of that id, so a photo's
   info is read with the context's profiles too: the catalog session holds
   them, given with the Library's, and so do the naming and import readers.
   Merge to HDR, AI Denoise and its benchmark open photos with no imported
   profiles: they read the sensor data and the camera's EXIF, and take a
   lens name from the decoder only when a frame's EXIF can't be read.
5. The library's operations: opening, with a `Cancel`, since opening a
   rendered file decodes it, and a RAW the decoders don't support refused
   as unsupported (the RAW adapter turns `rawmakase_raw`'s `Unsupported`
   and `NotRaw` into a typed cause, as it does its `Cancelled`); decoding
   and its statistics, the photo handle
   opening its file again for each decode; starting recipes from the
   context's raw defaults; presets with measures, Auto; changing a recipe
   by parameter, with what that needs of the model: a name for each
   parameter, its value read through `&Recipe` (today `ParameterId::ALL`
   is `cfg(test)`, and `value_mut` is the only way to a value), and a
   control's description, its range, step, scale, value and text, with
   Temp's reciprocal mapping moved from `src/app/ui/widgets.rs` to the
   model and a step for the sliders that state none; the curve's methods
   saying what they did (`ToneCurve::insert` returns an index even when
   the curve is full at 32 points and nothing was inserted, and
   `move_point` ignores an index out of range); the mailbox and the
   stale-frame rules, RGBA frames, the curve sampled for drawing and a
   decoded image's output size at a view's size; a renderer on the CPU (the whole photo, a region, and the
   draft rule, moved from the app's worker to the engine and kept per
   view, since the app's worker renders several views with one renderer,
   and told whether a frame still has finishing to do after it, which the
   rule counts today only for a frame presented on the GPU), and with
   `export` writing
   files, HDR masters and gain maps among them, from the photo and with the
   destination as an argument (interop's `ExportSettings` is the Export
   dialog's type, and the library's call reads no default destination from
   it, so it needs nothing of step 4's last change); `examples/embed` tested on the
   color-chart DNGs in `tests/corpus/charts`, and `examples/electron` moved
   from the prototype onto it. `scripts/crate-closures.sh` keeps the root
   crate, the GUI crates (`emath` and `ecolor` named too), rfd and the
   extension host out of the library's tree and, read from each example's
   own workspace, out of theirs. Each example declares a workspace of its
   own, commits its own `Cargo.lock`, and is formatted and linted in a CI
   job of the examples' own, apart from the app's, where they also build,
   test and take their measures. The script's search for `rawmakase_raw`
   reads tracked files, so an example's build and Node folders are not
   searched: today it walks `examples/` on disk, `target/` and
   `node_modules/` with it, and this step, the first to add an example
   workspace, fixes that.

### 2. Platforms and the GPU

6. Apple platforms: decoding's ImageIO table and the four switches in
   `raster/heic.rs`; AI Denoise's preferred provider.
7. The engine's backend features and the GPU's options as values, the
   library's `gpu-*` features, renderers made by the context, and
   `examples/embed` and `examples/electron` turning on their target's. A
   renderer gives a frame
   either as a texture on the app's own device, to present, or as pixels.
   Measure 5 decides whether pixels need a second path, finished on the
   GPU and read back as 8-bit, in place of today's float readback and CPU
   finish.
8. `examples/swift`, and CI on the macOS runner: the library checked for
   the device target, `aarch64-apple-ios`, with the features it has then,
   each feature added later joining the check; and built for the
   simulator's, `aarch64-apple-ios-sim`, for the Swift app's test, which
   renders one photo in the iOS simulator. Nothing runs on a device. When
   AI Denoise joins (step 17), the simulator is not taken for a device
   with a Neural Engine.

### 3. Editing

9. `edit_session` moves to `rawmakase-editing`, with its tests.
10. The session operations, one change each: opening a stored edit with its
    four protections; the two-phase save, under the save before navigation,
    autosave and the save with a deadline; autosave's readiness; the four
    other transitions, with `restore` and `set` marking what they changed;
    export options into the session, after which `save_state_mut` is
    removed; the whole-recipe step, taken up by Paste, Reset and Before and
    After; the frame's load.
11. Before and After's settings rules to `rawmakase-editing`, as a function
    from Before and After to what each becomes and the step's name, applied
    with the whole-recipe step.
12. The library's `edit` feature, and the examples editing through a
    session: Undo, Redo and unsaved changes in the two apps.

### 4. The optional parts

Each is made explicit, then shown through the library.

13. Catalogs: the file readers passed at open; the thumbnail budget in the
    preview cache; this computer's id from the local data folder and its
    name from the context, with no program started; the folders file
    operations protect (in planning and in `holds_own`), and the legacy
    sidecar's, passed in; a Lightroom import's snapshot made beside its
    destination; the catalog's
    own files in the data folder (new-photo and import defaults, the
    preview cache's path) given their folder; the kinds of the catalog's
    other failures (a folder sync or Find on Disk stopped, an Export as
    Catalog over an existing one, a legacy sidecar's file changed), which
    are still text; the library's catalog handle,
    which registers the mask reader for the catalog an app shows. The
    Library's sort moves to `rawmakase-catalog`, with the label order.
14. A photo's info and capture time to `rawmakase-decode`, with whether to
    read in pieces passed in.
15. Export's rest: a watermark by name; export settings read and written at
    a path and to and from the saved JSON, both through the code that keeps
    what a later release wrote; rename settings and saved templates given
    their folder; the context's own list of exports in flight.
16. Importing profiles and presets from files: the three importers leave
    `src/app/presets/bulk_import.rs` for `rawmakase-interop` with their
    destinations passed in and progress through a callback;
    `lcp::import_into` becomes public; saved curves, favorites and user
    presets given their folder; the editor keeps the task.
17. AI Denoise, in several changes. The pinned-file record and the receipt
    to the model's `storage`, used by the installer, AI Denoise and the AI
    masks extension. `rawmakase-denoise` without the installer: its
    `install` and what it re-exports of the installer go to the app, which
    has both. Its benchmark to the app's binary. The runtime's path a
    value, recorded by inference: looking for the runtime (the variable,
    the folders beside the executable, the file names) moves to the
    programs that do it today, which are the app, the AI masks extension
    and the remote worker, and the selection crate's loaders take the path
    they are given. AI Denoise's cache keys from the context's build id,
    its photos opened with the context's lens profiles, its model and
    cache folders from the context; its service owned by the context and
    stopped with a deadline. Its failures keep their kinds: today the
    service records a failed run as text (`Outcome::Failed(String)`), and
    a file changed under a run is an untyped error, so both become the
    typed causes of step 2 here.
18. The installer crate, in two changes: `rawmakase-model-install` becomes
    `rawmakase-install` over the model's file record (the rename reaches
    the root manifest, the AI masks extension, `scripts/deps-crates.txt`,
    the `Makefile`, CI, `docs/architecture.md`, `docs/code-map.md` and the
    headless plan; `scripts/crate-closures.sh` gains its rule, which
    forbids inference); then
    `lensfun_install` moves in, with its test archives left in
    `tests/lensfun/` where the app's tests read them too.
19. Merge to HDR through the library.
20. `render` and `render-batch` over the library, in place, with their
    tests. A catalog's edit and a legacy sidecar's are passed in by the
    binary, as raw defaults are today; a `--tap` run stays on the engine.

### 5. The selection models

21. The selection crate's seam and documented surface, with the re-exports
    its API needs; the model's and the input's identity to their owners,
    after tests that pin today's texts; the library taking coverage as a
    mask's raster with its source.

### 6. Finishing

22. The last ambient forms leave the library's crates, the tone rule's dump
    becomes an argument, `rawmakase-model` drops `rawmakase-protocol`, and
    the check's list is empty.
23. Tool and panel math, when a second front end needs a tool, in the order
    it asks: first `Space` to `model::image_frame`; a rule that is inline
    today becomes a named function with a test where it is, then moves.
24. The engine without a GPU, in two changes. First the types the CPU path
    shares with the GPU port are separated (`render_preview` builds
    `develop::gpu::Finish`; `quality::Output` carries GPU frames) and the
    preview renderer's GPU entry points are put behind an engine feature.
    Then wgpu and what comes with it become optional in the engine, the
    library's `gpu-*` features turn the feature on, and
    `scripts/crate-closures.sh` checks a tree built without it, which its
    `--all-features` runs can't.

## Open

- **Names.** `rawmakase-core` and `Context` are working names. The library
  could take the bare name `rawmakase` if the app's package gave it up,
  which reaches packaging, CI and every `-p rawmakase`.
- **The mask store as a value.** It is the one piece of shared state a
  server would want per job, and making it one changes every render entry
  point. Not needed for one app in one process.
- **The default system font.** The plan has the app pass it. Whether the
  library should also offer, as a function an app calls, the lookup the
  desktop does today is undecided; called from inside, it would break the
  rule.
- **Linear raw beyond 100 megapixels at half size** (the fork's `3210cfe3`,
  `283a9b68`): a decoding change to decide on its own merits.
- **The catalog session** (`src/catalog_session/`): no window in it, but it
  depends on `platform` and nobody has asked for it.
- **Memory limits** a caller can't set (the stage cache's 512 MB, the mask
  store's 256 MB, the kept AI Remove patches' 192 MiB): they become the
  context's when measured to matter.

## How each step is checked

- The moves: the tests move with the code, `scripts/deps.py check` and
  `scripts/crate-closures.sh` gain the new crates' rules, and the app's
  tests pass unchanged. No rendering reference is touched.
- Explicit inputs: the desktop renders, exports and lists what it did
  before, by its existing tests, and finds assets in both of its data
  folders; two contexts with two data folders in one process each get their
  own profiles, presets, lens corrections and fonts; a Lensfun database
  installed while a context lives is seen by it.
- Caches: the desktop's entries made before the change are still read; two
  contexts with one cache folder and two build ids don't read each other's.
- The library: the example crate's tests; its public listing unchanged
  unless the change says so; a cancelled decode, render and export each
  return the cancelled kind, and an invalid recipe, a changed file and a
  missing profile theirs; `render` and `render-batch`'s tests through it,
  printing what they print today.
- Features: CI builds the library with none, with each alone and with all,
  and checks that a build without `catalog` has no SQLite in its tree,
  without `install` no HTTP client, and without `ai-denoise` no ONNX
  Runtime binding. The app's tree has no selection crate, as today.
- The session operations, at the session's own boundary: each of the four
  protections on opening; a save that fails; an edit made between a save's
  two phases; a held gesture through a save; export options changed alone;
  a frame finished after navigation, against a kept session and a new one;
  a whole-recipe step inside a frame and outside one, each one step and one
  Undo, with the mask count moved when the masks changed; an Undo to a
  History state whose settings equal the ones shown, and one to a state
  analysis changed since, each left to be saved.
- AI Denoise and the installers: a pack installed by an earlier release is
  still found installed; both installers' tests as they are and the AI
  masks extension's setup tests; a Lensfun download from a plain HTTP
  address is still refused, and its lock still gives up after its wait; a
  context that names a second ONNX Runtime is refused.
- Export settings: a file a later release wrote, read and written back
  through both forms, unchanged.
- Saved masks' identities: the pinned texts before and after.
- Apple platforms: the iOS build and the simulator test of step 8. What
  happens on a device is Safelight's to report.
- The reference apps: the measures under "Proof", each taken in CI from
  the stage that makes it possible, and the cold start repeated at the end
  of each stage.

## Beside the headless plan

The [headless plan](headless-plan.md) builds `rawmakase-cli`, which works on
files and never links the catalog. With this plan it is a binary over the
library without the `catalog` feature: step 20 writes its two main commands
that way, steps 16 and 18 take `import-profiles` and `lensfun` out of the
app crate, and the `environment` module is where it reads
`RAWMAKASE_DATA_DIR` and the rest. Moving the commands into a crate of
their own, the older-glibc builds and the server stay that plan's.

A context is also what the server's
[captured jobs](headless-plan.md#2-captured-jobs) capture from: once
nothing is read from the environment, a job's inputs are the values a
context holds.

That plan made removing wgpu from a headless build a question to measure
first. Step 24 answers it for the library's sake, and the binary gets it
with it.
