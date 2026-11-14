---
name: extension
description: Create, test, package and publish a RAWmakase extension with the Rust SDK and cargo rawmakase. Use for "write an extension", "new RAWmakase extension", "add a command/panel/mask generator/control adapter/photo source", "watched folder", "tethering", "cargo rawmakase new", "package my extension", "publish an extension", rawmakase-extension.toml manifest errors, MockHost tests, or work on a bundled extension (MIDI, MCP, AI masks) in this repo.
---

# Make a RAWmakase extension

An extension is a package with one manifest, `rawmakase-extension.toml`, and
usually one native program written with the SDK (`crates/rawmakase-extension`).
The program talks to the app over the extension protocol. The docs are the
policy. This skill is the order of operations and the traps. Where they
disagree, the docs and the types win:

- [overview](../../../docs/extensions/overview.md): trust, where things live,
  developer mode, installing, how the app runs programs;
- [manifest](../../../docs/extensions/manifest.md) and
  [its schema](../../../docs/extensions/manifest.schema.json), generated from
  `crates/rawmakase-extension-protocol/src/manifest.rs`;
- [protocol](../../../docs/extensions/protocol.md): deadlines, buffers,
  leases, proposals, error codes;
- [publishing](../../../docs/extensions/publishing.md): `cargo rawmakase`.

[reference.md](reference.md) has the code sketches, all compiled and tested
against the SDK, and the repo-internal checklist.

**Released versions.** No tagged release has extensions yet: v0.4.12 predates
them. Until one does, try an extension with an app built from `main`.

## 1. Start

```sh
cargo install --locked --git https://github.com/pch/rawmakase cargo-rawmakase
cargo rawmakase new my-looks --id dev.me.my-looks                       # a command
cargo rawmakase new my-dial --id dev.me.my-dial --template control-adapter
```

- The folder name must be lowercase letters, digits and hyphens. It becomes
  the crate's name and the program's (`[program] path = "bin/<name>"`).
- The id is reverse-DNS and permanent, because settings and saved masks store
  it. `rawmakase.*` is reserved: an install of such a package is refused.
- `--rawmakase` sets the supported range. The default is the current minor
  (`DEFAULT_RANGE` in `tools/cargo-rawmakase/src/templates.rs`).
