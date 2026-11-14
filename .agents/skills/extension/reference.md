# Extension reference: code sketches and the repo checklist

Every sketch below was compiled and its tests run against the SDK on `main`,
in a crate scaffolded with `cargo rawmakase new`. If a signature moves, check
`crates/rawmakase-extension/src/lib.rs` (traits), `run.rs` (`Extension`,
`Context`, `ServeOptions`), `host.rs` (`Host`) and `mock.rs` (`MockHost`),
and the protocol types in `crates/rawmakase-extension-protocol/src/`
(`method.rs`, `controls.rs`, `buffer.rs`, `viewer.rs`).

The manifest the sketches go with (`api = 2` only because of the mask
generator):

```toml
[extension]
id = "dev.me.sketches"
name = "Sketches"
version = "0.1.0"
rawmakase = ">=0.4, <0.5"
api = 2

[program]
path = "bin/sketches"

[[setting]]
id = "step"
name = "Step"
control = { slider = [0, 2] }
default = 0.5
step = 0.05
unit = "EV"

[[command]]
id = "brighten"
title = "Brighten"
places = ["extensions.menu"]
applies-to = "selection"

[[panel]]
id = "strength"
title = "Strength"
place = "preferences"

[[mask-generator]]
id = "left"
input = ["none", "prompt"]
prompt-hint = "Click what to keep"
[[mask-generator.tile]]
name = "Left Half"
[[mask-generator.tile]]
name = "Right Half"
invert = true
```

`Cargo.toml` is the template's: `rawmakase-extension` from git, and
`serde_json = "1"`.

```rust
use rawmakase_extension::protocol::controls::{Control, ControlKind};
use rawmakase_extension::protocol::method::{CommandRun, EmptyMask, MaskGenerated, SettingsChange};
use rawmakase_extension::{Context, ErrorCode, Extension, MaskAsk, MaskGenerator, Panel, RemoteError};
use serde_json::{Map, Value, json};
```

## Command

A function or closure `FnMut(&Context, CommandRun) -> Result<Value, RemoteError>`
is a `Command`. `run.target` is the snapshot that `applies-to` names (`None`
for `none`). `run.input` is always `Value::Null` for now.

```rust
/// Raises Exposure by the `step` setting on every photo of the selection.
fn brighten(cx: &Context, run: CommandRun) -> Result<Value, RemoteError> {
    // `applies-to = "selection"`: the app read the selection as the command started.
    let target = run
        .target
        .ok_or_else(|| RemoteError::new(ErrorCode::InvalidRequest, "no target"))?;
    let step = cx.settings().get("step").and_then(Value::as_f64).unwrap_or(0.5);
    let mut changes = Vec::new();
    for (i, photo) in target.photos.iter().enumerate() {
        if cx.is_cancelled() {
            return Err(RemoteError::new(ErrorCode::Cancelled, "cancelled"));
        }
        cx.progress(Some(i as f64 / target.photos.len() as f64), None);
        if photo.error.is_some() {
            continue; // Unreadable settings: a proposal would be refused.
        }
        let exposure = photo.settings.get("exposure").and_then(Value::as_f64).unwrap_or(0.0);
        let mut settings = Map::new();
        settings.insert("exposure".into(), json!(exposure + step));
        changes.push(SettingsChange { photo: photo.id, settings });
    }
    // While the command runs, with the snapshot's token and this request's id.
    let proposed = cx.host().propose_settings(cx.op(), target.token, changes)?;
    Ok(json!({ "applied": proposed.applied, "refused": proposed.refused.len() }))
}
```

