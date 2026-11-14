# Extensions

Status: Phase 2 built ([design](../extensions-plan.md)). The manifest,
packages, installation, `extensions.json`, the extension protocol, the SDK and
`cargo rawmakase` exist, and the app runs extensions: it supervises their
programs, and has Preferences > Extensions, their commands, settings and
panels, reads and proposals with target tokens, control adapters, the app's
own and externally launched ones, mask generators (`api = 2`), and photo
sources, retouch generators and secret settings (`api = 3`); experimental AI
Remove uses the last two. MCP, MIDI and AI Masks are bundled extensions; AI
Remove is listed for installing but not bundled.

An **extension** adds to RAWmakase without changing it: commands, settings,
panels of controls the app draws, control adapters (a MIDI device, an MCP
server), mask generators and photo sources (a watched folder, a tethered
camera). It is a package with one manifest, `rawmakase-extension.toml`
([reference](manifest.md)), and usually a **program**: one native executable
that runs as its own process and talks to the app over the
[extension protocol](protocol.md). Extensions are written in Rust with the SDK,
`rawmakase-extension`, and packaged with `cargo rawmakase`
([publishing](publishing.md)).

## Trust

An extension runs with your permissions, like a Blender add-on: it can read and
write anything you can. Its manifest declares what it reaches (`[access]`:
the network, which files, which devices), and RAWmakase shows that before
installing it and when an update asks for more, but doesn't enforce it.
Extensions installed from a file or a GitHub repository are marked **Not
reviewed**. The official extensions (`rawmakase.*`) ship with the app and can
be disabled, not uninstalled; no other package may use their ids.

Installing never runs an extension's code: its program first starts when one
of its contributions is used.

## Bundled extensions

The official extensions are compiled into the app (their manifests) and
installed beside it (their programs): in the executable's folder, which is
`Contents/MacOS` in the macOS app and the folder holding `rawmakase.exe` on
Windows, or on Linux in `../lib/rawmakase` beside the executable's folder, where
the packages and `make install` put it. A bundled extension is enabled unless
`extensions.json` says it is disabled, except MIDI, which is off until it is
enabled (or was set up before it became an extension); it updates with the
app and can't be uninstalled. When its program is missing (an incomplete install), its page says
so. This release bundles three:

- **MCP** (`rawmakase.mcp`, program `rawmakase-mcp`): the MCP server
  ([MCP setup](../mcp.md)), an externally launched control adapter, `mcp`.
- **MIDI** (`rawmakase.midi`, program `rawmakase-midi`): MIDI control surfaces
  ([MIDI devices](../midi.md)), a control adapter the app starts with itself,
  `midi`, and its page, a panel the program drives. Its settings are in its
  data folder; on its first start the app copies the `midi.json` the core kept
  there, and leaves the original.
- **AI Masks** (`rawmakase.ai-masks`, program `rawmakase-ai-masks`): Select
  Subject, Sky and Background, two mask generators (`subject`, `sky`) run by
  the selection models on this computer or on a remote worker
  ([Subject and Background masks](../subject-and-background-masks.md)). It
  keeps the models in the local data folder's `models`, where the app always
  installed them.

## Control adapters

