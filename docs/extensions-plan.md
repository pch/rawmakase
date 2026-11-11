# Extensions: design plan

Status: Accepted (2026-11-09); phases 1 and 2 built. Phase 2 built the
manifest and its schema, `.rmext` packages, installation from a file and from
GitHub, `extensions.json`, bundled discovery, the extension protocol, the SDK
with its mock host, the host's process supervision primitives (process group or
job object, channel, deadlines, stop), `cargo rawmakase` and
`rawmakase extensions` ([reference](extensions/overview.md)); supervision in
the app (`app::work`), Preferences > Extensions, commands, settings, panels,
reads, proposals and target tokens in the app; control adapters, the app's own
and externally launched ones; and moved MCP and MIDI out as bundled
extensions. This page is the
design we agreed on before implementation; once a part is built, its reference
moves to `docs/extensions/` and this page records only what is still open.

## Why

RAWmakase does what its maintainer needs. Requests for features the maintainer
won't use (other catalog importers, tethering, AI tools, film looks) should be
answerable with "write an extension", and the existing niche features should be
able to leave the core the same way. The goal is a **small, documented surface**
that lets other people add imports, Library tools, editing tools, filters and
export steps, in Rust, at the app's speed, without the app having to know about
any of them.

## Decisions taken

| Question | Decision |
| --- | --- |
| Who writes extensions | People and AI agents, in Rust, building from source |
| Who installs them | Anyone: **prebuilt packages only**; nobody builds anything to run one |
| Trust | Full trust, like Blender add-ons: an extension runs with the user's permissions. Installing one that isn't bundled warns about this |
| Distribution | Manual install from a GitHub repository's releases or a package file. An official catalogue comes later, once there are third-party extensions to list. No automatic updates: Preferences checks and updates on request |
| Compatibility | None promised before 1.0. Each package states the RAWmakase versions it supports and is refused outside them |
| Settings in XMP | Never. Extension data is RAWmakase-only |
| Official extensions | MIDI, MCP and AI masks move out of the core into extensions bundled with the app, installed and enabled; each one can be disabled. Lens corrections data and community profiles stay as they are |
| Missing extension | The photo renders **without** its effects, says which extension to install, and offers to remove the effects (one History step) |
| Disabled extension | Its filters **still render**, with a message that they come from a disabled extension; its commands, panels and program are off |
| Export with a missing filter | Exports without the effect, after warning which photos will look different and why |
| A filter update changes its look | Edits render with the installed version, as the core engine does. A filter whose meaning changes gets a new id |
| UI | Drawn by the app from a fixed set of controls; extensions never draw egui themselves |
| Viewer tools | The app provides point, box, combined click-and-box prompts and brush input; an extension asks for one kind and receives the result |
| AI upscaling | Not planned; the design leaves room for it (see [Future](#future-derived-photos)) |
| Scope of the first version | Only what moves MIDI, MCP and AI masks out ([Phases](#phases)). Every other contribution is designed here but built only when a real extension needs it ([Designed, not scheduled](#designed-not-scheduled)); nothing built first may close off room for it |
| Extensions RAWmakase writes | Only the extracted features. Film grain, Auto modes, Capture One import, tethering, watched folders and object removal are examples of what others could write, not planned work |
| Lightroom import | Stays in the core for now. Its edits are converted once, at import (a separate change, planned on its own). Moving the importer into an extension is a follow-up the importer contract leaves room for |
| Other importers | One-time imports, such as Capture One and Lightroom CC: photos, metadata and edits converted to RAWmakase settings at import, with no ongoing link to the source |
| Filters before the scene tone stage, filters limited by a mask | Deferred until designs of their own; the filter design leaves room for both ([Filters](#filters)) |

## The model in one paragraph

An **extension** is a package with one manifest, `rawmakase-extension.toml`. The
manifest lists **contributions**: commands, key bindings, settings, panels,
filters, mask generators, retouch generators, importers, photo sources, export
destinations, control adapters and content (presets, LUTs, profiles). Each
contribution is backed by one of three things, and an extension may use any mix
of them:

| Backing | What it is | Runs | Used by |
| --- | --- | --- | --- |
| **Files** | Data in the package | Nowhere; the app reads it | Presets, LUTs, camera and lens profiles, key maps, watermark assets |
| **Program** | One native executable in the package | As its own process, started when needed, talking to the app over the [extension protocol](#the-extension-protocol); pixels go through shared memory | Commands, panels, importers, sources, generators, export destinations, control adapters |
| **Kernels** | A WGSL function and a WASM module | Inside the app's renderer: WGSL on the GPU preview, WASM on the CPU (export and the CPU preview) | Filters |

Why the split: a program can use any crate (ONNX, camera SDKs, SQLite readers),
crash without taking unsaved edits down, and is insulated from the app's
internals, so its interface is the protocol, not the app's Rust API. A filter
can't be a program: it runs on every preview frame, and a round trip from the GPU
to another process per slider move would make it the slowest thing in Develop.
Kernels are pure functions the app schedules itself, so they can come close to
the built-in stages' speed (a goal, measured before the filter
contract is fixed) and need no trust in their memory safety.

The rule that keeps saved photos safe: **only filters make a photo depend on an
extension.** Everything else an extension does ends as an ordinary value the app
validates and stores itself: settings, catalog rows, a mask raster, a retouch
patch. Those keep rendering after the extension is gone.

## Manifest

The manifest is TOML, validated against a published JSON Schema
(`docs/extensions/manifest.schema.json`), and the app reads only the manifest at
startup. The full reference is generated from the schema. A complete example:

```toml
[extension]
id = "dev.jane.analog"             # reverse-DNS, permanent; edits store it
name = "Analog Toolkit"
version = "1.2.0"                  # semver of the extension
rawmakase = ">=0.6, <0.7"          # app versions it supports; refused outside
api = "0.3"                        # extension API it was built against
authors = ["Jane Doe"]
license = "MIT"
repository = "https://github.com/jane/rawmakase-analog"
description = "Film grain, halation and film-stock Auto looks."
icon = "icon.png"

[access]                           # declared and shown; not enforced
network = false
files = "photos"                   # photos | any
devices = []                       # e.g. ["usb", "midi", "camera"]

[program]
path = "bin/analog"                # per-platform package, see Packages
start = "on-demand"                # on-demand | with-app

# --- settings shown on the extension's page in Preferences ---
[[setting]]
id = "default-stock"
control = { choice = ["Tri-X", "HP5", "Portra 400"] }
default = "Tri-X"

# --- a filter: a panel in Develop and a stage in the renderer ---
[[filter]]
id = "grain"                       # saved as dev.jane.analog/grain
name = "Analog Grain"
stage = "finish"                   # display | finish (scene: later, see Filters)
kind = "neighbourhood"             # point | neighbourhood
reach = 6                          # pixels read around each output pixel, at 100%
hdr = false                        # runs on HDR masters (values above white)
params-version = 2
gpu = "filters/grain.wgsl"
cpu = "filters/grain.wasm"
[filter.params]
amount = { control = { slider = [0, 100] }, default = 0, unit = "%" }
size   = { control = { slider = [1, 50] }, default = 25 }
stock  = { control = { choice = ["Tri-X", "HP5", "Portra 400"] }, default = "Tri-X" }

# --- a command: menus, a key, optional viewer input ---
[[command]]
id = "auto-film-look"
title = "Auto Film Look"
places = ["library.photo-menu", "library.auto", "develop.auto"]
key = "Ctrl+Alt+F"                 # default; the user's key map overrides it
applies-to = "selection"           # none | photo | selection
input = "none"                     # none | point | points | box | prompt | brush

# --- plain content ---
[[presets]]
path = "presets/"
[[luts]]
path = "luts/"
```

The other contribution tables (`[[mask-generator]]`, `[[retouch-generator]]`,
`[[importer]]`, `[[source]]`, `[[export-destination]]`, `[[control-adapter]]`,
`[[panel]]`, `[[keymap]]`, `[[profiles]]`) follow the same pattern and are listed
under [Contributions](#contributions). Unknown keys are an error in the manifest
(a typo must not silently disable a feature); the schema version is `api`.

Ids: an extension id is reverse-DNS and never changes; official extensions use
`rawmakase.*`. Contribution ids are local to their extension and saved as
`<extension>/<contribution>`.

## Packages and distribution

### Package format

A package is a zip named `<id>-<version>-<target>.rmext`, where `<target>` is one
of the targets RAWmakase releases for (for example `linux-x86_64`,
`macos-aarch64`, `windows-x86_64`), or `any` when the extension has no program.
It holds the manifest, the program for that target, kernels, content and a
`SHA256SUMS` listing every file. Kernels and content are identical across
targets.

### Where they come from

- **Bundled.** The official extensions live in this repository under
  `extensions/`, are built by the release workflow and ship inside the app. They
  update with the app, can be disabled and can't be uninstalled.
- **Official catalogue (later).** A reviewed index in a repository of its own,
  as `data/camera-profiles` is today: for each extension its id, repository, and
  per version and target the asset URL and SHA-256. The app verifies the
  download against the pinned digest, as the community profile catalogue does.
  It is built once there are third-party extensions worth listing; until then
  the two sources below are the only ones.
- **GitHub repository.** The user pastes a repository URL. The app reads its
  releases, picks the asset for this target whose manifest's `rawmakase` range
  includes this version, shows the manifest and the trust warning, then installs.
  These are marked "Not reviewed".
- **Package file.** For development and offline installs.

`cargo rawmakase release` and a GitHub Actions template build every target's
package and attach them to a release, so authors don't hand-assemble zips.

### Installation, state and updates

- Installed packages unpack to `<data dir>/extensions/<id>/<version>/`, published
  atomically (unpacked into a hidden folder, verified, renamed); the previous
  version is kept until no running job uses it.
- `<data dir>/extensions.json` records which versions are installed, which is
  active, enabled state and where each came from. It is written atomically and
  keeps unknown fields.
- **Installing never runs code.** The program first starts when a contribution
  needs it.
- "Check for updates" reads the GitHub releases of installed extensions (and
  the catalogue, once it exists) and lists newer compatible versions; "Update" installs one. An
  update whose manifest declares more access than the installed version says so.
- An extension whose `rawmakase` range excludes the running app is listed as
  incompatible and stays inactive; its filters behave as missing.

### Platform notes

- Programs are separate executables, so the app's own code signing and macOS
  library validation are unaffected. The app downloads packages itself, so no
  quarantine flag is set; the release template still ad-hoc signs macOS programs,
  which Apple silicon requires to run them. Notarizing is the author's choice.
- Kernels are data (WGSL text, WASM bytes), with nothing to sign.

## Preferences > Extensions

A new Preferences tab, beside Automation:

- The list: bundled, installed and incompatible extensions, with name, version,
  source (Bundled, Catalogue, GitHub, File), state (Enabled, Disabled,
  Incompatible, Failed) and a badge when an update was found.
- A detail page per extension: description, author, repository link, declared
  access, the contributions it adds, the photos in this catalog that use its
  filters, its settings (drawn from `[[setting]]`), the program's last errors and
  a log file link.
- Enable, Disable, Uninstall, Check for Updates, Update.
- Install from GitHub…, Install from File…, and Browse Catalogue once the
  catalogue exists.
- Turning on developer mode adds "Load from Folder…", which runs an unpacked
  extension and reloads it on change (`cargo rawmakase dev` uses it).

The same model lives below the UI in the host crate, so the command line can
list, install and remove extensions (`rawmakase extensions …`).

## Commands, menus and key bindings

Customisable key bindings are a core feature that extensions plug into, not an
extension feature.

- **Command registry.** Every user action becomes a named command with an id,
  title, the contexts it is available in, and its default keys. Built-in commands
  come from the existing action-name table that automation and MIDI already share
  (`app::shell::commands`), extended to the actions that today are matched on
  keys directly. Extension commands join it as `<extension>/<command>`.
- **Key map.** `<data dir>/keymap.toml` holds the user's overrides only, so new
  defaults still arrive. Contexts (Library grid, Loupe, Develop, a text field
  having focus, a modal) decide precedence; keys a text field consumes are never
  stolen. Conflicts are shown, never resolved silently.
- **Preferences > Shortcuts.** Search, rebind, reset, and see conflicts. The
  shortcut reference card (`app::shell::shortcuts`, a static table today) is
  generated from the registry.
- **Places.** A command's `places` puts it into fixed, named slots: the Library's
  photo menu and toolbar, the Library's and Develop's Auto menus, Develop's tool
  strip, File > Import, File > Export, and the extension's own menu under
  Extensions. The app owns availability (a photo must be open, a selection must
  exist, a save must finish first) and refuses a command before sending it.
- Automation and MCP see the same registry, so an extension command is also a
  `rawmakase control action dev.jane.analog/auto-film-look`.

## Host-drawn UI

Extensions describe controls; the app draws them with its own widgets, theme,
undo grouping and accessibility. The vocabulary:

| Control | Value |
| --- | --- |
| `slider` | number in a range, with unit, step and optional centred zero |
| `angle` | degrees |
| `toggle` | bool |
| `choice` | one of a list (drop-down) |
| `segmented` | one of a short list (buttons) |
| `color` | sRGB colour, optional alpha |
| `curve` | points 0–1, as the Tone Curve's |
| `number`, `text` | free entry, validated |
| `file`, `folder` | a path chosen in the system dialog |
| `button` | runs a command |
| `label`, `help` | text |
| `progress` | read-only, driven by the program |
| `image` | read-only thumbnail the program supplies |

Controls are grouped in sections, can be shown or hidden by a simple condition on
another control's value, and are laid out in a single column like every
inspector panel. A `[[panel]]` puts such a group into Develop's right column, the
Library's right column, or an extension's own window. Panels backed by a program
send their values to it and receive updated values and messages back. Filter
panels need no program: their parameters live in the recipe.

**Viewer input.** A command with `input` makes the app enter a tool mode on the
viewer and collect one of these, in image coordinates:

- `point`: one click;
- `points`: positive and negative clicks;
- `box`: one dragged box;
- `prompt`: positive and negative clicks and an optional box, refined together
  and sent as one value, as Select Subject takes them today
  (`subject_mask::run_prompt` builds one prompt from both);
- `brush`: a coverage raster with the app's brush size, feather and flow.

The program receives the result when the user confirms, or, for `prompt`, after
each change, so the mask can update while the user refines it. This covers
Select Subject, object removal's brush and future box-based tools, without an
extension ever drawing on the viewer.

If this vocabulary proves too narrow, the next step is more control types, not
custom drawing.

## The extension protocol

What the app and a program agree on. It is a crate of its own,
`rawmakase-extension-protocol`, with no dependency on the app, so the SDK builds
without it, as `rawmakase-protocol` does for control clients today.

### Transport

- The app starts the program with its working directory in the package, a fresh
  temporary folder, and the message channel below.
- Messages are one JSON object per line, in both directions, so the protocol is
  readable in a log and documentable as JSON Schema. Lines are bounded (1 MiB);
  bulk data never travels in JSON.
- The message channel is a connected local socket the app creates before
  starting the program and hands it at launch, not stdin/stdout, because it must
  also carry shared-memory handles: on Linux and macOS a Unix socket pair, with
  handles passed as `SCM_RIGHTS` ancillary data; on Windows a named pipe, with
  handles duplicated into the program's process by the app (`DuplicateHandle`;
  the app holds the child's process handle). Stdin is closed; stdout and stderr
  go to the extension's log file.
- **Pixels and rasters** travel in shared memory, and **the app allocates every
  buffer**, in both directions: anonymous mappings (`memfd` on Linux, an
  unlinked `shm_open` object on macOS, an unnamed file mapping on Windows), so no
  named object can be left behind. A message carries the handle and a
  descriptor: width, height, stride, channels, sample type (f32 or u16), colour
  space, transfer, and the region of the photo it covers. A program that returns
  pixels writes into an output buffer the request supplied, or asks the app for
  one (`allocate`) when the size isn't known in advance.
- **Ownership.** An input buffer is the program's to read until it replies;
  on Linux it is sealed against writes. An output buffer is the program's to
  write until it replies, after which the app copies out what it keeps and never
  reads it again. Every buffer of a request is released when its reply arrives,
  when it is cancelled, or when the program exits, so a crash leaks nothing.
  Large images travel as tiles with a stated overlap.

### Session

1. **Handshake.** The app sends `hello` with the protocol version, the app's
   version, the extension's settings and the contributions the manifest
   declares. The program answers with its own protocol version and which
   contributions it implements. A version mismatch ends the session with a clear
   error shown on the extension's page.
2. **Requests** go both ways, each with an id; every request gets exactly one
   reply, `ok` with a value or `error` with a code and a message. Long requests
   may send `progress` before their reply.
3. **Cancellation.** `cancel {id}` asks the other side to stop; the reply then is
   `error: cancelled`. Every app request has a deadline; one exceeded is
   cancelled, then failed.
4. **Events** the app sends without expecting a reply: the selection changed, a
   photo opened, a setting changed, the app is quitting.

### What the app offers a program

A program sees the app through the same guarded operations automation uses, not
the editor's internals:

- **Reads**, always from a snapshot: the photos of a selection or a search, their
  metadata, edits as settings, the open photo's identity (`photo_id`,
  `generation`, `revision`), folders and collections.
- **Pixels:** the photo as developed (a region, at a scale, in a stated colour
  space), the camera image before develop, or a stored mask.
- **Proposals**, which the app checks and applies itself: settings changes to one
  photo or a selection (one History step each, through
  `presets::apply_to_photo`'s path), metadata changes, a new mask raster, a new
  retouch patch, an import plan, new files to add.
- **Messages:** notifications, a status-line entry while work runs, and a request
  for the user to confirm.
- **Storage:** a private data folder and cache folder per extension.

**Targets.** Every read the app answers carries a target token the app issued:
the open catalog (its `CatalogLocation` and session), the catalog session's
structural generation, and for each photo its id, the file `Identity` the read
saw, and its edit and metadata revisions; for the open photo, also Develop's
load generation and revision, as automation's guarded commands use today. A
proposal names the token it was computed from. The app checks it photo by photo
just before applying: a photo whose file, edit or metadata moved on, a
structural change (photos removed, a photo id reused), or another catalog is
refused as stale, and the reply lists which photos were applied and which were
refused. Nothing is applied from a token of a catalog that is no longer open.

**Sources are scoped to a catalog.** A `source` runs for the catalog it was
started for; closing or switching catalogs sends `catalog-closed`, stops
admitting its reports, and starts it again for the next catalog if the user
enabled it there.

### Lifecycle and failure

- `on-demand` programs start at their first request and stop after a period with
  nothing to do; `with-app` programs (MIDI, watched folders) start with the app.
  One process per extension, never one per request.
- **The host supervises** every program as an owned service in `app::work`:
  whether quitting waits for it, whether the status line lists it, and its step
  of the exit sequence. On quit the app sends `quitting`, closes the message
  channel, waits up to the shared deadline, then kills the process. Nothing
  waits without a deadline, and nothing joins in `Drop` ([shutdown](shutdown.md)).
  Externally launched adapters ([below](#official-extensions)) aren't the app's
  processes and are not part of this sequence; their requests in flight end as
  every control request does when the app quits.
- A program that crashes or exits fails every request it held, clears every busy
  state they set, and releases its shared memory. It is restarted on its next
  request; three crashes in a row mark the extension **Failed** until the user
  retries from Preferences.
- A program and everything it starts run as one unit the app can end: a process
  group on Linux and macOS, a job object on Windows that kills its processes when
  the app's handle closes, so a helper process can't outlive the app. A program
  that stops reading its socket is treated as hung: the app's writes are
  non-blocking and bounded, and a full channel fails the request instead of
  blocking the UI or a worker.
- **Disabling** an extension stops admitting its requests, cancels those running,
  stops its program, and removes its commands and panels. Its filters keep
  rendering ([decided](#decisions-taken)).
- Every job captures the extension version it started with and keeps that
  version installed until the job ends, so an update mid-export can't switch
  implementations halfway.

## Contributions

What each contribution is, which side owns what, and the invariants it keeps.

| Contribution | Backed by | The extension does | The app keeps |
| --- | --- | --- | --- |
| `command` | program | Runs, given its target and optional viewer input; returns proposals | Availability, the key map, the target guard, History |
| `setting` | — | Declares settings | Drawing and storing them; sending changes to the program |
| `panel` | program, or none for filter panels | Declares controls; reacts to their values | Drawing, layout, undo grouping |
| `filter` | kernels | Computes pixels | Its place in the recipe, validation, scheduling, caches, missing and disabled handling ([Filters](#filters)) |
| `mask-generator` | program | Returns coverage for the photo (with optional point/box input) | Validating the raster, storing it through `mask_assets`, the mask and its History step |
| `retouch-generator` | program | Returns RGB pixels and coverage for a region (object removal) | Storing the patch as an asset, its place among retouches, rendering it ([Retouch patches](#retouch-patches)) |
| `importer` | program | Reads another application's catalog, once, into an import plan | Validating and committing the plan in one transaction; preserving the source's unsupported data |
| `source` | program | Watches for arriving files (a watched folder, a tethered camera) and reports each complete one | Adding them to the catalog, duplicates, conflicts, never moving or deleting originals |
| `export-destination` | program | Receives each published file and sends it somewhere (upload, gallery, printer) | Rendering, encoding and atomic publication; "exported" and "uploaded" reported separately |
| `control-adapter` | program | Translates a device or a protocol into commands (MIDI, MCP) | Commands, gesture grouping and History, exactly as automation does today |
| `presets`, `luts`, `profiles`, `keymap` | files | — | Discovery, validation and use, as for the user's own |

**Auto modes** are commands placed in `library.auto` and `develop.auto`: they read
the photo (pixels and settings) and propose settings, which apply as one History
step per photo. Their result is ordinary settings, so it survives the extension.

**Importer plans** describe photos, folders, collections, keywords, ratings,
labels, stacks, virtual copies, and edits, History steps and snapshots as
RAWmakase settings, plus the source's original data (including each edit,
step and snapshot as the source stored it) and its unsupported data, which the
app archives as the Lightroom import does today. The
importer never sees the catalog's database. The edit an importer proposes is
RAWmakase settings; an importer that converts another application's edit format
does it in its program.

**Imports are one-time.** An import converts what the source holds at that
moment and keeps no link to it: later changes in Capture One or Lightroom aren't
followed, and nothing is written back. Today's Lightroom import always creates a
new catalog; whether an importer may also add to the open catalog, and what a
second import of the same source does with photos already there, is decided
when importers are built ([Still open](#still-open)). The plan shape is meant to
hold:

- **Lightroom Classic**: today's importer, once it moves out (a follow-up, see
  [Official extensions](#official-extensions)). The plan must be able to carry
  everything it imports now, so moving it changes nothing for its users.
- **Capture One**: catalogs and sessions, with their adjustments converted in
  the extension.
- **Lightroom CC** (the cloud-based Lightroom): its local library, and originals
  that may live only in Adobe's cloud. An importer that downloads originals
  does so in its program, into a folder the user chose, and the plan names the
  downloaded files.

A plan may also name originals that aren't reachable now, such as photos on a
disconnected drive: they are imported as offline photos and relinked later, as
the Lightroom import does today.

**Sources** report files only when complete (a tethered capture fully written, a
copy finished). Reconnection and duplicate reports are expected; the app matches
by identity, so a file reported twice is added once.

## Filters

The one contribution that runs inside the renderer, and the only one saved in
edits.

### Stages

A filter declares one stage, which fixes where it runs. Two stages exist at
first:

| Stage | Where in the pipeline | Sees |
| --- | --- | --- |
| `display` | After the whole per-pixel develop stage (tone, curves, mixer, Point Color, grading, local adjustments), before detail, sharpening and finishing | The developed photo, before any output encoding |
| `finish` | After sharpening and after the built-in vignettes and grain; before resizing for export, the monitor profile, the histogram, the clipping overlay and quantisation | The finished photo at output size |

**One colour contract for every stage.** Kernels always receive and return
**linear light in a fixed working space: Rec. 2020 primaries, D65 white, 1.0 =
diffuse white**, with negative values allowed (out-of-gamut colours) and nothing
clipped on the way in. The app converts into it before the first filter of a
stage and back after the last: from the preview's sRGB working values on the
GPU, from the export's output space (sRGB, Adobe RGB or ProPhoto) on the CPU.
So the same numbers mean the same colour in the preview and in every export
space, and a filter never needs to know which space the export uses. The SDK
provides the conversions a kernel may want internally (to an encoded or
perceptual scale and back).

**HDR.** An HDR edit has two independent renders, not one split: the HDR master
(`Recipe::resolved_hdr`, rendered on the CPU) and the SDR rendition, which is
the edit rendered again with HDR off (`Recipe::resolved`), used for the SDR
preview, SDR exports and a gain map's base ([HDR](hdr.md)). Filters run in each
render at that render's stage boundaries: in the master on the CPU, where values
above 1.0 are highlights above diffuse white; in the SDR rendition like any SDR
render. A filter with `hdr = false` is skipped in **both** renders of an HDR
edit, with a message, so the master and the SDR base of a gain map never
disagree about which filters ran; resolving the SDR rendition therefore keeps
the knowledge that the edit is an HDR edit. Tests cover the master preview, the
SDR preview, an SDR export and both parts of a gain-map export.

**`scene` filters come later.** A filter before the scene tone stage would change
what the stage measures (Auto Tone at Exposure 0, the unretouched copy for the
default black, haze, the local maps' bases, the contrast pivot;
`develop::pipeline::tone::photo_measures`), and a filter that isn't linear
doesn't commute with exposure, so it can't simply run on today's measurement
copy. `scene` is added only after a design that says how each measurement and
its cache key include it, with cached-against-fresh and Fit, region and export
tests.

Within a stage, filters run in the order the user arranges them in Develop
(a filter panel can be dragged up or down); the same filter may be added more
than once. Stage boundaries never move; the core decides where a stage sits.

**What the GPU path needs first.** Today `develop.wgsl` runs the per-pixel stage
as one fused pass that ends in encoded sRGB, and `present.wgsl` runs sharpening,
vignettes, grain, the monitor profile, the histogram and 8-bit encoding as
another. Filter work therefore starts by giving both passes a variant that stops at
the stage boundary and keeps linear f32 values (then filter passes, then the
rest), selected only when the recipe has filters, so every recipe without
filters keeps today's fused passes and today's output. The CPU stages get the
same two boundaries.

### Kernels

Two files per filter, written side by side in the author's project:

- **WGSL**, a single function the app wraps in its own compute pass with fixed
  bindings. For a `point` filter:
  `fn apply(px: vec4f, at: Pixel, p: Params) -> vec4f`.
  For a `neighbourhood` filter, `px` is replaced by a sampler bounded by
  `reach`. `Pixel` holds the position in full-resolution photo coordinates, the
  scale of the current render (a Fit preview is not 100%) and the instance's
  seed. `Params` is generated from the manifest's `params`. The app compiles the
  function with naga when it loads the extension and refuses a filter whose
  signature or bindings don't match, with the error on the extension's page.
- **WASM**, compiled from Rust (`wasm32-unknown-unknown`, SIMD on), exporting
  `apply_tile` over an RGBA f32 tile with its origin, scale, seed and parameters.
  It imports nothing: no files, clock or randomness, so it is deterministic and
  can't block. The app runs it with wasmtime, tile by tile across its Rayon pool,
  with a memory limit and an execution limit per tile.

The SDK ships the helpers both need (hashes, noise, luminance, colour
conversions) in WGSL and in Rust with identical results, so the two versions
read nearly line for line.

**Regions, tiles and Fit.** Two contracts, kept apart:

- **Exact:** at 100%, a tile or region renders the same pixels as the same area
  of the full image (within float tolerance). The app guarantees the input for
  that: a region is expanded by the combined reach of every spatial operation
  from the region's start to the last filter: each neighbourhood filter's
  `reach`, sharpening's halo, and the masks' Noise (a 5×5 neighbourhood, after
  sharpening; `develop::quality::detail::local_noise`) and any other built-in
  neighbourhood stage in between. Only pixels beyond the photo's edge are
  clamped to the edge. The region renderer today expands only for sharpening, so
  this is part of the filter work, with a test of a region edge under mask Noise with
  sharpening at zero.
- **Approximate:** a Fit or zoomed-out preview renders from a reduced pyramid
  level, before the non-linear develop stage, so it can never equal the full
  render reduced; the core's own Fit tests exclude grain for that reason. A
  kernel receives the render's `scale` (output pixels per full-resolution pixel),
  reads `reach` scaled by it, and is responsible for looking right at that scale
  (grain averages its full-resolution pattern, as the built-in grain does). The
  test for this compares local statistics, not pixels: the mean and the variance
  per block, and the energy per frequency band, of the Fit render against the
  full render reduced, within tolerances the filter declares.

`cargo rawmakase test` checks CPU against GPU on the extension's test images,
the exact contract on random tiles and regions, and the approximate one at
several Fit sizes.

Writing the kernel once in Rust and generating the WGSL (rust-gpu to SPIR-V to
WGSL through naga) would remove the second copy. It needs a pinned nightly
toolchain today, so it is a possible later addition, not part of this design.

**Masks come later.** Limiting a filter by one of the photo's masks needs things
the model doesn't have: masks have no persistent id (`MaskGroup` is matched by
position and refuses unknown fields), Copy Settings, Sync and presets leave
raster masks behind, and a range mask selects from a render that would itself
include the filter. Filters are global until a design gives masks stable ids
(a saved-format change of its own), says what a filter's reference becomes when
its mask is deleted or doesn't transfer, and defines a range mask's input
without the filters it controls.

### How the renderer runs them

- The engine never sees extensions, packages or the host. A render job captures,
  with the recipe, the **resolved filters**: for each instance, its compiled GPU
  pass, its CPU kernel and its parameters, plus each filter's content digest
  (SHA-256 of its WGSL and WASM). This is the same pattern as AI Denoise's
  `AiSource`: the engine receives an immutable resolved value
  ([architecture](architecture.md#boundaries-to-preserve)).
- The engine defines what a resolved filter is; a separate crate,
  `rawmakase-filter-runtime`, builds one (naga, wasmtime), and the app, export
  and the command line use it. No value, catalog or engine crate depends on
  wasmtime.
- On the GPU, each filter is a pass of its own at its stage's boundary in the
  split passes ([Stages](#stages)); several filters of a stage run one after
  another on the same linear f32 buffers. On the CPU, the same boundaries call
  the WASM kernel. A GPU failure in a filter turns
  the GPU off for that filter for the session and renders it on the CPU, as for
  built-in stages.
- **Rendering identity.** Each filter's digest covers everything that decides
  its pixels: its WGSL and WASM bytes, the manifest's filter table (stage, kind,
  reach, `hdr`, parameter declarations and defaults, `params-version`) in a
  canonical form, and the version of the app's filter interface (the wrapping,
  bindings and helpers). It is computed once when the extension loads and
  captured with each job, which executes with the same resolved filter its
  identity names.
- **Cache identity**: the resolved filters (digests, parameters, seeds, order) are
  part of the stage cache's keys (`develop::stage_cache`, whose exhaustive split
  of the recipe makes this a compile error until it is placed) and of the
  Library's edited-preview tags (`app::library::previews::source`). Any change
  to a filter's rendering contract, including a manifest-only one, changes its
  digest, so previews made before it are rebuilt and a preview render still
  running with the old one is dropped when it completes. An export already
  running finishes with the version it captured.
- **Failures.** A kernel that traps, exceeds its memory or execution limit, or
  returns a non-finite value fails that filter for that render. On the GPU the
  render falls back to the CPU from the same captured inputs, as built-in stages
  do; on the CPU the photo renders without that filter and says why, with the
  same message and badge as below. An export that meets it publishes the photo
  without the filter and lists it among the export's warnings. Nothing about the
  saved instance changes.

### In the recipe

- `Recipe` gains `filters: Vec<FilterInstance>`, omitted while empty. An instance
  holds the extension id, the filter id, an instance id, whether it's on, its
  stage, its position, `params-version`, its parameters as a JSON object and its
  seed (chosen when it is added).
- Parameters are validated against the manifest when the extension is available;
  when it isn't, they are kept exactly as they are, including ones the manifest
  doesn't know.
- When `params-version` grows, the WASM module may export `migrate_params`,
  which the app calls when it resolves an older instance. Migration is all or
  nothing: its output must validate against the manifest, or the instance is
  left exactly as saved and the filter is treated as failing ("can't read its
  settings"). A migrated instance is written only when that edit is next saved
  for another reason. Without `migrate_params`, unknown parameters are kept and
  missing ones take their defaults.
- **Saved format.** A recipe with any filter is written as a new schema version
  (`model::saved_format`, after `HDR_SCHEMA`), and a catalog upgrades to a new
  format version before it stores one ([catalogs](catalogs.md)). Releases from
  before extensions then refuse such an edit instead of rendering it without its
  filters and dropping them when saving, as they do for Lensfun, AI Denoise and
  HDR today.
- **Copy Settings, Sync and presets** gain an Extension Filters group. Native
  presets carry filters; an XMP or Lightroom preset export says filters were left
  out.
- History, snapshots and virtual copies carry instances as part of the recipe,
  with nothing extra to do. `catalog::edit_rows` stays the only writer.

### When the extension isn't there

| State | Renders | Says | The user can |
| --- | --- | --- | --- |
| Installed and enabled | With the filter | — | Edit it |
| Disabled | With the filter | "Uses Analog Grain from Analog Toolkit, which is disabled" | Enable the extension, or remove the effect |
| Missing or incompatible | Without the filter | "Needs Analog Grain from Analog Toolkit (dev.jane.analog), which isn't installed", with Install when the catalogue knows it | Install it, or remove the effect |
| HDR edit, filter without HDR | Without the filter | "Analog Grain doesn't support HDR" | Turn HDR off, or remove the effect |
| Failed (trap, limit, non-finite output, failed migration) | Without the filter | "Analog Grain failed: …" | Update or reinstall the extension, or remove the effect |

The message shows on the filter's panel in Develop, and a badge marks the photo
in the Library and the filmstrip. **Remove effect** deletes the instance as one
History step; nothing is removed without the user asking. Opening a catalog
never installs or runs an extension because an edit names it.

**Exports** of such photos go ahead without the effect. Before starting, the
Export dialog lists the photos that will look different and the extensions they
need; `rawmakase render` and `render-batch` print the same warning, and
automation's export reply carries it.

## Retouch patches

AI object removal needs something the model doesn't have yet: generated pixels
stored with the edit. A **patch** is a raster with coverage, held by content id
in an asset store like `model::storage::mask_assets`, saved by
`catalog::edit_rows` in the same transaction as the edit naming it, and placed
among Heal and Clone spots in the retouch order. The generator runs once; the
patch renders forever after, without the extension. This is a core addition
(model, catalog format, engine) that the `retouch-generator` contribution
depends on.

**Its pixels live where Heal and Clone work**: linear camera RGB, before white
balance and the profile, so a patch still fits after the user changes white
balance, exposure or the profile. Most inpainting models expect a viewable
image, so the app gives the program the region in a **reversible linear
rendition**: the camera pixels multiplied by the photo's white balance and
converted to the filters' working space by a 3×3 matrix, with no tone curve and
nothing clipped, plus the developed preview of the same region for context. The
program returns its patch in that same linear rendition (any tone mapping the
model needs it applies and undoes itself), and the app converts it back to
camera RGB with the inverse matrix and multipliers, exactly. Coordinates are the
photo's image frame, as spots use (`model::image_frame`). The patch records
whether it was made from the plain or the AI Denoise source; it applies to
either, and the Develop panel notes when they differ. The patch work's tests cover a
white balance or exposure change after the patch, patches interleaved with Heal
and Clone, a patch asset that can't be loaded (the render fails as for a mask
raster, never renders without it silently), catalog export, and an asset whose
save fails (the edit isn't saved without it).

## Future: derived photos

AI upscaling and similar tools produce a new image, not an edit. The design
leaves room for a `derive` contribution: a program receives the developed photo
and returns a new file, which the app adds to the catalog stacked with the
original. Nothing in this plan depends on it, and it is not scheduled.

## The SDK

`rawmakase-extension`, published to crates.io, depends only on the protocol
crate:

- `#[rawmakase::extension]` and handler traits for each contribution
  (`Command`, `MaskGenerator`, `Importer`, `Source`, `ExportDestination`, …), a
  run loop that does the handshake, framing, cancellation and shared memory.
- Typed bindings generated from the manifest (settings, filter parameters,
  command ids), so a typo is a compile error.
- The kernel helpers in Rust and WGSL, and the `apply_tile` and `migrate_params`
  glue.
- `cargo rawmakase` with `new` (templates: command, filter, importer, source,
  mask generator), `build`, `test`, `dev`, `package` and `release`, plus the
  GitHub Actions template.
- A mock host for unit tests: feed requests, inspect proposals, without the app.

## Documentation

`docs/extensions/`, written as the parts are built:

- **Overview**: the model, trust, the decisions above.
- **Tutorial**: an Auto mode, then a filter, then an importer, each from
  `cargo rawmakase new` to a published release.
- **Manifest reference**, generated from the JSON Schema.
- **Protocol reference**: every message, error code and buffer descriptor,
  generated from the protocol crate's types, with recorded example sessions.
- **Filter guide**: the stages and what their pixels mean, the kernel signatures,
  determinism, scale and tile rules, parity testing.
- **Publishing**: packages, targets, the catalogue's review, the release template.
- The official extensions as worked examples, with links into `extensions/`.

## Code layout

New crates, all below the app:

| Crate | Owns | Depends on |
| --- | --- | --- |
| `rawmakase-extension-protocol` | Messages, buffer descriptors, versions, the manifest's types and schema | Nothing of the workspace (like `rawmakase-protocol`) |
| `rawmakase-extension` (SDK) | The author's side | The protocol crate only |
| `rawmakase-extensions` | The host: reading manifests, packages, installation, the catalogue, `extensions.json`, starting and supervising programs | Protocol, `rawmakase-fs`; no egui, engine or catalog |
| `rawmakase-filter-runtime` (with filters, [not scheduled](#designed-not-scheduled)) | Compiling WGSL with naga, running WASM with wasmtime, building resolved filters | Engine (for its types), protocol |

In the app: `app::extensions` (the Preferences tab, panels, viewer input, the
proposals' application through existing owners), and the command registry in
`app::shell::commands`. Official extensions live in `extensions/<name>/` as
workspace members built with the SDK, which keeps the SDK honest: the bundled
extensions use nothing a third party can't.

`scripts/deps-crates.txt` gains the rules: the model, interop, catalog and engine
never reach the host, the filter runtime or wasmtime; the protocol crate reaches
nothing; the SDK never reaches the app's crates. `docs/architecture.md` and
`docs/code-map.md` are updated as each crate lands. `deny.toml` is checked
against wasmtime's dependencies.

## Official extensions

Bundled, installed and enabled; each can be disabled. "Moves out" is what the
extension owns afterwards; "stays core" is what the app keeps so saved photos and
other features don't depend on the extension.

| Extension | Moves out | Stays core | Proves |
| --- | --- | --- | --- |
| **MIDI** (`rawmakase.midi`, built) | Device discovery and connection, device profiles, mapping (`extensions/midi`) | Commands, gesture grouping, History | `control-adapter`, `with-app` programs, settings |
| **MCP** (`rawmakase.mcp`) | The MCP server (`src/mcp.rs`), already a client of the control protocol | The command layer, and `rawmakase mcp` as the entry point clients already launch | An externally launched adapter (below) |
| **AI masks** (`rawmakase.ai-masks`) | Select Subject, Sky and Background: the models, their install (`model_install` for those packs), local and remote inference (`rawmakase-remote` as the extension's own worker) | Masks, rasters and their storage; rendering saved masks | `mask-generator`, viewer input, shared memory, heavy native dependencies |

**Lightroom import stays in the core for now.** Moving it out
(`rawmakase.lightroom`) is a follow-up, built with the `importer` contribution.
It would move reading a Lightroom catalog into an import plan
(`catalog::lightroom`'s reading side) and converting its Develop settings
(`lr_develop`), and keep committing the plan and archiving unsupported data,
including the original Lightroom settings, in the core. It depends on a separate
change that is planned on its own and must land first. Today a Lightroom edit is
stored as Lightroom's text and converted each time the photo's edit is resolved
(`edits::resolve`), and imported History steps and snapshots are converted the
same way when they are selected or applied
(`app::develop::panels::history_panel`, `snapshots`), so disabling the importer
would change imported photos. That change converts edits, History steps and
snapshots once at import and migrates existing catalogs, keeping the originals
archived. When the importer moves, tests apply an imported step and snapshot,
and Copy to Before, with the extension disabled. Until then, nothing built here
may make that move harder: the import-plan shape must be able to carry
everything today's importer produces ([Importer plans](#contributions)).

Lens corrections data and community profiles stay in the core as they are.

**Extensions others could write.** These are not planned and RAWmakase won't
write them; they show which contribution each request people make would use.

| Request | Contributions |
| --- | --- |
| Film grain | A `finish` filter, distinct from the built-in Grain, which keeps Lightroom's meaning |
| Extra Auto modes (Auto Film, Auto Low-key) | Commands in `library.auto` and `develop.auto` proposing settings |
| Capture One import | `importer` for Capture One catalogs and sessions |
| Watched folders | `source` that adds files appearing in chosen folders |
| Tethering | `source` for a connected camera, plus commands |
| AI object removal | `retouch-generator` with brush input |
| AI upscaling | `derive`, once it exists ([Future](#future-derived-photos)) |

**Externally launched adapters.** An MCP server is started by its client (an AI
agent), not by the app, and speaks MCP on its own stdin and stdout. A
`control-adapter` may therefore declare `launch = "external"`: its program isn't
supervised by the app, and connects to the app through the control socket
(`control.json`), as `rawmakase mcp` does today. `rawmakase mcp` stays the
command clients are configured with: it runs the bundled extension's program
when the extension is enabled and fails with "The MCP extension is disabled in
Preferences > Extensions" when it isn't.

Disabling must also stop a server that is already running, and the control
protocol opens a new connection for every request (`rawmakase-ctl`'s
`Connection::request`), so there is no connection to close. Control requests
therefore gain an optional `adapter` field naming the extension and adapter
that sends them; the bundled MCP program sends it on every request, and the app
checks that adapter's enabled state when it dispatches each request, including
queued ones, refusing with `adapter_disabled`. Requests without the field, from
`rawmakase control`, `rawmakase-ctl` and scripts, work as today. The field is
cooperative identification, not authentication: the control token still decides
who may control the app. Each MCP client runs its own server process, which
sends one control request per connection, as today. Tests cover an end-to-end
MCP edit, and disabling the extension while its server is running (its next
request is refused).

Stay in the core: AI Denoise (its setting is part of the edit, and the result
depends on the model in ways the recipe doesn't name), the built-in Grain and
every other Lightroom-compatible setting, watermarks, colour management, decoding
and export encoding.

## Phases

Built now, in this order. Each phase ends with the features it moves out
working, `make check` passing and the docs for what it added. A contract is
built only together with an extension that uses it.

1. **Command registry and key map.** Every action named in one registry, the key
   map, Preferences > Shortcuts, the generated reference card. Useful on its own.
   Today shortcuts are matched at several points around widget drawing, so this
   phase also gives key events one owner per frame; a registry alone doesn't.
2. **Packages, programs and the protocol.** The manifest and its schema, the
   host crate, installation from a file and from GitHub, `extensions.json`,
   Preferences > Extensions, bundled extensions in the release workflow; the
   protocol crate, the SDK and `cargo rawmakase`; supervision under `app::work`
   and the shutdown contract; commands with proposals and targets, settings,
   host-drawn panels, and externally launched adapters. Move MIDI and MCP out.
3. **Viewer input and mask generators.** `point`, `points`, `box` and `prompt`
   input (clicks and a dragged box combined, as Select Subject takes them
   today), shared-memory pixels and rasters, `mask-generator`. Move
   Select Subject, Sky and Background out into AI masks, with the remote worker.

### Phase 2 decisions

Defaults taken for phase 2 (#964), where the design left a choice open:

| | Decision |
| --- | --- |
| D1 | Bundled extensions' programs ship beside the app's executable (`../lib/rawmakase` on Linux packages), their manifests are compiled into the app, and the release workflow also attaches their `.rmext` packages |
| D2 | The bundled programs are named `rawmakase-mcp` and `rawmakase-midi` |
| D3 | The control socket's legacy `cc`/`note` requests stay in the core as a frozen Loupedeck compatibility adapter |
| D4 | MIDI's settings page moves to Preferences > Extensions; the Automation tab links to it |
| D5 | MIDI's configuration moves to the extension's data folder; the original `midi.json` is left in place, so a downgrade keeps working |
| D6 | Every `places` slot name is validated in manifests now; drawing commands into those slots waits for the first extension that declares one. Phase 2 gives extension commands keys, `rawmakase control action` and a Run button |
| D7 | No shared memory or handle passing in phase 2: no phase-2 extension moves pixels. The framing for handles and the buffer descriptor are specified now ([protocol](extensions/protocol.md#reserved-handles-and-buffers-phase-3)) |
| D8 | A control request naming an unknown or disabled adapter is refused `adapter_disabled`; enabling or disabling from the command line reaches a running app on its next start |
| D9 | Target tokens use edit and metadata digests, not new catalog counters, so the catalog format doesn't change |
| D10 | An on-demand program stops after 5 minutes idle; requests have a 30 s deadline (5 s for `hello`); the quit grace is 1 s |
| D11 | `cargo rawmakase new` has command and control-adapter templates only; `cargo rawmakase test` waits for filters |
| D12 | The Failed state (three crashes in a row) lasts for the session |

Also decided while building: the manifest's `api` is an integer (1 now);
contributions are named `<table>/<id>` in `hello`; a program that exits before
answering fails its requests with `exited`, and a proposal after its request
ended with `not_admitted` (codes added to the design's list); a package's
target comes from its file name, `<id>-<version>-<target>.rmext`, which must
match its manifest; ids under `rawmakase.` can't be installed from a package.

### Designed, not scheduled

The rest of this plan is designed so that the phases above leave room for it,
but each part is built only when someone wants to write an extension that needs
it. That extension proves the contract; when it lives outside this repository, a
test extension in the SDK proves it here as well.

| Part | Builds on | Who asked for it |
| --- | --- | --- |
| `files` content: presets, LUTs, profiles, key maps | Phase 2 | Preset and LUT packs |
| Brush input | Phase 3 | Object removal |
| `importer` | Phase 2 | One-time Capture One and Lightroom CC imports; then moving the Lightroom importer out, after Lightroom edits are converted at import |
| `source` | Phase 2 | Watched folders, tethering |
| Filters | Phase 2 | Film grain, custom filters |
| Retouch patches and `retouch-generator` | Phase 3 and brush input | AI object removal |
| `export-destination` | Phase 2 | Uploads, galleries |
| The official catalogue | Third-party extensions to list | — |
| `derive` | Phase 3 | AI upscaling |

**Filters start with a measured spike**: a filter pass on the split GPU passes
and a WASM kernel on the CPU path, timed for slider updates on the GPU, CPU Fit,
Library preview throughput, export, cancellation latency and peak memory, plus
wasmtime's binary size and dependencies against `deny.toml`. The filter contract
is fixed only after that. Then the filter runtime, the split passes, the
recipe's `filters`, the saved-format and catalog versions, the missing,
disabled and failed states, export warnings and parity tests in the SDK.
`scene` filters and filters limited by masks follow only after designs of their
own ([Stages](#stages), [Kernels](#kernels)).

**Room to keep.** Phases 1–3 must not close these off:

- the manifest's unknown-key rule rejects tables this version doesn't know, so
  each new contribution table arrives with an `api` version that knows it;
- proposals and target tokens are already per photo and per catalog, as
  importers and sources need;
- shared-memory buffers carry a full descriptor (colour space, transfer, region),
  not only what masks use, so filters' and patches' pixels fit without a
  protocol change;
- `Recipe` and the saved format are not touched until filters are built.

## Tests each part needs

Following AGENTS.md: a failing test first for bugs, observable behaviour at the
boundary, lifecycle cases covered deterministically.

- **Manifest and packages:** schema round trips, unknown keys refused,
  incompatible `rawmakase` ranges, digest mismatch, an interrupted install leaving
  the previous version, `extensions.json` keeping unknown fields.
- **Protocol:** a scripted fake program for each failure: crash mid-request,
  hang past the deadline, malformed or oversized lines, a protocol version
  mismatch, cancellation racing completion, a stale proposal (photo, generation
  or revision moved), shared memory released on crash. The protocol parser joins
  `fuzz/`.
- **Lifecycle:** disabling during a job, quitting with a hung program (bounded by
  the deadline), a program that stops reading its socket, a program whose helper
  process outlives it, an update during an export (the job keeps its version),
  restart after three crashes.
- **Shared memory:** buffers in both directions created after startup, on Linux,
  macOS and Windows; release on reply, on cancel and on crash.
- **Targets:** a proposal after a catalog switch, after a photo was removed and
  its id reused, after its file was replaced, and after another edit to a photo
  that isn't open; a selection proposal applied partly, with the refused photos
  listed.
- **Key map:** conflicts, a text field keeping its keys, overrides surviving new
  defaults, a popup or tool taking precedence, a focused control that isn't a
  text field, clipboard shortcuts, and a modifier released within the frame.
- **Filters:** CPU against GPU, the exact tile and region contract, the
  approximate Fit contract, determinism across thread counts, the same colours
  in every export space; recipes without filters render exactly as before on the
  fused passes; missing, disabled, incompatible, HDR and failed cases (a trap, an
  exceeded limit, non-finite output, a failed migration) render and report as
  the table says; Remove effect is one History step; export warns and goes on; a
  pre-extension release refuses the new schema (a stored fixture); parameters of
  a missing extension survive save, History, snapshots, virtual copies, Copy
  Settings and presets unchanged; the stage cache and Library previews rebuild
  after an extension update, including a manifest-only one.
- **Extractions:** each moved feature keeps its existing tests, run against the
  bundled extension; saved masks render with AI masks disabled; MIDI and MCP
  edits behave as before.
- The parts [not scheduled](#designed-not-scheduled) bring their tests above
  (filters, targets for importers and sources) when they are built.
- **Rendering references are never re-blessed** for this work: no built-in stage
  changes its output.

## Still open

Decisions to take when their part is built, not before:

- Memory budgets for shared-memory pixels and for idle programs (phase 2).
- `scene` filters: the measurement design described under [Stages](#stages).
- Filters limited by masks: stable mask ids and transfer rules, under
  [Kernels](#kernels).
- Whether filters may contribute their own parameters to Auto (for example an Auto
  Grain amount), or Auto stays settings-only.
- Whether an importer may add to the open catalog as well as create a new one,
  and what a second import of the same source does with photos already there.
- The catalogue's review rules and who reviews.