The setting names and units are the control layer's (`rawmakase control
capabilities`, or `cx.host().read_capabilities()`). Other reads are
`read_selection()`, `read_open_photo()` and `read_photos(ids)`. Each returns a
new token. Tell the user with `notify(Level::Info, ..)` or `status(Some(..))`.

## Panel

`open` and `changed` return the panel's controls, whole. `clicked` defaults to
refusing. Answer a bad value with the controls, the error in that control's
`help`. To push new controls unasked, use `host.panel_update(id, controls)`.

```rust
#[derive(Default)]
struct Strength {
    value: f64,
}
impl Strength {
    fn controls(&self, error: Option<String>) -> Vec<Control> {
        vec![Control {
            id: Some("strength".into()),
            label: Some("Strength".into()),
            help: error, // A validation error is shown here.
            visible_when: None,
            disabled: false,
            kind: ControlKind::Slider {
                value: self.value,
                min: 0.0,
                max: 1.0,
                step: Some(0.05),
                unit: None,
                centred: false,
            },
        }]
    }
}
impl Panel for Strength {
    fn open(&mut self, _cx: &Context) -> Result<Vec<Control>, RemoteError> {
        Ok(self.controls(None))
    }
    fn changed(&mut self, _cx: &Context, _control: &str, value: Value) -> Result<Vec<Control>, RemoteError> {
        match value.as_f64() {
            Some(v) if (0.0..=1.0).contains(&v) => self.value = v,
            _ => return Ok(self.controls(Some("Between 0 and 1".into()))),
        }
        Ok(self.controls(None))
    }
}
```

The control kinds are in the protocol's [panels table](../../../docs/extensions/protocol.md#panels)
and in `ControlKind` (`controls.rs`).

## Mask generator (API 2)

The SDK validates the request and maps both buffers before calling
`generate`, and unmaps them before it replies. `ask.request.input` is `None`
(automatic) or `Some(ViewerInput::Prompt { clicks, bounds })`, normalised to
the image frame, origin top left.

```rust
/// The left half of the photo, or a disc around each positive click.
struct Left;
impl MaskGenerator for Left {
    fn generate(&mut self, cx: &Context, ask: MaskAsk<'_>) -> Result<MaskGenerated, RemoteError> {
        let c = &ask.request.coverage; // 1-channel u8, the image's size.
        let (w, h, stride) = (c.width as usize, c.height as usize, c.stride as usize);
        for y in 0..h {
            if cx.is_cancelled() {
                return Err(RemoteError::new(ErrorCode::Cancelled, "cancelled"));
            }
            for x in 0..w / 2 {
                ask.coverage[y * stride + x] = 255;
            }
        }
        // `ask.request.input` is `None` for an automatic mask, or the prompt.
        // `ask.image` is 3-channel sRGB u8, `ask.request.image.stride` apart.
        Ok(MaskGenerated {
            model: "left-half@1".into(),
            model_key: "left-half/1".into(),
            empty: if w < 2 { Some(EmptyMask::NothingThere) } else { None },
        })
    }
}
```

- `model` is saved with the mask. `model_key` stands for everything besides
  the pixels the result depends on (weights, contract and processing
  versions), so change it when the output would change.
- A model that isn't installed answers
  `Err(protocol::message::NeedsSetup { what: "the X model".into(), bytes }.error())`.
  Then implement `setup(&mut self, cx, generator)`, reporting
  `cx.progress(..)` and polling cancel. A missing runtime answers
  `ErrorCode::Unavailable`.
- Nothing found: set `empty` (`NoSubject`, `NoSky`, `NothingThere`). The app
  also takes all-zero coverage as nothing found.
- Requests run one at a time. Cache per photo by `ask.request.image_key`.

## Control adapter

`start` runs on the handler thread once the handshake is done. Spawn the
device threads and return. This is the `control-adapter` template's
(`ADAPTER_MAIN` in `tools/cargo-rawmakase/src/templates.rs`), cut down; it
compiles in place of the template's. `extensions/midi` is the real one:

```rust
impl ControlAdapter for Device {
    fn start(&mut self, host: Host) -> Result<(), RemoteError> {
        let stop = self.stop.clone();
        std::thread::spawn(move || {
            let epoch = 1; // Raise on every reconnection.
            if host.device("device", "example", DeviceState::Connected, epoch).is_err() { return; }
            while !stop.load(Ordering::SeqCst) {
                // Block on the device's next input here, with a timeout so `stop` is seen.
                let ops = vec![Operation::Adjust { param: "exposure".into(), ticks: 1 }];
                if host.operate("device", "example", epoch, ops).is_err() { break; }
            }
            let _ = host.device("device", "example", DeviceState::Disconnected, epoch);
        });
        Ok(())
    }
    fn stop(&mut self) { self.stop.store(true, Ordering::SeqCst); }
}
```

The operations are `Adjust { param, ticks }`, `Value { param, value: 0..=127 }`,
`Photo { ticks, detent }` and `Action { spec, advance }`. Check a spec a user
typed with `host.read_action(spec)`. An adapter with `launch = "external"`
(MCP) doesn't use this protocol: its program uses the control socket
([overview](../../../docs/extensions/overview.md#externally-launched-adapters)).

## Photo source (API 3)

A watched folder: its own manifest, since a source needs `api = 3` and a
program that starts with the app:

```toml
api = 3

