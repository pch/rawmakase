PREFIX ?= $(HOME)/.local
# Package name for the license directory (Arch packages pass their pkgname).
PKGNAME ?= rawmakase
BIN := target/release/rawmakase

.PHONY: all build check check-python check-msrv check-tools install uninstall
all: build

build:
	cargo build --release --locked

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
check: check-python check-tools check-msrv
	python3 scripts/deps.py check
	scripts/crate-closures.sh
	cargo fmt --check
	cargo clippy --locked --workspace --all-targets -- -D warnings
	cargo test --locked -p rawmakase -p rawmakase-decode -p rawmakase-export
	cargo clippy --locked --all-targets -p rawmakase-cancel -p rawmakase-fs -p rawmakase-tiff -p rawmakase-raw -p rawmakase-model -p rawmakase-interop -p rawmakase-catalog -p rawmakase-engine -p rawmakase-protocol -p rawmakase-remote -p rawmakase-ctl -- -D warnings
	cargo test --locked -p rawmakase-model --all-features
	cargo test --locked -p rawmakase-cancel -p rawmakase-fs -p rawmakase-tiff -p rawmakase-raw -p rawmakase-interop -p rawmakase-catalog -p rawmakase-engine
	cargo test --locked -p rawmakase-protocol -p rawmakase-remote -p rawmakase-ctl -p rawmakase-inference
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
	install -Dm755 $(BIN) $(DESTDIR)$(PREFIX)/bin/rawmakase
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
	rm -f $(DESTDIR)$(PREFIX)/share/applications/rawmakase.desktop
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/rawmakase.svg
	rm -rf $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)
