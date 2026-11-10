# Manifest reference

Every extension has one manifest, `rawmakase-extension.toml`, at the root of its
package. Its JSON Schema is [manifest.schema.json](manifest.schema.json),
generated from `crates/rawmakase-extension-protocol/src/manifest.rs` (a test
fails when the committed file and the types disagree). This page explains it;
where they differ, the schema and the types are right.

**Unknown keys are an error**, at every level and for whole tables, so a typo
can't silently turn a feature off. The tables this version reads are below; the
design's others (`[[filter]]`, `[[importer]]`, `[[presets]]`, …) are refused
until a version that builds them raises `api`.

```toml
[extension]
id = "dev.jane.analog"
name = "Analog Toolkit"
version = "1.2.0"
rawmakase = ">=0.5, <0.6"
api = 1
authors = ["Jane Doe"]
license = "MIT"
repository = "https://github.com/jane/rawmakase-analog"
description = "Film-stock Auto looks."
icon = "icon.png"

[access]
network = false
files = "photos"
devices = []

[program]
path = "bin/analog"
start = "on-demand"

[[setting]]
id = "default-stock"
name = "Default stock"
control = { choice = ["Tri-X", "HP5", "Portra 400"] }
default = "Tri-X"

[[command]]
id = "auto-film-look"
title = "Auto Film Look"
places = ["library.photo-menu", "library.auto", "develop.auto"]
key = "Ctrl+Alt+F"
applies-to = "selection"

[[panel]]
id = "looks"
title = "Film Looks"
place = "develop"

[[control-adapter]]
id = "mcp"
launch = "external"
```

## `[extension]`

| Key | Required | Meaning |
| --- | --- | --- |
| `id` | yes | Reverse-DNS, two or more parts of lowercase letters, digits and hyphens (`dev.jane.analog`), at most 128 characters. Permanent: settings and edits store it. `rawmakase.*` is reserved for the official extensions |
| `name` | yes | Shown in Preferences |
| `version` | yes | The extension's version, [semver](https://semver.org) (`1.2.0`) |
| `rawmakase` | yes | The RAWmakase versions it supports, a semver range (`>=0.5, <0.6`). Outside it the package is refused, and an installed one is listed as incompatible. Before 1.0 no compatibility is promised, so state the minor versions tested |
| `api` | yes | The manifest format it was written for. This version reads `1`; a higher one is refused as written for a newer RAWmakase |
| `authors` | no | Names |
| `license` | no | An SPDX expression |
| `repository` | no | `https://github.com/<owner>/<repo>` |
| `description` | no | A sentence or two for the extension's page |
| `icon` | no | A PNG in the package (a relative path) |

## `[access]`

What the extension declares it reaches, shown before installing and when an
update asks for more. Not enforced: an extension runs with your permissions.

| Key | Default | Meaning |
| --- | --- | --- |
| `network` | `false` | It uses the network |
| `files` | `"photos"` | `photos` (what the app hands it) or `any` |
| `devices` | `[]` | Kinds of devices, for example `"midi"`, `"usb"`, `"camera"` |

## `[program]`

Needed by every extension with commands, panels or control adapters.

| Key | Default | Meaning |
| --- | --- | --- |
| `path` | — | The executable in the package: relative, `/` between plain names, no `..`, `\` or `:`, and no name Windows reads differently (see [Publishing](publishing.md)). On Windows `.exe` is added when the name has no extension |
| `start` | `"on-demand"` | `on-demand`: at its first request, stopped after a while with nothing to do. `with-app`: with the app, until it quits |

## `[[setting]]`

Settings shown on the extension's page in Preferences, stored in
`extensions.json` and sent to the program in `hello` and `setting-changed`.

| Key | Required | Meaning |
| --- | --- | --- |
| `id` | yes | Lowercase letters, digits and hyphens, unique among the settings |
| `name` | no | The label; the id without one |
| `help` | no | A line under it |
| `control` | yes | How it is edited (below) |
| `default` | yes | Its value until changed; must fit the control |
| `step` | no | A slider's step |
| `unit` | no | A slider's or a number's unit |

`control` is one of: `{ slider = [min, max] }`, `{ number = [min, max] }`,
`"angle"`, `"toggle"`, `{ choice = ["a", "b"] }`, `{ segmented = ["a", "b"] }`,
`"text"`, `"color"` (a default of `[r, g, b]`, 0 to 1), `"file"`, `"folder"`.

## `[[command]]`

Commands join the app's command registry as `<extension>/<command>`, so they can
have keys, run from `rawmakase control action <extension>/<command>`, and run
from the extension's page.

| Key | Default | Meaning |
| --- | --- | --- |
| `id` | — | Lowercase letters, digits and hyphens, unique among the commands |
| `title` | — | Shown in menus and Preferences > Shortcuts |
| `places` | `[]` | Fixed slots it is shown in (below) |
| `key` | none | Its default key in the key map's notation (`Ctrl+Alt+F`); the user's key map overrides it |
| `applies-to` | `"none"` | `none`, `photo` (the open photo) or `selection`; the app refuses the command when that isn't there |
| `input` | `"none"` | Viewer input collected first: `none`, `point`, `points`, `box`, `prompt`, `brush` (accepted now; collected from phase 3) |

`places` takes these names: `library.photo-menu`, `library.toolbar`,
`library.auto`, `develop.auto`, `develop.tool-strip`, `file.import`,
`file.export`, `extensions.menu`. Every name is checked now, so a manifest
written today stays valid; the app draws commands into these slots once an
extension that uses them is built (decision D6 in the
[design](../extensions-plan.md#phase-2-decisions)).

## `[[panel]]`

A panel of host-drawn controls the program fills ([protocol](protocol.md#panels)).

| Key | Meaning |
| --- | --- |
| `id` | Lowercase letters, digits and hyphens, unique among the panels |
| `title` | The panel's title |
| `place` | `develop` (Develop's right column), `library` (the Library's right column), `window` (a window of its own) or `preferences` (the extension's page) |

This release draws every panel on the extension's page in Preferences >
Extensions, whatever its `place`; the other places are checked and will be
drawn once an extension that uses them is built.

## `[[control-adapter]]`

An adapter translating a device or a protocol into commands.

| Key | Default | Meaning |
| --- | --- | --- |
| `id` | — | Unique among the adapters; control requests name it as `<extension>/<id>` |
| `name` | the id | Shown in Preferences |
| `launch` | `"app"` | `app`: the extension's program, started by the app (MIDI). `external`: started by something else, such as an AI agent starting an MCP server; it reaches the app through the control socket and is not supervised by it |