[program]
path = "bin/drop"
start = "with-app"

[[source]]
id = "drop"
name = "Drop folder"
```

`start` runs on the handler thread when a scope starts: once for each catalog
the user turned the source on for, while it is open. Keep one watcher for the
program's life and point it at the scope running now. `complete_files`
stands for your own rule for a finished file (it stopped growing, the camera
said so):

```rust
use rawmakase_extension::protocol::method::{Level, SourceReport, SourceStarted};
use rawmakase_extension::{ErrorCode, Extension, Host, RemoteError, Source};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Watches one folder for the program's life and reports what arrives in
/// whichever scope runs: 0 while none does.
#[derive(Default)]
struct Drop {
    scope: Arc<AtomicU64>,
    watching: bool,
}

impl Source for Drop {
    fn start(&mut self, host: Host, started: SourceStarted) -> Result<(), RemoteError> {
        self.scope.store(started.scope, Ordering::SeqCst);
        if !self.watching {
            self.watching = true;
            let scope = self.scope.clone();
            std::thread::spawn(move || watch(&host, &scope, Path::new("/srv/drop")));
        }
        Ok(())
    }
    fn stop(&mut self, scope: u64) {
        // Only the scope running now; never wait here.
        let _ = self.scope.compare_exchange(scope, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

fn watch(host: &Host, scope: &AtomicU64, folder: &Path) {
    loop {
        // A file is complete once it has stopped growing, or its writer says so.
        let files: Vec<PathBuf> = complete_files(folder);
        let now = scope.load(Ordering::SeqCst);
        if now != 0 && !files.is_empty() {
            for chunk in files.chunks(200) {
                let report = SourceReport {
                    source: "drop".into(),
                    scope: now,
                    folder: folder.into(),
                    files: chunk.to_vec(),
                    show: false,
                };
                match host.report(report) {
                    Ok(reported) => {
                        for refused in reported.refused {
                            let _ = host.notify(Level::Warning, &format!("{}: {:?}", refused.path.display(), refused.reason));
                        }
                    }
                    // The scope stopped, or the app stayed busy for a minute:
                    // report the files again in the next scope or pass.
                    Err(e) if matches!(e.code, ErrorCode::NotAdmitted | ErrorCode::Busy) => break,
                    Err(_) => return, // The session ended.
                }
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn complete_files(_folder: &Path) -> Vec<PathBuf> {
    Vec::new()
}
```

- `host.report` returns once the files are added. `added` counts the new
  photos, `photos` gives each file's photo id (also for files the catalog
  already had) and `refused` the rest.
- A tethering source does the same with `show: true` for each capture.
  Shutter and camera settings go in commands and a panel, as usual.
- Test the scopes with `MockHost::start_with_options(.., ServeOptions { api:
  3 }, ..)` and `host.event(AppEvent::SourceStarted(..))` /
  `SourceStopped(..)`. The responder answers `source.report`, and an
  `Err(RemoteError::new(ErrorCode::Busy, ..))` shows the retry.

## Builder

```rust
fn extension() -> Extension {
    Extension::new()
        .command("brighten", brighten)
        .panel("strength", Strength::default())
        .mask_generator("left", Left)
}

fn main() -> std::io::Result<()> {
    extension().run()
}
```

Keep `extension()` separate from `main` so the tests can build it. Register
one handler for each contribution in the manifest, under the same id. A
request for an id with no handler is answered `invalid_request` ("this
program implements no …"), and a handler whose id isn't in the manifest is
never asked.

## Tests with MockHost

```rust
#[cfg(test)]
mod tests {
    use rawmakase_extension::ServeOptions;
    use rawmakase_extension::mock::MockHost;
    use rawmakase_extension::protocol::PROTOCOL;
    use rawmakase_extension::protocol::buffer::{
        Access, Alpha, Buffer, Meaning, Primaries, Region, Sample, Space, Transfer, White,
    };
    use rawmakase_extension::protocol::method::{Hello, MaskGenerate, Photo, Snapshot};
    use serde_json::{Map, Value, json};

    /// The mode the manifest's `api = 2` makes the app start the program in.
    fn start() -> MockHost {
        MockHost::start_with_options(super::extension(), ServeOptions { api: 2 }, |method, params| {
            match method {
                // Accept every photo of the proposal.
                "propose.settings" => Ok(json!({
                    "applied": params["changes"].as_array().unwrap().iter()
                        .map(|c| c["photo"].clone()).collect::<Vec<_>>(),
                    "refused": [],
                })),
                _ => Ok(Value::Null),
            }
        })
    }

    #[test]
    fn brighten_proposes_for_the_selection() {
        let host = start();
        let mut settings = Map::new();
        settings.insert("step".into(), json!(1.0));
        host.hello_with(Hello {
            protocol: PROTOCOL,
            app_version: "0.4.12".into(),
            extension: "dev.me.sketches".into(),
            settings,
            contributions: vec!["command/brighten".into()],
            data_dir: std::env::temp_dir().join("sketches-data"),
            cache_dir: std::env::temp_dir().join("sketches-cache"),
            session: 1,
        })
        .unwrap();
        let target = Snapshot {
            token: "t".into(),
            photos: vec![Photo {
                id: 7,
                path: "/a.dng".into(),
                settings: Map::from_iter([("exposure".to_string(), json!(0.25))]),
                error: None,
            }],
        };
        let reply = host.run_command("brighten", Some(target), Value::Null).unwrap();
        assert_eq!(reply, json!({"applied": [7], "refused": 0}));
        host.finish().unwrap();
    }

    #[test]
    fn the_panel_validates() {
        let host = start();
        host.hello().unwrap();
        assert_eq!(host.open_panel("strength").unwrap().len(), 1);
        let controls = host.change("strength", "strength", json!(3.0)).unwrap();
        assert_eq!(controls[0].help.as_deref(), Some("Between 0 and 1"));
        host.finish().unwrap();
    }

    /// A mask request for a `w` x `h` image, shaped as the app sends it.
    fn mask_request(w: u32, h: u32) -> MaskGenerate {
        let image = Buffer {
            handle: 0,
            bytes: u64::from(w * h * 3),
            width: w,
            height: h,
            stride: w * 3,
            channels: 3,
            sample: Sample::U8,
            meaning: Meaning::Color {
                primaries: Primaries::Srgb,
                white: White::D65,
                transfer: Transfer::Srgb,
                alpha: Alpha::None,
            },
            region: Region { space: Space::ImageFrame, rect: [0.0, 0.0, 1.0, 1.0], scale: 1.0, overlap: 0 },
            access: Access::Read,
        };
        let coverage = Buffer {
            handle: 1,
            bytes: u64::from(w * h),
            stride: w,
            channels: 1,
            meaning: Meaning::Coverage,
            access: Access::Write,
            ..image
        };
        MaskGenerate { generator: "left".into(), input: None, image, coverage, image_key: "k".into() }
    }

    #[test]
    fn the_mask_is_the_left_half() {
        let host = start();
        host.hello().unwrap();
        let (result, coverage) = host.generate_mask(mask_request(4, 2), &[128; 4 * 2 * 3]).unwrap();
        assert_eq!(result.model, "left-half@1");
        assert_eq!(coverage, [255, 255, 0, 0, 255, 255, 0, 0]);
        host.finish().unwrap();
    }
}
```

- `hello()` uses empty settings and folders under the system's temporary
  folder, which it doesn't create. Use `hello_with` to pass settings, or a
  data folder your code writes to.
- For cancellation, use `send_request(AppRequest::CommandRun(..))` or
  `send_mask(..)`, then `cancel(id)`, then `wait(id)` / `wait_mask(id)`, and
  expect `ErrorCode::Cancelled`. `crates/rawmakase-extension/tests/sdk.rs`
  has both.
- `recorded()` / `wait_for(|r| ..)` show what the program sent besides
  replies: its requests with the answers they got, events (`panel-update`) and
  progress.
- Tests that count `rawmakase_extension::mapped_buffers()` share a
  process-wide counter. Serialise them behind a lock, as `sdk.rs` does with
  `mapping()`.

## Repo-internal checklist

For agents changing a bundled extension in this repo, or adding one. Not
needed by third-party authors. `AGENTS.md` is the policy.

**Dependency rules.** `scripts/deps-crates.txt` lists, crate by crate, the
workspace crates it must never reach (`scripts/deps.py check`), and
`scripts/crate-closures.sh` limits the external closure (no GUI, GPU, native
imaging or inference) for the SDK, the host, `rawmakase-example`,
`rawmakase-midi` and `cargo-rawmakase`. A bundled extension is built as a
third party's: the SDK and its own libraries, never the app, the host
(`rawmakase-extensions`) or another extension. A new crate needs its own
`never` line, with a comment saying why. It also has to be added to the
existing lines that should never reach it: the app's line forbids
`rawmakase-ai-masks`, and the lower layers' lines name every extension crate.
Never loosen a rule to make a change pass.

**Workspace and test lists.** Add the crate to `members` in `Cargo.toml`.
Then add `-p <crate>` where its siblings appear: the `check` target in the
`Makefile` (the `cargo build` of bundled programs before the app's tests, and
the clippy and test lists) and `.github/workflows/ci.yml` (the clippy and test
lines, the `cargo build -p rawmakase-mcp -p rawmakase-midi -p rawmakase-ai-masks`
steps on each platform, and the Linux app-test line that runs
`rawmakase-ai-masks`). If the app's tests run its program, add it as a
dev-dependency of `rawmakase`, as MIDI and AI masks are, with a comment.

**Discovery and packaging.** Follow MIDI:

- Compile its manifest into the app: the `BUNDLED` list in
  `src/app/extensions/mod.rs` (`include_str!` of
  `extensions/<name>/rawmakase-extension.toml`). `crates/rawmakase-extensions/src/bundled.rs`
  parses those manifests and finds each program, by the last part of its
  `program.path`, beside the executable (or in `../lib/rawmakase` on Linux).
  Keep `path` a bare name, `rawmakase-<name>`.
- Install the program beside the app everywhere: the `Makefile` (a `*_BIN`
  variable, `install` and `uninstall`), both `packaging/arch/*/PKGBUILD`,
  `packaging/linux/bundle.py`, `packaging/macos/bundle.py` / `app.sh`,
  `packaging/windows/stage.ps1`, the verify scripts beside them,
  `.github/workflows/packaging.yml` and `release.yml` (which also publishes
  MIDI's `.rmext`). Then add it to the program lists checked by
  `packaging/check_programs.py` (and its test), which checks each program is
  there, built for the right architecture and linked to system libraries
  only. `grep -rn rawmakase-midi packaging .github Makefile` lists every place.
- A bundled extension's `id` is `rawmakase.<name>`, `rawmakase = "*"`, and its
  `version` is the app's. `every_bundled_extension_is_at_the_app_version`
  (`src/app/extensions/tests.rs`) fails otherwise.
- Update `docs/extensions/overview.md` (Bundled extensions),
  `docs/architecture.md` and `docs/code-map.md` when ownership changes.

**Release versions.** A bundled extension's `Cargo.toml` and
`rawmakase-extension.toml` versions, and its pin in the root `Cargo.toml`'s
dev-dependencies, are bumped with every release. Add the new files to the
list in `AGENTS.md` ("Releases", step 1) and in `.agents/skills/release`.

**Before pushing.** Run `make check`, which runs every CI check that can run
here, then follow `ship` for review and the PR. A change that only touches
an extension's docs is docs-only (`[skip ci]`, straight to `main`).
