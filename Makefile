PREFIX ?= $(HOME)/.local
# Package name for the license directory (Arch packages pass their pkgname).
PKGNAME ?= rawmakase
# Extra flags for the release build, e.g. --no-default-features to leave out
# the software HEVC decoder (see docs/dependencies.md).
CARGO_FLAGS ?=
BIN := target/release/rawmakase

.PHONY: all build check install uninstall
all: build

build:
	cargo build --release --locked $(CARGO_FLAGS)

check:
	python3 scripts/deps.py check
	python3 -m unittest discover -s scripts/corpus
	scripts/crate-closures.sh
	cargo fmt --check
	cargo clippy --locked --workspace --all-targets -- -D warnings
	cargo test --locked -p rawmakase -p rawmakase-decode -p rawmakase-export
	cargo clippy --locked --all-targets --no-default-features -p rawmakase -p rawmakase-decode -- -D warnings
	cargo test --locked --no-default-features -p rawmakase-decode
	cargo clippy --locked --all-targets -p rawmakase-tiff -p rawmakase-raw -p rawmakase-model -p rawmakase-interop -p rawmakase-catalog -p rawmakase-engine -p rawmakase-protocol -p rawmakase-ctl -- -D warnings
	cargo test --locked -p rawmakase-model --all-features
	cargo test --locked -p rawmakase-tiff -p rawmakase-raw -p rawmakase-interop -p rawmakase-catalog -p rawmakase-engine
	cargo test --locked -p rawmakase-protocol -p rawmakase-ctl

# Install does not rebuild, so `make && sudo make install PREFIX=/usr` never compiles as root.
install:
	@test -x $(BIN) || { echo "Run 'make' first to build $(BIN)"; exit 1; }
	install -Dm755 $(BIN) $(DESTDIR)$(PREFIX)/bin/rawmakase
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/LICENSE
	install -Dm644 licenses/Adobe-DNG-SDK.txt $(DESTDIR)$(PREFIX)/share/licenses/$(PKGNAME)/Adobe-DNG-SDK.txt
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
