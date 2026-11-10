PREFIX ?= $(HOME)/.local
# Package name for the license directory (Arch packages pass their pkgname).
PKGNAME ?= rawmakase
BIN := target/release/rawmakase
# The bundled MCP extension's program, which `rawmakase mcp` runs. The app
# looks for its bundled programs beside its executable and, on Linux, in
# ../lib/rawmakase (crates/rawmakase-extensions/src/bundled.rs).
MCP_BIN := target/release/rawmakase-mcp
ifeq ($(shell uname -s),Darwin)
PROGRAM_DIR = $(PREFIX)/bin
else
PROGRAM_DIR = $(PREFIX)/lib/rawmakase
endif

.PHONY: all build check check-python check-msrv check-tools install uninstall
all: build

build:
	cargo build --release --locked -p rawmakase -p rawmakase-mcp

# The minimum Rust version Cargo.toml declares, compiled as CI's lint job does.
MSRV := $(shell sed -n 's/^rust-version = "\(.*\)"/\1/p' Cargo.toml)
# rustup selects that toolchain with `cargo +$(MSRV)`. Where rustup's proxy is
# unavailable, set MSRV_BIN to the toolchain's bin folder instead, for example
# MSRV_BIN=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin; its cargo
# and rustc are then run by path, and must report Rust $(MSRV).
MSRV_BIN ?=
# The camera calibration tests need numpy and Pillow; they are installed into
# this virtual environment on the first run, as CI does.
CHECK_VENV ?= target/check-venv

# Every CI check (.github/workflows/ci.yml) that runs on this machine alone.
# Not covered here, and left to CI:
# - the tests on the other platforms (Linux x86_64 and aarch64, Windows,
#   macOS): this machine runs only its own platform's code;
# - the HEIC pixel tests that CI requires (RAWMAKASE_REQUIRE_HEIF, Arch's
#   libheif) and the exported-metadata tests that require exiftool: they skip
#   here when libheif or exiftool is missing;
# - the build without LibRaw or Little CMS installed (the "crates" job),
#   which only proves something on a machine without them;
# - the Arch package build and install, and the portable catalog SQL
#   prepared on Postgres, which need an Arch container and a Postgres server.
# cargo deny and desktop-file-validate run when installed (check-tools).
# The tests read an empty data folder, not this machine's: installed camera
# profiles change what Develop resolves, and saved export settings would send
# Export with Previous somewhere real.
# rawmakase-mcp is built before the app's tests: they run `rawmakase mcp`,
# which runs the program found beside it in target/debug.
TEST_DATA = $(CURDIR)/target/test-data
TEST_ENV = RAWMAKASE_DATA_DIR="$(TEST_DATA)" XDG_DATA_HOME="$(TEST_DATA)"

check: check-python check-tools check-msrv
	rm -rf -- "$(TEST_DATA)" && mkdir -p -- "$(TEST_DATA)"
	python3 scripts/deps.py check
	scripts/crate-closures.sh
	cargo fmt --check
	cargo clippy --locked --workspace --all-targets -- -D warnings
	cargo build --locked -p rawmakase-mcp
	$(TEST_ENV) cargo test --locked -p rawmakase -p rawmakase-decode -p rawmakase-export
	cargo clippy --locked --all-targets -p rawmakase-cancel -p rawmakase-fs -p rawmakase-tiff -p rawmakase-raw -p rawmakase-model -p rawmakase-interop -p rawmakase-catalog -p rawmakase-engine -p rawmakase-protocol -p rawmakase-remote -p rawmakase-ctl -p rawmakase-mcp -p rawmakase-extension-protocol -p rawmakase-extension -p rawmakase-extensions -p cargo-rawmakase -p rawmakase-example -- -D warnings
	$(TEST_ENV) cargo test --locked -p rawmakase-model --all-features
	$(TEST_ENV) cargo test --locked -p rawmakase-cancel -p rawmakase-fs -p rawmakase-tiff -p rawmakase-raw -p rawmakase-interop -p rawmakase-catalog -p rawmakase-engine
	$(TEST_ENV) cargo test --locked -p rawmakase-protocol -p rawmakase-remote -p rawmakase-ctl -p rawmakase-mcp -p rawmakase-inference -p rawmakase-model-install -p rawmakase-extension-protocol -p rawmakase-extension -p rawmakase-extensions -p cargo-rawmakase -p rawmakase-example
	RUSTDOCFLAGS="-D warnings -A rustdoc::private_intra_doc_links" cargo doc --workspace --all-features --no-deps --locked --document-private-items