A `[[control-adapter]]` the app launches (the default) is served by the
extension's program, started with the app (`with-app`). Its devices report
their connections with `adapter.device` (connected, disconnected, removed,
each with an epoch that counts up with every connection), and their input as
named operations with `adapter.operate` ([protocol](protocol.md#control-adapters)).
The app takes a device's report in on the channel's reader thread, ahead of
any operation still waiting, and runs each operation only while its device is
connected in the operation's epoch: input from before a disconnection never
runs, however much of it waits. Each program start's devices are its own: a
restarted program's devices never continue a gesture of the previous run.
When the operations come faster than the app takes them, the ones that don't
fit are refused with `channel_full` at once. A disconnection, the program's
end and disabling the extension end its devices' gestures; disabled, its
operations are refused.

## Photo sources

A `[[source]]` ([manifest](manifest.md#source)) finds new photos as they
arrive, a watched folder's or a tethered camera's, and the app adds them to
the catalog. Watching the folder or talking to the camera is the program's
work, in any crate it likes; placing, adding and showing the photos is the
app's, so a photo a source added is an ordinary photo of the catalog, which
stays when the extension is gone.

- **On per catalog.** A source is off until the user turns it on for the
  open catalog on its extension's page (**Photo sources**, *Add its photos
  to …*), and it adds only to catalogs it is on for. `extensions.json`
  records them, under the extension's entry, as `"sources": {"<source>":
  ["<catalog file>", …]}`, on this computer only.
- **Scopes.** While its program runs, its extension is enabled, a catalog is
  open and the source is on for it, the source has a **scope**: the program
  hears `source-started` with the scope's number and the catalog's name, and
  `source-stopped` when the catalog closes or another opens, or the source
  is turned off. A program started again gets its scopes again. A scope's
  number is never used again, so a report from a scope that ended is refused
  (`not_admitted`), however late it arrives.
- **Reports.** The program reports files once they are complete (a capture
  fully written, a copy finished), in its scope, with the folder it puts
  them in ([`source.report`](protocol.md#sourcereport-request)). The app adds
  them as Add Folder adds a folder's new photos: each file's folder placed by
  this computer's folder locations, a folder no catalog folder holds going
  under a new root (the reported folder), with the sidecars', the camera's
  and the default metadata, and Stack Camera Bursts when Preferences has it.
  Each report is one import batch, so Previous Import shows the last one;
  reporting files the catalog has already adds nothing and starts no batch,
  so a source may report again after reconnecting. A file that isn't there,
  isn't a photo, is hidden or outside the folder, or whose folder is linked
  elsewhere or ambiguous, is refused and listed with why; Add Folder… asks
  about an ambiguous one.
- **Busy.** While other work holds the catalog (adding or synchronizing a
  folder, a batch edit, a file operation, another report), a report is
  answered `busy`; the SDK's `Host::report` tries again for up to a minute.
  While a report is added, the same work waits for it.
- **Develop stays.** The Library reads the catalog again in place once the
  photos are added, so the photo open in Develop, its edit, History and any
  save pending are as they were; a Copy Name or metadata field being typed
  in the Library is saved first. A report that asks to be shown (`show`)
  makes the last photo it added the Library's selected one, or, when Develop
  shows a photo, opens it there as a click would: the edit left is saved
  first, and nothing opens when that fails.
- **Stopping.** A report being added is answered `not_admitted` when its
  scope ends, `cancelled` when the program cancels it, and `exited` with its
  session, and then writes nothing, unless its write had begun: then it
  finishes, the Library shows what it added, and the program, if still
  there, is answered with it. Disabling the extension, uninstalling it and
  quitting end its scopes as its program stops.

## Externally launched adapters

A `[[control-adapter]]` with `launch = "external"` isn't started by the app.
Something else starts its program (an MCP client runs `rawmakase mcp`), and the
program reaches the app through the [control socket](../automation.md#json-protocol-version-1),
naming its adapter (`<extension>/<adapter>`) in every request's `adapter`
field. The app runs those requests only while the adapter's extension is
enabled: disabled, even requests already queued and `wait`s already parked are
refused with `adapter_disabled`, so disabling an extension stops a server that
is still running. The command its clients run checks `extensions.json` before
starting the program and fails at once when the extension is disabled. Such a
program isn't supervised: no crash count, deadlines or log file, and quitting
the app doesn't stop it; its requests in flight end as every control request
does. A change the command line makes (`rawmakase extensions disable
rawmakase.mcp`) reaches a running app on its next start (D8); the launcher
reads it at once.

## Where things live

Under the data folder (`RAWMAKASE_DATA_DIR`, else the platform's, as for
`control.json`):

| Path | What |
| --- | --- |
| `extensions/<id>/<version>/` | An installed version, unpacked and verified. Never changed once in place; installing the version again reuses the folder only if it still holds exactly the package's files and the package passes the checks an install unpacking it applies, and otherwise replaces it whole |
| `extensions/.staging-*`, `extensions/.download-*` | An install or a download in progress, or a removed extension's folder being deleted. One a crash left is removed by the host's housekeeping (`Store::clean`, which the app runs at start) once it is an hour old |
| `extensions.json` | What is installed, which version is active, enabled or not, where it came from, its settings, developer folders ([below](#extensionsjson)) |
| `extensions.lock` | Held while `extensions.json` changes |
| `extension-data/<id>/` | The extension's private data, kept across versions and uninstalls; its program's `log.txt` (rotated at 1 MiB) |
| `extension-cache/<id>/` | The extension's cache folder, `hello`'s `cache_dir` |

## `extensions.json`

```json
{
  "version": 1,
  "extensions": {
    "dev.jane.analog": {
      "active": "1.2.0",
      "installed": ["1.1.0", "1.2.0"],
      "enabled": true,
      "source": {"kind": "github", "repository": "https://github.com/jane/rawmakase-analog"},
      "settings": {"default-stock": "HP5"}
    },
    "dev.pch.ai-remove": {
      "active": "0.1.0", "installed": ["0.1.0"], "enabled": true,
      "source": {"kind": "github", "repository": "https://github.com/pch/rawmakase-ai-remove"},
      "settings": {"provider": "fal.ai"},
      "secrets": ["fal-key"],
      "uploads_allowed": true
    },
    "rawmakase.mcp": {"enabled": false}
  },
  "developer": {
    "dev.jane.analog": {"path": "/home/jane/analog/target/rawmakase-dev", "enabled": true}
  }
}
```

A bundled extension has an entry only once something about it was changed.
An entry's `sources` lists the catalogs each of its sources adds photos to
([Photo sources](#photo-sources)).
`secrets` names the secret settings whose value is kept in the system's
credential store; the values are never in the file. `uploads_allowed` records the
user's answer, asked once, to a retouch generator that uploads.
`source.kind` is `bundled`, `file` (with `path`) or `github` (with
`repository`). The app and `rawmakase extensions` both change the file, each
change under the lock, reading the file again first, so neither overwrites
the other's. Fields RAWmakase doesn't know are kept, and a file written by a
newer RAWmakase (a higher `version`) is never written over.

## Installing and updating

- **From a file**: `rawmakase extensions install analog.rmext`. The package
  must be named `<id>-<version>-<target>.rmext`, be for this computer's target
  (or `any` without a program) and support this RAWmakase version.
- **From GitHub**: `rawmakase extensions install https://github.com/jane/rawmakase-analog`
  reads the repository's releases, newest first (drafts and prereleases
  skipped), and offers the first package for this target, or `any`, whose
  manifest supports this version. It shows what it found before installing.
- **Updates**: `rawmakase extensions check` reads the releases of every
  extension installed from GitHub and lists newer compatible versions, saying
  when one asks for more access. Installing it again from the repository
  updates it; the previous version stays installed until nothing uses it.
- `rawmakase extensions list|remove|enable|disable` do what they say; a running
  RAWmakase notices a change made from the command line when Preferences >
  Extensions opens, or on its next start. `enable` and `disable` refuse an id
  that is neither installed, a developer folder nor bundled with the app. A
  removed extension's folder that can't be deleted yet (on Windows, while its
  program runs) is deleted at a later start.

An install unpacks into a staging folder, checks every file against the
package's `SHA256SUMS`, and only then renames the folder into place and makes
that version active, in one change of `extensions.json`. An install that fails
or is interrupted leaves the previous version installed and active.

## Developer mode

`cargo rawmakase dev` builds the extension, lays it out as its package would
unpack under `target/rawmakase-dev/`, and records that folder in
`extensions.json`'s `developer` table. The app runs developer extensions
without installing them. In Preferences > Extensions, **Developer mode** adds
**Load from Folder…**, and while the page is open it looks every second for a
changed manifest or program of a developer extension and loads it again,
stopping its program, which starts again as the new build at its next request.
A folder whose manifest no longer reads (or names another extension) stays
listed with what is wrong, without its commands, and is loaded again once its
manifest is fixed.

## In the app

**Preferences > Extensions** lists every extension with its source (Bundled,
File, GitHub, Developer folder), its state (Enabled, Disabled, Running, Failed,
Can't run) and an **Update** badge after Check for Updates found one. An
extension's page shows its description, author, repository, declared access,
its commands (each with **Run**), settings, panels, control adapters, photo
sources (each turned on or off for the open catalog), its last errors and its
log file; and **Enable**, **Disable**, **Stop** (while a command
runs), **Retry** (when Failed), **Uninstall** (not for a bundled one) and
**Update**. **Install from GitHub…** downloads the newest compatible package,
and **Install from File…** reads a `.rmext`; either shows the package's
manifest, its access and the **Not reviewed** warning, and installs it only
when the user confirms (a file is read again then, and refused if it changed).
An installed extension's page and its line in the list say **Not reviewed**
too. **Available** lists the extensions RAWmakase knows of but doesn't bundle,
while they aren't installed (AI Remove, `dev.pch.ai-remove`): **Install…** finds
the latest release of its repository as Install from GitHub… does, with the same
confirmation and Not reviewed mark. Every change is
written to `extensions.json` first; when that fails, the page says so and
nothing changes.

A program starts at its first request (`on-demand`), or after the app's first
frame (`with-app`), and an `on-demand` program with nothing to do for 5 minutes
is stopped. A request it doesn't answer in 30 seconds (5 for `hello`) is
cancelled, then failed 2 seconds later. A program that exits on its own counts
as a crash: whatever it was asked fails, and its status-line entry and panels
clear. So does one the app hangs up for breaking the protocol, or for holding
a cancelled request's buffers past their grace ([leases](protocol.md#leases)):
the app ends it first. Three crashes in a row make the extension **Failed** for the rest of the
session, until **Retry**; only useful work (a command or a panel answered
successfully, a device operation that ran, or 5 minutes running) starts the
count again; a handshake or a device's report doesn't. A `with-app` program
that ends is started again, until it is Failed. Disabling an
extension cancels its requests, stops its program and takes its commands and
panels away. Quitting sends `quitting` and ends any program still running
within the quit deadline, with the helpers it started.

**Commands** join the app's commands as `<extension>/<command>`: Preferences >
Shortcuts lists them under Extensions, they take keys there like any other
command (an extension's default key included, conflicts shown), and
`rawmakase control action <extension>/<command>` runs one. A command that
applies to the open photo or the selection gets it, read as it starts (photos
not open in Develop are read off the UI thread, and the command is sent once
they are; Stop or disabling the extension meanwhile cancels it). The
manifest's `places` are checked, but menus and toolbars don't show extension
commands yet: keys, `control action` and the Run button on the extension's
page run them. Run closes Preferences first, so the command's change to the
open photo is made and seen rather than refused behind a dialog.

**Settings** are drawn from the manifest's `[[setting]]`s, stored in
`extensions.json`, sent in `hello` and, when changed, as `setting-changed`. A
change is committed once: a slider or number when it is let go or a value
typed into it lands, a text field on Enter or when it loses focus.
**Panels** are drawn on the extension's page from the controls its program
returns.

**Mask generators** add tiles to the Masking drawer, beside Subject, Sky and
Background, and to a mask's Add, Subtract and Intersect menus. A tile asks its
generator for a new mask (inverted for a Background-style tile); one that takes
`prompt` is aimed with clicks on the photo (Alt-click to leave out), a dragged
box, or both, each change refining the same mask, one request at a time. The
app renders the photo for it, keeps the coverage as a raster mask stored with
the edit, and makes one History step of each result; a result arriving after
the photo, its masks or its spots changed is dropped. A generator that answers
`needs_setup` shows a card with what it needs and its size; Download sends
`mask.setup`, with its progress and Cancel, and the selection goes on once it
is done. Refine and Regenerate find the generator from the provenance a mask
saved. A disabled, Failed or missing extension's tiles go, and what would
refine or regenerate its masks shows disabled with why; the masks themselves
render as before, labelled from their provenance.

**Reads and proposals**: a program reads the selection, the open photo or
photos by id, with a token naming what it read, and proposes settings with
that token while its command runs. A photo changed since (another edit, its
file replaced, unsaved changes in Develop), or a catalog closed or read again,
is refused, and the reply lists which photos changed and which were refused.
Each photo changed is one History step, named after the command, and the
whole proposal is one Undo. Photos not open in Develop are changed in the
background, as a batch edit is, with its window and Cancel.

## Decisions

The design's defaults for phase 2 are recorded in the
[design](../extensions-plan.md#phase-2-decisions).
