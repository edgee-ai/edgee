import AppKit
import SwiftUI

/// The dropdown panel shown when the menubar icon is clicked.
struct MenuContentView: View {
    @State private var status: AuthStatus?
    @State private var loading = true
    @State private var loggingIn = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            Divider()
            account
            Divider()
            placeholders
            Divider()
            footer
        }
        .padding(16)
        .frame(width: 300)
        .task { await reload() }
    }

    // MARK: - Sections

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "bolt.horizontal.fill")
                .foregroundStyle(.purple)
            Text("Edgee")
                .font(.title2.bold())
            Spacer()
            Button {
                Task { await reload() }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
            .help("Refresh")
        }
    }

    @ViewBuilder private var account: some View {
        if loading {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Checking…").foregroundStyle(.secondary)
            }
        } else if let status, status.loggedIn {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    Circle().fill(.green).frame(width: 8, height: 8)
                    Text(status.email ?? "Logged in").fontWeight(.semibold)
                }
                if let org = status.orgSlug {
                    caption("org · \(org)")
                }
                caption("profile · \(status.profile)")
                if !status.providers.isEmpty {
                    caption("configured · \(status.providers.keys.sorted().joined(separator: ", "))")
                }
            }
        } else {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 8) {
                    Circle().fill(.red).frame(width: 8, height: 8)
                    Text("Not logged in")
                }
                Button {
                    Task { await login() }
                } label: {
                    if loggingIn {
                        HStack(spacing: 6) {
                            ProgressView().controlSize(.small)
                            Text("Opening browser…")
                        }
                    } else {
                        Text("Log in…")
                    }
                }
                .disabled(loggingIn)
            }
        }
    }

    private var placeholders: some View {
        VStack(alignment: .leading, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Label("Stats", systemImage: "chart.bar.fill").font(.headline)
                caption("Token savings & sessions — coming next")
            }
            VStack(alignment: .leading, spacing: 2) {
                Label("Launch & Relay", systemImage: "play.circle.fill").font(.headline)
                caption("Start agents and the relay — coming next")
            }
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
            Button("Quit") { NSApplication.shared.terminate(nil) }
        }
        .buttonStyle(.borderless)
    }

    // MARK: - Helpers

    private func caption(_ text: String) -> some View {
        Text(text).font(.caption).foregroundStyle(.secondary)
    }

    private func reload() async {
        loading = true
        status = await EdgeeCLI.authStatus()
        loading = false
    }

    private func login() async {
        loggingIn = true
        _ = await EdgeeCLI.login()
        loggingIn = false
        await reload()
    }
}
