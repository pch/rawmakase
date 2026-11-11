# Extensions

Status: Partly built ([design](../extensions-plan.md), phase 2). The manifest,
packages, installation, `extensions.json`, the extension protocol, the SDK and
`cargo rawmakase` exist, and the app runs extensions: it supervises their
programs, and has Preferences > Extensions, their commands, settings and
panels, and reads and proposals with target tokens. Externally launched control
adapters, and moving MCP and MIDI out of the core, come next.

An **extension** adds to RAWmakase without changing it: commands, settings,
panels of controls the app draws, and control adapters (a MIDI device, an MCP
server). It is a package with one manifest, `rawmakase-extension.toml`
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
    "rawmakase.mcp": {"enabled": false}
  },
  "developer": {
    "dev.jane.analog": {"path": "/home/jane/analog/target/rawmakase-dev", "enabled": true}
  }
}
```

A bundled extension has an entry only once something about it was changed.
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
its commands (each with **Run**), settings, panels, control adapters, its last
errors and its log file; and **Enable**, **Disable**, **Stop** (while a command
runs), **Retry** (when Failed), **Uninstall** (not for a bundled one) and
**Update**. **Install from GitHub…** downloads the newest compatible package,
and **Install from File…** reads a `.rmext`; either shows the package's
manifest, its access and the **Not reviewed** warning, and installs it only
when the user confirms (a file is read again then, and refused if it changed).
An installed extension's page and its line in the list say **Not reviewed**
too. Every change is
written to `extensions.json` first; when that fails, the page says so and
nothing changes.

A program starts at its first request (`on-demand`), or after the app's first
frame (`with-app`), and an `on-demand` program with nothing to do for 5 minutes
is stopped. A request it doesn't answer in 30 seconds (5 for `hello`) is
cancelled, then failed 2 seconds later. A program that exits on its own counts
as a crash: whatever it was asked fails, and its status-line entry and panels
clear. Three crashes in a row make the extension **Failed** for the rest of the
session, until **Retry**; only useful work (a command or a panel answered
successfully, or 5 minutes running) starts the count again. Disabling an
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
