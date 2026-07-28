APP      := Edgee.app
CONFIG   := release
BIN      := .build/$(CONFIG)/EdgeeMenuBar
# The edgee CLI binary to embed in the bundle. Build it with:
#   (cd ../.. && nix develop -c cargo build --release -p edgee-cli)
EDGEE_BIN ?= ../../target/release/edgee

# A nix/direnv devshell (this repo's flake) exports DEVELOPER_DIR and SDKROOT
# pointing at a nix apple-sdk, and ships its own xcrun shim earlier on PATH.
# That breaks Xcode's swift/xcrun ("tool 'swift' not found"). Clear those and
# put the system toolchain paths first so `make` works inside the nix shell too.
unexport DEVELOPER_DIR
unexport SDKROOT
export PATH := /usr/bin:/bin:/usr/sbin:/sbin:$(PATH)

.PHONY: build bundle run clean

build:
	swift build -c $(CONFIG)

# Assemble a minimal .app bundle around the SwiftPM binary. The Info.plist marks
# it LSUIElement (menubar-only), and we ad-hoc codesign so macOS will run it.
bundle: build
	@test -x "$(EDGEE_BIN)" || { echo "error: edgee binary not found at $(EDGEE_BIN)."; echo "build it: (cd ../.. && nix develop -c cargo build --release -p edgee-cli)"; exit 1; }
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources"
	cp "$(BIN)" "$(APP)/Contents/MacOS/Edgee"
	cp "$(EDGEE_BIN)" "$(APP)/Contents/Resources/edgee"
	cp Info.plist "$(APP)/Contents/Info.plist"
	codesign --force --sign - "$(APP)/Contents/Resources/edgee"
	codesign --force --sign - "$(APP)"

run: bundle
	open "$(APP)"

clean:
	rm -rf .build "$(APP)"
