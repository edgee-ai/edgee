// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "EdgeeMenuBar",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "EdgeeMenuBar",
            path: "Sources/EdgeeMenuBar",
            // Language mode v5 keeps the skeleton free of Swift 6 strict-concurrency
            // friction; we can tighten to v6 once the app surface settles.
            swiftSettings: [.swiftLanguageMode(.v5)]
        )
    ]
)
