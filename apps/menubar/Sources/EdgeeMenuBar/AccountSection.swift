import SwiftUI

/// Account block: login state plus the org and profile switchers.
struct AccountSection: View {
    @EnvironmentObject private var model: MenuModel

    var body: some View {
        if model.loading {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Checking…").foregroundStyle(.secondary)
            }
        } else if let status = model.status, status.loggedIn {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    Circle().fill(.green).frame(width: 8, height: 8)
                    Text(status.email ?? "Logged in").fontWeight(.semibold)
                }
                if let org = status.orgSlug {
                    switcher(
                        label: "org · \(org)", items: model.orgs,
                        name: { $0.name }, active: { $0.active }
                    ) { org in Task { await model.switchOrg(org.slug) } }
                }
                switcher(
                    label: "profile · \(status.profile)", items: model.profiles,
                    name: { $0.name }, active: { $0.active }
                ) { profile in Task { await model.switchProfile(profile.name) } }
            }
        } else {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 8) {
                    Circle().fill(.red).frame(width: 8, height: 8)
                    Text("Not logged in")
                }
                Button {
                    Task { await model.login() }
                } label: {
                    if model.loggingIn {
                        HStack(spacing: 6) {
                            ProgressView().controlSize(.small)
                            Text("Opening browser…")
                        }
                    } else {
                        Text("Log in…")
                    }
                }
                .disabled(model.loggingIn)
            }
        }
    }

    /// A caption-styled dropdown for switching between `items` (org or profile).
    /// Falls back to a plain caption when there's only one choice, so both
    /// switchers render identically.
    @ViewBuilder
    private func switcher<Item: Identifiable>(
        label: String,
        items: [Item],
        name: @escaping (Item) -> String,
        active: @escaping (Item) -> Bool,
        select: @escaping (Item) -> Void
    ) -> some View {
        if items.count > 1 {
            Menu {
                ForEach(items) { item in
                    Button {
                        guard !active(item) else { return }
                        select(item)
                    } label: {
                        if active(item) {
                            Label(name(item), systemImage: "checkmark")
                        } else {
                            Text(name(item))
                        }
                    }
                }
            } label: {
                Text(label).font(.caption).foregroundStyle(.secondary)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            .disabled(model.switching)
        } else {
            Text(label).font(.caption).foregroundStyle(.secondary)
        }
    }
}
