# Extensions

Status: Partly built. The manifest, packages, installation, `extensions.json`,
the extension protocol, the SDK and `cargo rawmakase` exist
([design](../extensions-plan.md), phase 2); the app doesn't run extensions yet.
Supervising programs, Preferences > Extensions, commands, settings and panels
in the app, and moving MIDI and MCP out come next.

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
| `extensions/.staging-*`, `extensions/.download-*` | An install or a download in progress. One a crash left is removed by the host's housekeeping (`Store::clean`, which the app runs at start) once it is an hour old |
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
  RAWmakase notices a change made from the command line on its next start.
  `enable` and `disable` refuse an id that is neither installed, a developer
  folder nor bundled with the app.

An install unpacks into a staging folder, checks every file against the
package's `SHA256SUMS`, and only then renames the folder into place and makes
that version active, in one change of `extensions.json`. An install that fails
or is interrupted leaves the previous version installed and active.

## Developer mode

`cargo rawmakase dev` builds the extension, lays it out as its package would
unpack under `target/rawmakase-dev/`, and records that folder in
`extensions.json`'s `developer` table. The app runs developer extensions
without installing them (once the app supervises extensions).

## Decisions

The design's defaults for phase 2 are recorded in the
[design](../extensions-plan.md#phase-2-decisions).
