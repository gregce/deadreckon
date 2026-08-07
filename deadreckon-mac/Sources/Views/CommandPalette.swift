import DeadreckonKit
import SwiftUI

/// Command-K: dimmed backdrop with a centered filter panel over the queue
/// (the exemplar SearchOverlay pattern, simplified to a text filter). Rows
/// match on goal, job id, or provider; Enter opens the first match in the
/// APP-3 stub detail.
struct CommandPalette: View {
    @ObservedObject var store: FleetStore
    @Binding var isShown: Bool
    let onOpen: (QueueItem) -> Void

    @State private var query = ""
    @FocusState private var focused: Bool

    var body: some View {
        ZStack(alignment: .top) {
            Theme.scrim
                .ignoresSafeArea()
                .onTapGesture { dismiss() }

            VStack(spacing: 0) {
                HStack(spacing: 10) {
                    Image(systemName: "magnifyingglass")
                        .foregroundStyle(Theme.textTertiary)
                    TextField("Search runs \u{2014} goal, id, or agent", text: $query)
                        .textFieldStyle(.plain)
                        .font(Theme.body(15))
                        .focused($focused)
                        .onSubmit {
                            if let first = matches.first { open(first) }
                        }
                    if !query.isEmpty {
                        Button {
                            query = ""
                        } label: {
                            Image(systemName: "xmark.circle.fill")
                                .foregroundStyle(Theme.textTertiary)
                        }
                        .buttonStyle(.tactile)
                        .accessibilityLabel("Clear filter")
                    }
                }
                .padding(14)

                Divider().overlay(Theme.border)

                if matches.isEmpty {
                    Text(store.queue.isEmpty ? "No runs yet" : "No matches")
                        .font(Theme.body(12))
                        .foregroundStyle(Theme.textTertiary)
                        .padding(16)
                } else {
                    ScrollView {
                        VStack(spacing: 2) {
                            ForEach(matches.prefix(12)) { item in
                                PaletteRow(item: item) { open(item) }
                            }
                        }
                        .padding(8)
                    }
                    .frame(maxHeight: 320)
                }

                Divider().overlay(Theme.border)

                HStack(spacing: 12) {
                    Text("\u{23CE} open")
                    Text("esc dismiss")
                    Spacer()
                }
                .font(Theme.body(10))
                .foregroundStyle(Theme.textTertiary)
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
            }
            .background(Theme.windowBg,
                        in: RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
                .strokeBorder(Theme.border, lineWidth: 1))
            // The one permitted shadow: overlays only (DESIGN.md §4).
            .shadow(color: Theme.overlayShadow, radius: 24, y: 8)
            .frame(maxWidth: 620)
            .padding(.top, 56)
            .padding(.horizontal, 40)
        }
        .onAppear { focused = true }
        .onExitCommand { dismiss() }
        .transition(.opacity)
    }

    /// Queue order is preserved (decision-readiness first), so the first
    /// match is always the most decision-ready one.
    private var matches: [QueueItem] {
        let all = store.queue.allItems
        let needle = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !needle.isEmpty else { return all }
        return all.filter { item in
            switch item.kind {
            case .job(let row):
                return row.goal.lowercased().contains(needle)
                    || row.jobID.lowercased().contains(needle)
                    || (row.provider?.lowercased().contains(needle) ?? false)
            case .quarantined(let inner):
                return (inner.goal?.lowercased().contains(needle) ?? false)
                    || (inner.jobID?.lowercased().contains(needle) ?? false)
            }
        }
    }

    private func open(_ item: QueueItem) {
        dismiss()
        onOpen(item)
    }

    private func dismiss() {
        query = ""
        isShown = false
    }
}

private struct PaletteRow: View {
    let item: QueueItem
    let onOpen: () -> Void

    @State private var hovering = false

    var body: some View {
        Button(action: onOpen) {
            HStack(spacing: 10) {
                Theme.sectionTitle(Lexicon.sectionTitle(item.section))
                    .frame(width: 130, alignment: .leading)
                VStack(alignment: .leading, spacing: 1) {
                    Text(goal)
                        .font(Theme.body(12, weight: .medium))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1)
                    HStack(spacing: 5) {
                        Text(id)
                            .font(Theme.monoS)
                        if let provider = item.row?.provider {
                            Text("\u{00B7}")
                            ProviderIcon(provider: provider, size: 12)
                            Text(Lexicon.agentName(provider) ?? provider).font(Theme.caption)
                        }
                    }
                    .foregroundStyle(Theme.textTertiary)
                }
                Spacer()
                if item.needsDecision {
                    StatusChip(text: Lexicon.needsYou, color: Theme.warn)
                }
                if item.row?.receipt?.verified == .valid {
                    StatusChip(text: Lexicon.proofVerified, color: Theme.success, strong: true)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous))
            .background(
                hovering ? Theme.panelHover : Color.clear,
                in: RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous))
            .onHover { hovering = $0 }
        }
        .buttonStyle(.tactile)
    }

    private var goal: String {
        switch item.kind {
        case .job(let row): return row.goal
        case .quarantined(let inner): return inner.goal ?? Lexicon.unreadableEntry
        }
    }

    private var id: String {
        switch item.kind {
        case .job(let row): return row.jobID
        case .quarantined(let inner): return inner.jobID ?? Lexicon.unknownID
        }
    }
}
