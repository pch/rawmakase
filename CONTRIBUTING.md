# Contributing to RAWmakase

Thanks for helping. This page is the way in: how to build, where things are, what
a change must keep and how to send it. Coding agents follow [AGENTS.md](AGENTS.md),
which holds the same rules in more detail; people can read it too.

## Build and run

You need current stable Rust (the minimum is in `Cargo.toml`'s `rust-version`) and a C
compiler; no RAW or colour-management library is needed. On macOS use a native
rustup toolchain (`aarch64-apple-darwin` on Apple Silicon). See the README's
[From source](README.md#from-source-linux-and-macos) for each system.

```sh
cargo run --release            # the desktop app
cargo run --release -- --help  # the command line
```

## Find your way

- [docs/README.md](docs/README.md): every page, and whether it describes the code
  today (Current), records a past measurement (Record) or proposes work (Plan).
- [Code map](docs/code-map.md): where each thing lives and where a change starts.
- [Architecture](docs/architecture.md): the crates, which may depend on which, the
  threads and jobs, and the lifecycle and compatibility contracts.
- [Glossary](docs/glossary.md): recipe, raw defaults, stand-in, generation and the
  other words the code uses.
- [Adding an adjustment](docs/howto/add-adjustment.md): a new develop setting from
  the model to its tests.

## What a change must keep

- **Saved edits keep working.** Catalogs, presets, sidecars and `session.json` are
  read by this release and by older ones. Unknown fields survive; saved slider
  values keep their meaning; Lightroom settings mean what they mean in Lightroom.
- **Domain code stays free of the UI.** Parsing, persistence and rendering never
  import the desktop app; the crate edges are checked by `scripts/deps.py`.
- **Background work always finishes.** Failure, panic, cancellation and
  disconnection clear busy state; a result for a photo that changed is dropped.
- **Behaviour changes and refactors are separate pull requests.** A bug fix starts
  with a test that fails without it.
- **Rendering references are evidence.** Don't re-bless snapshots or Camera Raw
  baselines to make a change pass; when output changes on purpose, say why.

## Tests and checks

```sh
make check   # dependency checks, formatting, clippy and tests
```

`make check` runs the dependency and crate checks, `cargo fmt --check`, clippy with
warnings denied and the crates' tests; the [Makefile](Makefile) lists them. CI
(`.github/workflows/ci.yml`) also runs what one computer cannot: the Windows, Arch
and Ubuntu lanes, among others. Run one crate's tests with
`cargo test --locked -p rawmakase-model` and so on.

Tests that need private files (your own RAWs, profiles, Lightroom catalogs) or a
GPU are ignored by default; the README's [Development](README.md#development)
section lists them and how to run them. Rendering changes are checked against Camera
Raw on the colour corpus: read [tests/corpus/README.md](tests/corpus/README.md)
first.

## Sending a change

- Open an issue first for anything large, so the approach can be agreed.
- One concern per pull request. Say what changed, why, and how you checked it,
  including what you could not check (no GPU, no private corpus).
- Commit only the files you changed. Never commit RAW files, Adobe profiles,
  Lightroom catalogs, private photos or paths from your own machine.
- Update the docs a change makes wrong in the same pull request: the code map when
  a module moves, the architecture guide when ownership or a dependency changes.

## Licence

RAWmakase is MIT licensed ([LICENSE](LICENSE)); contributions are accepted under the
same licence. Third-party code, data and models keep their own licences, recorded in
[dependencies](docs/dependencies.md); adding one means recording its terms there.
