import DeadreckonKit
import SwiftUI

// MARK: - Shared row atoms

/// The state-dot grammar shared by the sidebar, the Overview, and the
/// popover (DESIGN.md §5): accent = live, success = verified awaiting
/// approval, warn = stopped/paused for you, danger = failed,
/// textTertiary = queued/waiting-quiet. Durable facts only.
enum RunRowAtoms {
    static func isLive(_ row: FleetRow) -> Bool {
        switch row.projection.phase {
        case .running, .verifyingChecks, .verifyingMeaning: return true
        default: return false
        }
    }

    static func dotColor(_ item: QueueItem) -> Color {
        guard let row = item.row else { return Theme.warn }
        switch item.section {
        case .atTheGate: return Theme.success
        case .needsReview: return Theme.warn
        case .approaching: return Theme.accent
        case .underway:
            if item.needsDecision { return Theme.warn }
            return row.projection.phase == .running ? Theme.accent : Theme.textTertiary
        case .wrecked: return Theme.danger
        case .unknown: return Theme.warn
        }
    }
}

/// A 6px run-state dot that breathes when the run is live (DESIGN.md §7:
/// the single ambient motion, opacity 1.0→0.55→1.0 over 1.6s; Reduce
/// Motion disables it).
struct RunStateDot: View {
    let item: QueueItem

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var dimmed = false

    var body: some View {
        let live = item.row.map(RunRowAtoms.isLive) ?? false
        Circle()
            .fill(RunRowAtoms.dotColor(item))
            .frame(width: 6, height: 6)
            .opacity(live && dimmed ? 0.55 : 1)
            .onAppear {
                guard live, !reduceMotion else { return }
                withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true)) {
                    dimmed = true
                }
            }
            .accessibilityHidden(true)
    }
}

/// One fleet-health verdict for the quiet footer surfaces: the worst
/// degraded fact in plain words, or "All systems OK". Unknown states say
/// "unknown" with the reason in the tooltip — never a guessed count.
@MainActor
enum FleetHealth {
    struct Summary {
        let color: Color
        let text: String
        let tooltip: String?
    }

    static func summarize(store: FleetStore, attention: AttentionCenter) -> Summary {
        if case .unavailable(let reason) = store.fleet {
            return Summary(color: Theme.warn, text: "Can\u{2019}t read your runs", tooltip: reason)
        }

        var legacyLine: String? {
            store.legacyRunCount > 0 ? Lexicon.olderCLIRuns(store.legacyRunCount) : nil
        }
        func tooltip(_ extra: String? = nil) -> String? {
            let parts = [extra, legacyLine].compactMap { $0 }
            return parts.isEmpty ? nil : parts.joined(separator: "\n")
        }

        // Worst first: hard failures, then degradations, then unknowns.
        if case .failed = store.harbor.doctor {
            return Summary(color: Theme.danger,
                           text: Lexicon.healthWord(store.harbor.doctor),
                           tooltip: tooltip("doctor --json"))
        }
        if case .stopped = store.harbor.supervisor {
            return Summary(color: Theme.warn,
                           text: Lexicon.serviceWord(store.harbor.supervisor),
                           tooltip: tooltip())
        }
        if case .ok(let warnings) = store.harbor.doctor, warnings > 0 {
            return Summary(color: Theme.warn,
                           text: Lexicon.healthWord(store.harbor.doctor),
                           tooltip: tooltip("doctor --json"))
        }
        if case .counted(let ok, let total) = store.harbor.providers, ok < total {
            return Summary(color: Theme.warn,
                           text: Lexicon.agentsWord(store.harbor.providers),
                           tooltip: tooltip())
        }
        if !attention.issues.isEmpty {
            let reasons = attention.issues.sorted(by: { $0.key < $1.key })
                .map { "\($0.key): \($0.value)" }
                .joined(separator: "\n")
            return Summary(color: Theme.warn,
                           text: Lexicon.notificationsBroken(attention.issues.count),
                           tooltip: tooltip(reasons))
        }
        if case .unknown(let reason) = store.harbor.doctor {
            return Summary(color: Theme.textTertiary,
                           text: Lexicon.healthWord(store.harbor.doctor),
                           tooltip: tooltip(reason))
        }
        if case .unknown(let reason) = store.harbor.providers {
            return Summary(color: Theme.textTertiary,
                           text: Lexicon.agentsWord(store.harbor.providers),
                           tooltip: tooltip(reason))
        }
        if case .unknown(let reason) = store.harbor.supervisor {
            return Summary(color: Theme.textTertiary,
                           text: Lexicon.serviceWord(store.harbor.supervisor),
                           tooltip: tooltip(reason))
        }
        return Summary(color: Theme.success, text: "All systems OK", tooltip: tooltip())
    }
}

