APP      := Edgee.app
CONFIG   := release
BIN      := .build/$(CONFIG)/EdgeeMenuBar
# The edgee CLI binary embedded in the bundle. By default it is (re)built from
# source via the repo's nix devshell so it can never go stale relative to the
# app. Set BUILD_CLI=0 (and optionally EDGEE_BIN=/path) to embed a prebuilt one.
EDGEE_BIN ?= ../../target/release/edgee
BUILD_CLI ?= 1

# A nix/direnv devshell (this repo's flake) exports DEVELOPER_DIR and SDKROOT
# pointing at a nix apple-sdk, and ships its own xcrun shim earlier on PATH.
# That breaks Xcode's swift/xcrun ("tool 'swift' not found"). Clear those and
# put the system toolchain paths first so `make` works inside the nix shell too.
unexport DEVELOPER_DIR
unexport SDKROOT
export PATH := /usr/bin:/bin:/usr/sbin:/sbin:$(PATH)

.PHONY: build bundle run clean edgee-cli icons

# Regenerate the app icon (Edgee.icns) and menubar template (MenuBarIcon.pdf)
# from the Edgee mark. Run after changing tools/gen-icons.swift; commit the
# resulting Assets/. (swift/iconutil inherit the fixed env from above.)
icons:
	swift tools/gen-icons.swift Assets
	iconutil -c icns Assets/Edgee.iconset -o Assets/Edgee.icns
	rm -rf Assets/Edgee.iconset

build:
	swift build -c $(CONFIG)

# (Re)build the edgee CLI (release) via the repo's nix devshell, unless disabled.
# nix sets its own DEVELOPER_DIR/SDKROOT for the Rust build, so the unexports
# above don't affect it. Keeps the embedded binary in lockstep with the app.
edgee-cli:
	@if [ "$(BUILD_CLI)" = "1" ]; then \
		echo "building edgee CLI (release) via nix…"; \
		( cd ../.. && nix develop -c cargo build --release -p edgee-cli ); \
	fi

# Assemble a minimal .app bundle around the SwiftPM binary. The Info.plist marks
# it LSUIElement (menubar-only), and we ad-hoc codesign so macOS will run it.
bundle: build edgee-cli
	@test -x "$(EDGEE_BIN)" || { echo "error: edgee binary not found at $(EDGEE_BIN)."; echo "build it: (cd ../.. && nix develop -c cargo build --release -p edgee-cli)"; exit 1; }
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources"
	cp "$(BIN)" "$(APP)/Contents/MacOS/Edgee"
	cp "$(EDGEE_BIN)" "$(APP)/Contents/Resources/edgee"
	cp Assets/Edgee.icns "$(APP)/Contents/Resources/Edgee.icns"
	cp Assets/MenuBarIcon.pdf "$(APP)/Contents/Resources/MenuBarIcon.pdf"
	cp Info.plist "$(APP)/Contents/Info.plist"
	codesign --force --sign - "$(APP)/Contents/Resources/edgee"
	codesign --force --sign - "$(APP)"

run: bundle
	open "$(APP)"

clean:
	rm -rf .build "$(APP)"
