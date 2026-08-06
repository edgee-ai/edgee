import AppKit
import SwiftUI

/// The dropdown panel shown when the menubar icon is clicked. Composes the
/// account, stats, and relay sections; data/actions live in `MenuModel`.
struct MenuContentView: View {
    @EnvironmentObject private var model: MenuModel
    @EnvironmentObject private var relays: RelayManager

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            Divider()
            AccountSection()
            Divider()
            StatsView(stats: model.stats, loading: model.statsLoading)
            Divider()
            RelaySection()
            Divider()
            footer
        }
        .padding(16)
        .frame(width: 320)
        .task { await model.reload() }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Image(nsImage: AppIcons.menuBar)
                .renderingMode(.template)
                .resizable()
                .scaledToFit()
                .frame(height: 20)
                .foregroundStyle(Theme.brand)
            Text("Edgee")
                .font(.title2.bold())
            Spacer()
            Button {
                Task { await model.reload() }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
            .help("Refresh")
        }
    }

    private var footer: some View {
        HStack {
            Button("Console") {
                if let url = URL(string: "https://www.edgee.ai") {
                    NSWorkspace.shared.open(url)
                }
            }
            Spacer()
            Button("Quit") {
                relays.stopAll()
                NSApplication.shared.terminate(nil)
            }
        }
        .buttonStyle(.borderless)
    }
}