// MARK: - Sidebar

/// The window sidebar (REDESIGN-SPEC §B1): brand header, the window's one
/// accent action, the NEEDS YOU group, the projects → runs tree grouped by
/// the rollup's `scope`, and the quiet fleet-health footer. Rows derive
/// from durable rollup facts only; selection swaps the center in place and
/// stays neutral (well + border) — accent marks liveness, not selection.
struct SidebarView: View {
    @ObservedObject var store: FleetStore
    @ObservedObject var attention: AttentionCenter
    @Binding var selection: QueueItem?
    let onNewGoal: () -> Void

    /// Collapsed project scopes, remembered across launches (§B1).
    @State private var collapsedScopes: Set<String> =
        Set(UserDefaults.standard.stringArray(forKey: "sidebarCollapsedProjects") ?? [])
    /// Per-project "N finished" disclosures (session state).
    @State private var finishedShown: Set<String> = []

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            newGoalButton
            Divider().overlay(Theme.border)
            content
            Divider().overlay(Theme.border)
            footer
        }
        .frame(width: 240)
        // Paints up behind the transparent titlebar so the sidebar column
        // is one unbroken surface.
        .background(Theme.sidebarBg.ignoresSafeArea())
    }

    // MARK: Header + the one accent action

    private var header: some View {
        HStack(spacing: 7) {
            Image(systemName: "diamond.fill")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(Theme.accent)
            Text("deadreckon")
                .font(Theme.body(13, weight: .semibold))
                .foregroundStyle(Theme.textPrimary)
            Spacer()
        }
        .padding(.horizontal, 14)
        .frame(height: 44)
    }

    private var newGoalButton: some View {
        Button {
            onNewGoal()
        } label: {
            Label("New Goal", systemImage: "plus")
                .frame(maxWidth: .infinity)
        }
        .buttonStyle(.themePrimary)
        .disabled(newGoalDisabledReason != nil)
        .help(newGoalDisabledReason
            ?? "Start a new goal (\u{2318}N) \u{2014} everything is visible before anything runs")
        .padding(.horizontal, 12)
        .padding(.bottom, 10)
    }

    /// New Goal stays enabled through a degraded fleet scan; only a missing
    /// binary (no version fact ever read) disables it, with the reason as
    /// its help (§B4).
    private var newGoalDisabledReason: String? {
        if case .unavailable(let reason) = store.fleet, store.binaryVersion == nil {
            return reason
        }
        return nil
    }

    // MARK: Content states

    @ViewBuilder private var content: some View {
        switch store.fleet {
        case .loading:
            skeleton
        case .unavailable:
            // §B4: the center banner carries the reason; the sidebar keeps
            // just header + New Goal + degraded footer.
            Spacer()
        case .loaded(let queue):
            if queue.isEmpty {
                Spacer()
            } else {
                tree
            }
        }
    }

    /// Loading skeleton (§B4): three quiet bars, never fake rows.
    private var skeleton: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(0..<3, id: \.self) { _ in
                RoundedRectangle(cornerRadius: 3, style: .continuous)
                    .fill(Theme.panel)
                    .frame(height: 10)
            }
            Spacer()
        }
        .padding(14)
        .accessibilityHidden(true)
    }

    private var tree: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 2) {
                needsYouGroup
                projectGroups
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
        }
    }

    // MARK: NEEDS YOU

    /// Decision inventory, decision-first (the existing queue order):
    /// verified-awaiting-approval, judge-stopped review, decision-shaped
    /// waiting rows.
    private var needsYouItems: [QueueItem] {
        store.queue.allItems.filter { $0.needsDecision || $0.section == .atTheGate }
    }

    @ViewBuilder private var needsYouGroup: some View {
        let items = needsYouItems
        if !items.isEmpty {
            HStack(spacing: 6) {
                Theme.sectionTitle("NEEDS YOU", color: Theme.textSecondary)
                CountBadge(count: items.count)
                Spacer()
            }
            .padding(.horizontal, 6)
            .padding(.top, 6)
            .padding(.bottom, 2)
            // Namespaced identity: these items ALSO render in their project
            // group below. Two ForEach entries sharing one id inside the
            // same LazyVStack makes SwiftUI drop rows (blank 36px ghosts in
            // the tree), so the decision inventory gets its own id space.
            ForEach(items, id: \.needsYouRowID) { item in
                needsYouRow(item)
            }
        }
    }

    private func needsYouRow(_ item: QueueItem) -> some View {
        sidebarRowButton(item) {
            HStack(alignment: .center, spacing: 8) {
                RunStateDot(item: item)
                VStack(alignment: .leading, spacing: 1) {
                    Text(goalText(item))
                        .font(Theme.baseMedium)
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1)
                    Text(Lexicon.rowStateWord(item))
                        .font(Theme.small)
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8)
            .frame(height: 40)
        }
    }

    // MARK: Projects tree

    private struct ProjectGroup: Identifiable {
        let scope: String
        let items: [QueueItem]
        var id: String { scope }
    }

    /// Rows grouped by the rollup's `scope` fact, projects ordered by
    /// display name. Quarantined entries (no trustworthy scope) gather in
    /// one honest trailing group.
    private var groups: [ProjectGroup] {
        var byScope: [String: [QueueItem]] = [:]
        var unreadable: [QueueItem] = []
        for item in store.queue.allItems {
            if let row = item.row {
                byScope[row.scope, default: []].append(item)
            } else {
                unreadable.append(item)
            }
        }
        var groups = byScope.map { ProjectGroup(scope: $0.key, items: $0.value) }
            .sorted {
                let a = Lexicon.projectName($0.scope).lowercased()
                let b = Lexicon.projectName($1.scope).lowercased()
                return a != b ? a < b : $0.scope < $1.scope
            }
        if !unreadable.isEmpty {
            groups.append(ProjectGroup(scope: "", items: unreadable))
        }
        return groups
    }

    @ViewBuilder private var projectGroups: some View {
        ForEach(groups) { group in
            projectHeader(group)
            if !collapsedScopes.contains(group.id) {
                projectRows(group)
            }
        }
    }

    private func projectHeader(_ group: ProjectGroup) -> some View {
        let collapsed = collapsedScopes.contains(group.id)
        let decisions = group.items.filter { $0.needsDecision || $0.section == .atTheGate }.count
        return Button {
            if collapsed {
                collapsedScopes.remove(group.id)
            } else {
                collapsedScopes.insert(group.id)
            }
            UserDefaults.standard.set(Array(collapsedScopes), forKey: "sidebarCollapsedProjects")
        } label: {
            HStack(spacing: 5) {
                Image(systemName: collapsed ? "chevron.right" : "chevron.down")
                    .font(.system(size: 8, weight: .semibold))
                    .foregroundStyle(Theme.textTertiary)
                Text(group.scope.isEmpty ? "unreadable entries" : Lexicon.projectName(group.scope))
                    .font(Theme.body(11, weight: .semibold))
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(1)
                Text("(\(group.items.count))")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.textTertiary)
                    .monospacedDigit()
                if decisions > 0 {
                    CountBadge(count: decisions)
                }
                Spacer()
            }
            .padding(.horizontal, 6)
            .padding(.top, 10)
            .padding(.bottom, 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.tactile)
        .help(group.scope.isEmpty
            ? "entries the app could not read; the CLI remains authoritative"
            : group.scope)
    }

    @ViewBuilder private func projectRows(_ group: ProjectGroup) -> some View {
        let split = splitRows(group)
        ForEach(split.listed) { item in
            projectRow(item)
        }
        if !split.olderFinished.isEmpty {
            Divider().overlay(Theme.border).padding(.horizontal, 6).padding(.vertical, 2)
            Button {
                if finishedShown.contains(group.id) {
                    finishedShown.remove(group.id)
                } else {
                    finishedShown.insert(group.id)
                }
            } label: {
                HStack(spacing: 4) {
                    Text("\(split.olderFinished.count) finished")
                        .font(Theme.small)
                        .foregroundStyle(Theme.textTertiary)
                        .monospacedDigit()
                    Image(systemName: finishedShown.contains(group.id)
                        ? "chevron.down" : "chevron.right")
                        .font(.system(size: 7, weight: .semibold))
                        .foregroundStyle(Theme.textTertiary)
                    Spacer()
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 3)
                .contentShape(Rectangle())
            }
            .buttonStyle(.tactile)
            if finishedShown.contains(group.id) {
                ForEach(split.olderFinished) { item in
                    projectRow(item)
                }
            }
        }
    }

    /// §B1 ordering: live first, then needs-you, then recency; finished
    /// (terminal non-decision) runs keep their newest five inline and fold
    /// the rest behind the "N finished" disclosure.
    private func splitRows(_ group: ProjectGroup) -> (listed: [QueueItem], olderFinished: [QueueItem]) {
        let finished = group.items.filter { $0.section == .wrecked }
            .sorted { updatedAt($0) > updatedAt($1) }
        let active = group.items.filter { $0.section != .wrecked }
            .sorted { rank($0) != rank($1) ? rank($0) < rank($1) : updatedAt($0) > updatedAt($1) }
        return (active + Array(finished.prefix(5)), Array(finished.dropFirst(5)))
    }

    private func rank(_ item: QueueItem) -> Int {
        guard let row = item.row else { return 3 }
        if RunRowAtoms.isLive(row) { return 0 }
        if item.needsDecision || item.section == .atTheGate { return 1 }
        return 2
    }

    private func updatedAt(_ item: QueueItem) -> Date {
        item.row?.updatedAt ?? .distantPast
    }

    private func projectRow(_ item: QueueItem) -> some View {
        sidebarRowButton(item) {
            HStack(alignment: .center, spacing: 8) {
                RunStateDot(item: item)
                Text(goalText(item))
                    .font(Theme.body(12.5))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                Spacer(minLength: 8)
                if let row = item.row {
                    Text(Lexicon.relativeTime(row.updatedAt))
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textTertiary)
                        .monospacedDigit()
                }
            }
            .padding(.horizontal, 8)
            .frame(height: 36)
        }
        .help("\(goalText(item)) \u{2014} \(Lexicon.rowStateWord(item))")
    }

    // MARK: Row chrome

    private func sidebarRowButton(_ item: QueueItem,
                                  @ViewBuilder label: () -> some View) -> some View {
        let selected = selection?.id == item.id
        return Button {
            selection = item
        } label: {
            label()
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(
                    selected ? Theme.well : .clear,
                    in: RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous)
                    .strokeBorder(selected ? Theme.borderHover : .clear, lineWidth: 1))
                .contentShape(Rectangle())
        }
        .buttonStyle(.tactile)
        .accessibilityLabel("\(goalText(item)), \(Lexicon.rowStateWord(item))")
    }

    private func goalText(_ item: QueueItem) -> String {
        switch item.kind {
        case .job(let row): return row.goal
        case .quarantined(let inner): return inner.goal ?? Lexicon.unreadableEntry
        }
    }

    // MARK: Footer (fleet health, quiet)

    @ViewBuilder private var footer: some View {
        if case .loading = store.fleet {
            VStack(alignment: .leading, spacing: 2) {
                Text("Reading your runs\u{2026}")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textSecondary)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
        } else {
            SidebarFooterView(store: store, attention: attention)
        }
    }
}

/// The fleet-health footer: dot + the worst degraded fact in plain words
/// (or "All systems OK"), refresh recency beneath. Click opens Settings —
/// the Info tab holds the full read-only facts.
private struct SidebarFooterView: View {
    @ObservedObject var store: FleetStore
    @ObservedObject var attention: AttentionCenter
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        let summary = FleetHealth.summarize(store: store, attention: attention)
        Button {
            openSettings()
        } label: {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    StateDot(color: summary.color)
                    Text(summary.text)
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)
                    Spacer()
                }
                if let refreshed = store.lastRefreshed {
                    Text("updated \(Lexicon.relativeTime(refreshed))")
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textTertiary)
                        .monospacedDigit()
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.tactile)
        .help(summary.tooltip ?? "System health \u{2014} opens Settings")
    }
}

private extension QueueItem {
    /// Distinct ForEach identity for the NEEDS YOU copies of rows that the
    /// projects tree lists again below (see needsYouGroup): two entries
    /// sharing one id inside the same LazyVStack render as blank rows.
    var needsYouRowID: String { "needs-you-\(id)" }
}
