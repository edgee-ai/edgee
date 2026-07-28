# Edgee menubar app

A native macOS menubar app (SwiftUI `MenuBarExtra`) that surfaces Edgee stats
and launches agents/relay. It shells out to the `edgee` CLI and reads Edgee's
local files, so the Rust CLI stays the single source of truth.

## Requirements

- macOS 14+
- Xcode / Swift toolchain (`swift`, `xcodebuild`)
- The `edgee` CLI installed (found on `PATH`, in `~/.local/bin`, or set `EDGEE_BIN`)

## Build & run

```bash
make run        # build, bundle into Edgee.app, and launch it
make build      # just `swift build -c release`
make bundle     # produce Edgee.app without launching
make clean
```

`make run` produces `Edgee.app` (marked `LSUIElement`, so it lives only in the
menubar — no Dock icon) and opens it. Click the menubar icon for the dropdown.

## Layout

- `Sources/EdgeeMenuBar/EdgeeMenuBarApp.swift` — `@main` app + `MenuBarExtra` scene
- `Sources/EdgeeMenuBar/MenuContentView.swift` — the dropdown UI
- `Sources/EdgeeMenuBar/EdgeeCLI.swift` — subprocess wrapper around `edgee`
- `Sources/EdgeeMenuBar/AuthStatus.swift` — decodes `edgee auth status --json`
- `Info.plist` — bundle metadata (`LSUIElement`)
- `Makefile` — build/bundle/run
