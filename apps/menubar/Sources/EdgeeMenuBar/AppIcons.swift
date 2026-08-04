import AppKit

enum AppIcons {
    /// The menubar glyph: the Edgee mark as a template image (macOS tints it for
    /// the current menubar appearance). Loaded from the bundled MenuBarIcon.pdf,
    /// with an SF Symbol fallback when running unbundled.
    static let menuBar: NSImage = {
        if let url = Bundle.main.resourceURL?.appendingPathComponent("MenuBarIcon.pdf"),
            let image = NSImage(contentsOf: url)
        {
            image.isTemplate = true
            return image
        }
        let fallback =
            NSImage(systemSymbolName: "bolt.horizontal.fill", accessibilityDescription: "Edgee")
            ?? NSImage()
        fallback.isTemplate = true
        return fallback
    }()
}
