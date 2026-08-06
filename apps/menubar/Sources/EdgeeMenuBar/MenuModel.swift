import SwiftUI

/// Owns the panel's data and its CLI-backed actions. Held at the App level so
/// state is cached across popover opens — a reopen refreshes in the background
/// instead of blanking the UI.
@MainActor
final class MenuModel: ObservableObject {
    @Published private(set) var status: AuthStatus?
    @Published private(set) var stats: Stats?
    @Published private(set) var profiles: [Profile] = []
    @Published private(set) var orgs: [Org] = []
    @Published private(set) var loading = true
    @Published private(set) var statsLoading = true
    @Published private(set) var loggingIn = false
    @Published private(set) var switching = false

    func reload() async {
        // Only show the loading placeholders on a cold start; a refresh keeps the
        // cached values on screen until the new ones arrive.
        if status == nil { loading = true }
        if stats == nil { statsLoading = true }

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

    func login() async {
        loggingIn = true
        _ = await EdgeeCLI.login()
        loggingIn = false
        await reload()
    }

    func switchProfile(_ name: String) async {
        switching = true
        await EdgeeCLI.switchProfile(name)
        switching = false
        await reload()
    }

    func switchOrg(_ idOrSlug: String) async {
        switching = true
        await EdgeeCLI.switchOrg(idOrSlug)
        switching = false
        await reload()
    }
}
