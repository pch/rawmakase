# Making and publishing an extension

`cargo rawmakase` (`tools/cargo-rawmakase`) makes, builds and packages
extensions. Until it is published to crates.io, install it from this
repository:

```sh
cargo install --locked --git https://github.com/pch/rawmakase cargo-rawmakase
```

## Start

```sh
cargo rawmakase new my-looks --id dev.me.my-looks
cargo rawmakase new my-dial --id dev.me.my-dial --template control-adapter
```

`new` writes a crate with `Cargo.toml` (depending on the SDK,
`rawmakase-extension`, from this repository), `rawmakase-extension.toml` and
`src/main.rs`. The `command` template has one command, placed in the
extension's menu, that reads the selection and says how many photos it holds,
with a test against the SDK's mock host. The `control-adapter` template starts
a device thread that reports a connection and turns Exposure. `--rawmakase`
sets the supported range (the current minor version by default). The other
templates the design names (filter, importer, source, mask generator) come
with their contributions.

## Build and try

- `cargo rawmakase build` is `cargo build --release`, passing on any
  arguments after it.
- `cargo test` runs the extension's tests: `rawmakase_extension::mock::MockHost`
  serves the extension on a thread, sends it requests and events, answers its
  requests with a function of yours, and records what it sent.
- `cargo rawmakase dev [--data-dir <dir>]` builds a debug program, lays it out
  under `target/rawmakase-dev/` as the package would unpack, and registers
  that folder in `extensions.json` as a developer extension. Run it again after
  a change.

## Package

`cargo rawmakase package` writes `target/rmext/<id>-<version>-<target>.rmext`:

- the manifest, the program from `target/release/` at the manifest's
  `program.path` (`--program` names another file), the icon, and any
  `--include <path>` files, at the same paths;
- `SHA256SUMS`, listing every other file in `sha256sum`'s format.

`<target>` is `<os>-<arch>` as RAWmakase's releases name them (`linux-x86_64`,
`linux-aarch64`, `macos-aarch64`, `macos-x86_64`, `windows-x86_64`,
`windows-aarch64`), this computer's unless `--target` says otherwise, or `any`
for an extension without a program. The tool checks the package as RAWmakase
will before it says it is done.

RAWmakase refuses a package that is named otherwise, is for another target,
doesn't support its version, holds a file missing from `SHA256SUMS` or listed
there but missing, holds a digest that doesn't match, an absolute path, `..`,
a link, two names differing only in case, or more than 512 MiB unpacked. It
also refuses, on every platform, names Windows would read as something else: a
name ending in a dot or a space (Windows drops them, so `readme.` is `readme`),
and a name whose part before the first dot is a device name such as `CON`,
`NUL`, `COM1` or `LPT1`, in any case.

## Release on GitHub

`cargo rawmakase release` builds and packages for this computer, and writes
`.github/workflows/rawmakase-release.yml` (unless it exists). Push a tag
`v<version>` and the workflow builds the extension on every target, ad-hoc
signs the macOS programs (Apple silicon runs only signed code; notarizing is
your choice), packages each, and attaches them to the tag's release.

Users install it with **Install from GitHub…** or
`rawmakase extensions install https://github.com/<owner>/<repo>`: RAWmakase
reads the releases newest first, skipping drafts and prereleases, and installs
the first package for their target (or `any`) whose `rawmakase` range includes
their version. Keep older releases up for users of older RAWmakase versions.
Check for Updates reads the same releases, and says when a newer version's
`[access]` asks for more.

Extensions installed this way are marked **Not reviewed**. An official
catalogue of reviewed extensions comes later.
