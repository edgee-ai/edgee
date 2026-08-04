import AppKit
import SwiftUI

/// The dropdown panel shown when the menubar icon is clicked.
struct MenuContentView: View {
    @EnvironmentObject private var relays: RelayManager
    @State private var status: AuthStatus?
    @State private var stats: Stats?
    @State private var profiles: [Profile] = []
    @State private var orgs: [Org] = []
    @State private var loading = true
    @State private var statsLoading = true
    @State private var loggingIn = false
    @State private var switching = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            Divider()
            account
            Divider()
            StatsView(stats: stats, loading: statsLoading)
            Divider()
            relaySection
            Divider()
            footer
        }
        .padding(16)
        .frame(width: 320)
        .task { await reload() }
    }

    // MARK: - Sections

    private var header: some View {
        HStack(spacing: 8) {
            Image(nsImage: AppIcons.menuBar)
                .renderingMode(.template)
                .resizable()
                .scaledToFit()
                .frame(height: 20)
                .foregroundStyle(Color(red: 148 / 255, green: 0, blue: 211 / 255))
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
                    orgPicker(current: org)
                }
                profilePicker(current: status.profile)
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

    private var relaySection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Launch & Relay", systemImage: "play.circle.fill").font(.headline)
            if status?.loggedIn == true {
                ForEach(RelayTarget.all.filter { $0.installed }) { target in
                    relayRow(target)
                }
            } else {
                caption("Log in to launch apps and start the relay.")
            }
        }
    }

    @ViewBuilder
    private func relayRow(_ target: RelayTarget) -> some View {
        let state = relays.state(target.id)
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 8) {
                Image(systemName: target.symbol)
                    .frame(width: 16)
                    .foregroundStyle(.secondary)
                Text(target.name)
                Spacer()
                statusDot(state)
                Button(buttonTitle(state, target)) { relays.toggle(target) }
                    .buttonStyle(.borderless)
                    .disabled(state == .starting)
            }
            if case let .failed(message) = state {
                Text(message).font(.caption).foregroundStyle(.red).lineLimit(2)
            }
        }
    }

    private func buttonTitle(_ state: RelayRunState, _ target: RelayTarget) -> String {
        switch state {
        case .running: return "Stop"
        case .starting: return "…"
        case .stopped, .failed: return target.startVerb
        }
    }

    @ViewBuilder
    private func statusDot(_ state: RelayRunState) -> some View {
        let color: Color =
            switch state {
            case .running: .green
            case .starting: .orange
            case .failed: .red
            case .stopped: .secondary
            }
        Circle().fill(color).frame(width: 7, height: 7)
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

    // MARK: - Helpers

    private func caption(_ text: String) -> some View {
        Text(text).font(.caption).foregroundStyle(.secondary)
    }

    /// The `org · <slug>` line. A switcher menu when the account has more than
    /// one org; otherwise a plain caption.
    @ViewBuilder
    private func orgPicker(current: String) -> some View {
        if orgs.count > 1 {
            Menu {
                ForEach(orgs) { org in
                    Button {
                        guard !org.active else { return }
                        Task { await switchOrg(org.slug) }
                    } label: {
                        if org.active {
                            Label(org.name, systemImage: "checkmark")
                        } else {
                            Text(org.name)
                        }
                    }
                }
            } label: {
                Text("org · \(current)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            .disabled(switching)
        } else {
            caption("org · \(current)")
        }
    }

    /// The `profile · <name>` line. A switcher menu when more than one profile
    /// is configured; otherwise a plain caption.
    @ViewBuilder
    private func profilePicker(current: String) -> some View {
        if profiles.count > 1 {
            Menu {
                ForEach(profiles) { profile in
                    Button {
                        guard !profile.active else { return }
                        Task { await switchProfile(profile.name) }
                    } label: {
                        if profile.active {
                            Label(profile.name, systemImage: "checkmark")
                        } else {
                            Text(profile.name)
                        }
                    }
                }
            } label: {
                HStack(spacing: 4) {
                    Text("profile · \(current)")
                    if switching { ProgressView().controlSize(.small) }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            .disabled(switching)
        } else {
            caption("profile · \(current)")
        }
    }

    private func reload() async {
        loading = true
        statsLoading = true
        async let auth = EdgeeCLI.authStatus()
        async let summary = EdgeeCLI.stats()
        async let profs = EdgeeCLI.profiles()
        async let organizations = EdgeeCLI.orgs()
        status = await auth
        loading = false
        stats = await summary
        statsLoading = false
        profiles = await profs
        orgs = await organizations
    }

    private func switchProfile(_ name: String) async {
        switching = true
        await EdgeeCLI.switchProfile(name)
        switching = false
        await reload()
    }

    private func switchOrg(_ idOrSlug: String) async {
        switching = true
        await EdgeeCLI.switchOrg(idOrSlug)
        switching = false
        await reload()
    }

    private func login() async {
        loggingIn = true
        _ = await EdgeeCLI.login()
        loggingIn = false
        await reload()
    }
}