- Only the `command` and `control-adapter` templates exist. For a panel, a
  mask generator or a photo source, start from `command` and follow
  [reference.md](reference.md#panel).
- The template depends on the SDK by git (`rawmakase-extension = { git = ... }`),
  so it builds against `main`.

## 2. Write the manifest

Read [manifest.md](../../../docs/extensions/manifest.md) for every key:
`[extension]`, `[access]`, `[program]`, `[[setting]]`, `[[command]]`,
`[[panel]]`, `[[control-adapter]]`, `[[mask-generator]]` with its
`[[mask-generator.tile]]`s, and `[[source]]`. The rules people hit:

- **Unknown keys are errors** at every level, so a typo fails instead of
  silently turning a feature off. Tables the design plans but no version
  reads yet (`[[filter]]`, `[[importer]]`, …) are refused too.
- **Ids** are lowercase letters, digits and hyphens, at most 64 characters,
  and unique *within* their table. A command and a panel may share an id.
- **`api`**: `1` to `3`. A higher value is refused as "for a newer
  RAWmakase". Use the lowest one that has what you need: `2` adds
  `[[mask-generator]]` and the shared-memory buffers that mask requests
  carry; `3` is API 2 plus `[[source]]`. The app starts the program with
  `RAWMAKASE_EXTENSION_API`, and the SDK serves that mode. API 2 and up
  also make the program's reader strict (step 6).
- **Sources** (watched folders, tethering) need `api = 3` and
  `[program] start = "with-app"`. A source is off until the user turns it on
  for a catalog on the extension's page, so a freshly installed one reports
  nothing.
- **Viewer input.** A command's `input` (`point`, `box`, …) is accepted, but
  the app doesn't collect it yet: `CommandRun::input` is always `null`
  (`src/app/extensions/mod.rs`, `run_command`). Only mask generators get
  viewer input today, as `prompt`. A generator's `input` takes `none` and
  `prompt` only.
- **Mask provenance.** Leave `provenance` out. It defaults to
  `<extension>/<id>`, and any other value is refused. The one exception is the
  bundled `rawmakase.ai-masks`, which may claim `subject` and `sky`.
- Commands, panels, adapters and mask generators all need a `[program]`.
- `places` and panel `place` are checked but not drawn yet. Commands run from
  keys, `rawmakase control action <ext>/<cmd>` and the Run button on the
  extension's page. Panels show on that page.
- `[access]` is declared, not enforced. Declare the truth: users see it
  before installing, and again when an update asks for more.

`cargo rawmakase build`, `package` and `dev` all parse the manifest first and
print the exact rule that failed.

## 3. Write the program

Use one builder, with one handler for each contribution id in the manifest:

```rust
Extension::new()
    .command("brighten", brighten)          // impl Command, or a closure
    .panel("strength", Strength::default()) // impl Panel
    .adapter("device", Device::default())   // impl ControlAdapter
    .mask_generator("left", Left)           // impl MaskGenerator
    .source("drop", Drop::default())        // impl Source
    .on_event(|event, host| { /* AppEvent */ })
    .run()
```

`Context` gives a handler `host()` (reads, proposals, `notify`, `status`),
`op()` (this request's id, for proposals), `is_cancelled()` and
`cancel_flag()`, `settings()` (with every `setting-changed` applied),
`data_dir()`, `cache_dir()` and `progress(done, text)`. An adapter or a
source gets only a `Host`, which has `data_dir()`, `session()` and, for a
source, `report(..)`. The signatures are in
`crates/rawmakase-extension/src/lib.rs`, `run.rs` and `host.rs`. Worked code
is in [reference.md](reference.md).

## 4. Test it

`cargo test`, with `rawmakase_extension::mock::MockHost` standing in for the
app over the real transport (`crates/rawmakase-extension/src/mock.rs`):

- `MockHost::start(ext)` answers the program's requests with `null`.
  `start_with(ext, |method, params| …)` answers them your way.
- An `api = 2` or `3` manifest needs
  `start_with_options(ext, ServeOptions { api: 2 }, responder)` (or
  `api: 3`). A mock
  serving API 1 refuses `generate_mask` at once, as the app does.
- `hello()` or `hello_with(Hello { settings, .. })`, then `run_command`,
  `open_panel` / `change` / `click`, `generate_mask(request, &image)` (it
  returns the result and the coverage written), `send_mask` + `cancel` +
  `wait_mask`, `setup_mask`, `event` (a source's scopes start and stop
  through `AppEvent::SourceStarted` / `SourceStopped`), `recorded()` /
  `wait_for(..)`, and `finish()` last.
- Test the refusals too: cancellation, `needs_setup`, a proposal refused
  `stale_target`.

Worked tests: `crates/rawmakase-extension/tests/sdk.rs`; the example
extension, `extensions/example` (`src/lib.rs` for commands, a setting and a
panel; `src/masks.rs` with `masks/rawmakase-extension.toml` for mask
generators; `src/sources.rs` with `sources/rawmakase-extension.toml`, a
test-only source the app's own tests drive); and the bundled ones: `extensions/midi` (a with-app adapter and
panel, `src/tests.rs`), `extensions/mcp` (an externally launched adapter) and
`extensions/ai-masks` (mask generators with setup, `src/tests.rs`).

## 5. Try it in the app, package, publish

- `cargo rawmakase dev [--data-dir <dir>]` makes a debug build, lays it out
  under `target/rawmakase-dev/` and registers the folder in `extensions.json`
  as a developer extension. In Preferences > Extensions, **Developer mode**
  adds **Load from Folder…** and reloads a changed manifest or program while
  the page is open. Run `dev` again after each change.
- `cargo rawmakase build` runs `cargo build --release`.
  `cargo rawmakase package [--target T] [--program P] [--include F]` writes
  `target/rmext/<id>-<version>-<target>.rmext` and checks it as the app
  would.
- `rawmakase extensions install <file.rmext | https://github.com/o/r> [--yes]`,
  or **Install from File…** / **Install from GitHub…**. Packages installed
  this way are marked **Not reviewed**.
- `cargo rawmakase release` packages for this computer and writes
  `.github/workflows/rawmakase-release.yml`. Push a tag `v<version>` and every
  target's package is attached to the release. Keep older releases up for
  users of older RAWmakase versions.

## 6. Rules and pitfalls

The details are in [protocol.md](../../../docs/extensions/protocol.md).

- **Deadlines.** `hello` gets 5 s, other requests 30 s, `mask.generate` 200 s
  and `mask.setup` an hour. At the deadline the app sends `cancel` and fails
  the request 2 s later. Progress doesn't extend a deadline. An `on-demand`
  program is stopped after 5 idle minutes.
- **One reply per request.** Return from the handler and the SDK replies.
  Never answer twice, and don't keep working after you return.
- **Honour cancel.** Poll `cx.is_cancelled()`, or lend `cx.cancel_flag()` to a
  model run, and return `ErrorCode::Cancelled`. A handler that ignores it holds
  the queue, and with buffers it holds the lease. Past 10 s the app treats
  the program as hung and kills it.
- **Proposals.** Edit only through `host().propose_settings(cx.op(), token,
  changes)` *while the command runs*, with the token of the snapshot you read.
  Once the command is answered, cancelled or past its deadline, a proposal is
  refused `not_admitted`. A photo that changed since the read is refused
  `stale_target`, photo by photo. Read `applied` and `refused`. Skip photos
  whose snapshot has an `error`. A read or proposal covers at most 200
  photos.
- **API 2 framing is strict.** Any framing fault (a malformed or oversized
  line, missing or extra handles) ends the program (exit status 70). Never
  write to the channel yourself. `println!` goes to the log, not the
  channel.
- **Buffers live only for the handler's call.** `MaskAsk`'s `image` and
  `coverage` are borrowed. The SDK unmaps them after `generate` returns and
  before replying, so copy anything you keep. Write the coverage
  `stride` bytes per row. `image_key` is a digest you can cache an analysis
  by.
- **Budgets.** At most 256 MiB of buffers per request and 512 MiB in flight
  per extension, counted across restarts. One JSON line is at most 1 MiB, and
  a longer reply goes out as `too_large`. A panel holds at most 1000
  controls, nested at most 8 deep. A mask's `model` and `model_key` are 1 to
  128 bytes.
- **Data folder versus local data folder.** `cx.data_dir()`
  (`<data>/extension-data/<id>/`, which also holds `log.txt`) and
  `cx.cache_dir()` (`<data>/extension-cache/<id>/`) sit under the data folder.
  `RAWMAKASE_DATA_DIR` may point that at a shared or synced folder. Anything
  big or machine-specific (models, runtimes) belongs in the local data folder,
  `rawmakase_protocol::paths::local_data_dir()`, which is never synced. Only
  AI masks does that today.
- **Don't block the handler loop.** One thread runs every handler and event
  in order, while another keeps reading so cancels and replies still arrive.
  So `ControlAdapter::start` must spawn its threads and return. `on_event`
  must be quick. Long work polls cancel. Device threads report through their
  own `Host` clone and must stop when `stop()` is called.
- **Adapters.** Raise `epoch` on every connection. `adapter.operate` from an
  old epoch is refused `stale_target`. When the app's queue is full, it
  refuses at once with `channel_full`: drop the input or coalesce it, and
  don't retry in a loop.
- **Sources.** Report only complete files: written whole and closed, never
  one still being written. Each report names the scope from
  `source-started`, the folder the source writes to (the files must be in it
  or below it) and 1 to 200 files. `host.report` waits for the files to be
  added: it retries `busy` for up to a minute while other work holds the
  catalog, and it is refused `not_admitted` once the scope has stopped, so
  report from your own thread, never from `start`. Reporting a file again is
  harmless, so after a restart or a reconnection, report everything that
  might be missing. `show: true` selects the last photo added, or opens it in
  Develop, as a tethered capture does. `stop(scope)` must not wait on a
  device. Read `refused`: `not_found`, `not_a_photo`, `outside_folder`,
  `linked_elsewhere` (its folder belongs to a catalog folder linked to
  another place on this computer) or `ambiguous` (the user adds it with Add
  Folder…, which asks). There is no live view and no preset on import yet.
- **Logging.** Stdout and stderr go to `<data>/extension-data/<id>/log.txt`,
  rotated at 1 MiB, and the extension's page links to it. Stdin is null.
- **Errors the app understands.** `NeedsSetup { what, bytes }.error()` shows a
  download card, then `mask.setup`. `Unavailable` covers a missing runtime or
  worker. Three crashes in a row make the extension Failed until Retry.

## 7. Repo-internal: bundled extensions in this repo

Only for agents changing `extensions/midi`, `extensions/mcp`,
`extensions/ai-masks`, `extensions/example` or adding a new bundled one.
Third-party authors can skip this section. `AGENTS.md` applies throughout,
and the checklist is in [reference.md](reference.md#repo-internal-checklist):
`scripts/deps-crates.txt` rules, the `make check` and CI package lists,
`bundled.rs` discovery and the packaging scripts, the release version
locations, then `make check` before pushing.