check-msrv:
	@test -n "$(MSRV)" || { echo "Cargo.toml declares no rust-version"; exit 1; }
ifeq ($(MSRV_BIN),)
	cargo +$(MSRV) check --locked --all-targets
else
	@"$(MSRV_BIN)/rustc" -V | grep -q '^rustc $(MSRV)[. ]' || { echo "$(MSRV_BIN)/rustc is not Rust $(MSRV)"; exit 1; }
	RUSTC="$(MSRV_BIN)/rustc" "$(MSRV_BIN)/cargo" check --locked --all-targets
endif

check-python: $(CHECK_VENV)/.installed
	python3 scripts/corpus/shadow-context.py --check
	$(CHECK_VENV)/bin/python -m unittest discover -s scripts/cameras
	$(CHECK_VENV)/bin/python -m unittest discover -s scripts/corpus

$(CHECK_VENV)/.installed:
	python3 -m venv $(CHECK_VENV)
	$(CHECK_VENV)/bin/pip install --quiet --disable-pip-version-check numpy pillow
	touch $@

check-tools:
	@if command -v cargo-deny >/dev/null; then cargo deny check; \
	else echo "cargo-deny is not installed: the license and advisory audit runs only in CI"; fi
	@if command -v desktop-file-validate >/dev/null; then desktop-file-validate packaging/applications/rawmakase.desktop; \
	else echo "desktop-file-validate is not installed: the desktop entry is validated only in CI"; fi

# Install does not rebuild, so `make && sudo make install PREFIX=/usr` never compiles as root.
install:
	@test -x $(BIN) || { echo "Run 'make' first to build $(BIN)"; exit 1; }
	@test -x $(MCP_BIN) || { echo "Run 'make' first to build $(MCP_BIN)"; exit 1; }
	install -Dm755 $(BIN) $(DESTDIR)$(PREFIX)/bin/rawmakase
	install -Dm755 $(MCP_BIN) $(DESTDIR)$(PROGRAM_DIR)/rawmakase-mcp
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/LICENSE
	install -Dm644 licenses/Adobe-DNG-SDK.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Adobe-DNG-SDK.txt
	install -Dm644 licenses/Compact-ICC-Profiles-CC0.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Compact-ICC-Profiles-CC0.txt
	install -Dm644 licenses/Hack-MIT-BitstreamVera.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Hack-MIT-BitstreamVera.txt
	install -Dm644 licenses/Inter-OFL.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Inter-OFL.txt
	install -Dm644 licenses/Lensfun-database-CC-BY-SA-3.0.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Lensfun-database-CC-BY-SA-3.0.txt
	install -Dm644 licenses/Lucide-ISC.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Lucide-ISC.txt
	install -Dm644 licenses/NotoEmoji-OFL.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/NotoEmoji-OFL.txt
	install -Dm644 licenses/Ubuntu-UFL.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Ubuntu-UFL.txt
	install -Dm644 licenses/emoji-icon-font-MIT.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/emoji-icon-font-MIT.txt
ifneq ($(shell uname -s),Darwin)
	install -Dm644 packaging/applications/rawmakase.desktop $(DESTDIR)$(PREFIX)/share/applications/rawmakase.desktop
	install -Dm644 packaging/icons/rawmakase.svg $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/rawmakase.svg
endif

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/rawmakase
	rm -f $(DESTDIR)$(PROGRAM_DIR)/rawmakase-mcp
	-rmdir $(DESTDIR)$(PREFIX)/lib/rawmakase 2>/dev/null
	rm -f $(DESTDIR)$(PREFIX)/share/applications/rawmakase.desktop
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/rawmakase.svg
	rm -rf $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)
