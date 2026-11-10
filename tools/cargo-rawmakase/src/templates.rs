//! What `cargo rawmakase new` writes, and the release workflow template.

/// The RAWmakase versions a new extension states it supports. Raise it with
/// each minor release: before 1.0 no compatibility is promised, so a new
/// extension supports the minor version it was made for.
pub(crate) const DEFAULT_RANGE: &str = ">=0.4, <0.5";

/// The new extension's files: (path, contents).
pub(crate) fn files(
    adapter: bool,
    name: &str,
    id: &str,
    rawmakase: &str,
) -> Vec<(&'static str, String)> {
    let title = name
        .split('-')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    let common = format!(
        r#"[extension]
id = "{id}"
name = "{title}"
version = "0.1.0"
rawmakase = "{rawmakase}"
api = 1
description = "What {title} does, in a sentence or two."
"#
    );
    let (manifest, main) = if adapter {
        (
            format!(
                r#"{common}
[access]
devices = []

[program]
path = "bin/{name}"
start = "with-app"

[[control-adapter]]
id = "device"
name = "{title}"
"#
            ),
            ADAPTER_MAIN.to_string(),
        )
    } else {
        (
            format!(
                r#"{common}
[program]
path = "bin/{name}"

[[command]]
id = "hello"
title = "Say Hello"
places = ["extensions.menu"]
applies-to = "selection"
"#
            ),
            COMMAND_MAIN.to_string(),
        )
    };
    let cargo = format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
rawmakase-extension = {{ git = "https://github.com/pch/rawmakase" }}
serde_json = "1"
"#
    );
    vec![
        ("Cargo.toml", cargo),
        (rawmakase_extension_protocol::manifest::FILE, manifest),
        ("src/main.rs", main),
        (".gitignore", "/target\n".to_string()),
    ]
}

const COMMAND_MAIN: &str = r#"//! A RAWmakase extension with one command.
use rawmakase_extension::protocol::method::{CommandRun, Level};
use rawmakase_extension::{Context, Extension, RemoteError};
use serde_json::{Value, json};

/// Says how many photos are selected. A command that edits reads its
/// photos, then proposes settings with `cx.host().propose_settings`.
fn hello(cx: &Context, _run: CommandRun) -> Result<Value, RemoteError> {
    let selection = cx.host().read_selection()?;
    let text = format!("Hello from {} photos", selection.photos.len());
    cx.host().notify(Level::Info, &text)?;
    Ok(json!({ "photos": selection.photos.len() }))
}

/// The extension: one handler per contribution its manifest declares.
fn extension() -> Extension {
    Extension::new().command("hello", hello)
}

fn main() -> std::io::Result<()> {
    extension().run()
}

#[cfg(test)]
mod tests {
    use rawmakase_extension::mock::MockHost;
    use serde_json::json;

    #[test]
    fn hello_counts_the_selection() {
        let host = MockHost::start_with(super::extension(), |method, _| match method {
            "read.selection" => Ok(json!({"token": "t", "photos": [{"id": 1, "path": "/a.dng"}]})),
            _ => Ok(json!(null)),
        });
        host.hello().unwrap();
        let reply = host.run_command("hello", None, json!(null)).unwrap();
        assert_eq!(reply, json!({"photos": 1}));
        host.finish().unwrap();
    }
}
"#;

const ADAPTER_MAIN: &str = r#"//! A RAWmakase extension with a control adapter: a device's input becomes
//! editing operations.
use rawmakase_extension::protocol::method::{DeviceState, Operation};
use rawmakase_extension::{ControlAdapter, Extension, Host, RemoteError};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Watches the device on a thread of its own and reports what it does.
#[derive(Default)]
struct Device {
    stop: Arc<AtomicBool>,
}

impl ControlAdapter for Device {
    fn start(&mut self, host: Host) -> Result<(), RemoteError> {
        let stop = self.stop.clone();
        std::thread::spawn(move || {
            // Each connection is a new epoch: operations of an earlier one are
            // refused once the device disconnected.
            let epoch = 1;
            if host.device("device", "example", DeviceState::Connected, epoch).is_err() {
                return;
            }
            while !stop.load(Ordering::SeqCst) {
                // Read the device here; this one turns Exposure one tick a
                // second.
                std::thread::sleep(std::time::Duration::from_secs(1));
                let ops = vec![Operation::Adjust { param: "exposure".into(), ticks: 1 }];
                if host.operate("device", "example", epoch, ops).is_err() {
                    break;
                }
            }
            let _ = host.device("device", "example", DeviceState::Disconnected, epoch);
        });
        Ok(())
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn main() -> std::io::Result<()> {
    Extension::new().adapter("device", Device::default()).run()
}
"#;

/// `.github/workflows/rawmakase-release.yml`: on a `v*` tag, build and package
/// the extension on each platform RAWmakase releases for and attach the
/// packages to the tag's release.
pub(crate) const RELEASE_WORKFLOW: &str = r#"# Written by `cargo rawmakase release`. On a tag v<version>, builds the
# extension on every platform RAWmakase releases for, packages it with
# `cargo rawmakase package` and attaches the packages to the tag's release,
# where RAWmakase's Install from GitHub finds them.
name: Release extension

on:
  push:
    tags: ['v*']

permissions:
  contents: write

jobs:
  package:
    strategy:
      fail-fast: false
      matrix:
        include:
          - { os: ubuntu-latest, target: linux-x86_64 }
          - { os: ubuntu-24.04-arm, target: linux-aarch64 }
          - { os: macos-15, target: macos-aarch64 }
          - { os: macos-15-intel, target: macos-x86_64 }
          - { os: windows-2025, target: windows-x86_64 }
          - { os: windows-11-arm, target: windows-aarch64 }
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - name: Rust
        shell: bash
        run: |
          rustup toolchain install stable --profile minimal
          rustup default stable
          cargo install --locked --git https://github.com/pch/rawmakase cargo-rawmakase
      - run: cargo rawmakase build
      # Apple silicon runs only signed programs: an ad-hoc signature is enough
      # for a program RAWmakase downloads itself.
      - if: runner.os == 'macOS'
        shell: bash
        run: |
          for program in target/release/*; do
            if [ -f "$program" ] && [ -x "$program" ]; then codesign --force --sign - "$program"; fi
          done
      - run: cargo rawmakase package --target ${{ matrix.target }}
      - name: Attach to the release
        shell: bash
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          gh release view "$GITHUB_REF_NAME" >/dev/null 2>&1 || gh release create "$GITHUB_REF_NAME" --title "$GITHUB_REF_NAME" --notes ""
          gh release upload "$GITHUB_REF_NAME" target/rmext/*.rmext --clobber
"#;
