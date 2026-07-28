APP     := Edgee.app
CONFIG  := release
BIN     := .build/$(CONFIG)/EdgeeMenuBar

.PHONY: build bundle run clean

build:
	swift build -c $(CONFIG)

# Assemble a minimal .app bundle around the SwiftPM binary. The Info.plist marks
# it LSUIElement (menubar-only), and we ad-hoc codesign so macOS will run it.
bundle: build
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources"
	cp "$(BIN)" "$(APP)/Contents/MacOS/Edgee"
	cp Info.plist "$(APP)/Contents/Info.plist"
	codesign --force --sign - "$(APP)"

run: bundle
	open "$(APP)"

clean:
	rm -rf .build "$(APP)"
