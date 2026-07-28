import SwiftUI

/// Edgee macOS menubar app.
///
/// A native SwiftUI `MenuBarExtra` (window style → an anchored dropdown panel).
/// It talks to the `edgee` CLI as a subprocess and reads Edgee's local files, so
/// the Rust CLI stays the single source of truth for auth, stats, and launching.
@main
struct EdgeeMenuBarApp: App {
    var body: some Scene {
        MenuBarExtra("Edgee", systemImage: "bolt.horizontal.fill") {
            MenuContentView()
        }
        .menuBarExtraStyle(.window)
    }
}
