import AppKit
import SwiftUI

/// "Launch & Relay" section: one row per installed relay target, driven by the
/// shared `RelayManager`.
struct RelaySection: View {
    @EnvironmentObject private var model: MenuModel
    @EnvironmentObject private var relays: RelayManager

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Launch & Relay", systemImage: "play.circle.fill").font(.headline)
            if model.status?.loggedIn == true {
                ForEach(RelayTarget.all.filter { $0.installed }) { target in
                    row(target)
                }
            } else {
                Text("Log in to launch apps and start the relay.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private func row(_ target: RelayTarget) -> some View {
        let state = relays.state(target.id)
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 8) {
                icon(for: target)
                Text(target.name)
                Spacer()
                dot(state)
                Button(title(state)) { relays.toggle(target) }
                    .buttonStyle(.borderless)
                    .disabled(state == .starting)
            }
            if case let .failed(message) = state {
                Text(message).font(.caption).foregroundStyle(.red).lineLimit(2)
            }
        }
    }

    /// The app's real macOS icon when installed; otherwise a generic SF Symbol.
    @ViewBuilder
    private func icon(for target: RelayTarget) -> some View {
        if let path = target.appBundlePath {
            Image(nsImage: NSWorkspace.shared.icon(forFile: path))
                .resizable()
                .frame(width: 18, height: 18)
        } else {
            Image(systemName: target.symbol)
                .frame(width: 18)
                .foregroundStyle(.secondary)
        }
    }

    private func title(_ state: RelayRunState) -> String {
        switch state {
        case .running: return "Stop"
        case .starting: return "…"
        case .stopped, .failed: return "Start"
        }
    }

    @ViewBuilder
    private func dot(_ state: RelayRunState) -> some View {
        let color: Color =
            switch state {
            case .running: .green
            case .starting: .orange
            case .failed: .red
            case .stopped: .secondary
            }
        Circle().fill(color).frame(width: 7, height: 7)
    }
}
