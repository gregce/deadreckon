import AppKit
import DeadreckonKit
import SwiftUI

/// The Library browser (SETTINGS-SCREENS-SPEC §R3): a modest read-only
/// table over `library list --json` — approved results, promoted. Renders
/// in the main window center (one window, one center, selection-driven);
/// scope toggle rides the documented `--all` flag; filtering is client-side
/// over goal / project / run id. Clicking a row whose run still exists in
/// the fleet opens that run (the evidence lives there); otherwise the row
/// is inert facts.
struct LibraryView: View {
    @ObservedObject var store: FleetStore
    /// Open a fleet run (the artifact's run id still resolves).
    let onOpenRun: (QueueItem) -> Void

    @StateObject private var library = LibraryStore(cli: WriteCLI.client)
    @State private var query = ""

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(Theme.border)
            content
        }
        .background(Theme.windowBg)
        .task { await library.load() }
    }

    // MARK: Header

    private var header: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text("Library")
                    .font(Theme.title)
                    .foregroundStyle(Theme.textPrimary)
                if let envelope = library.envelope {
                    Text("\(envelope.artifacts.count) artifact\(envelope.artifacts.count == 1 ? "" : "s")")
                        .font(Theme.small)
                        .foregroundStyle(Theme.textTertiary)
                        .monospacedDigit()
                }
                Spacer()
                scopeToggle
            }
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textTertiary)
                TextField("Filter by goal, project, or run id\u{2026}", text: $query)
                    .textFieldStyle(.plain)
                    .font(Theme.body(12))
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .inputChrome()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    /// [This project | All projects] — the CLI resolves "this project"
    /// from the working directory; the app's seat is not project-scoped, so
    /// All projects is the default.
    private var scopeToggle: some View {
        HStack(spacing: 2) {
            TabButton(title: "This project", active: !library.allProjects) {
                guard library.allProjects else { return }
                library.allProjects = false
                Task { await library.load() }
            }
            TabButton(title: "All projects", active: library.allProjects) {
                guard !library.allProjects else { return }
                library.allProjects = true
                Task { await library.load() }
            }
        }
    }

    // MARK: Content

    @ViewBuilder private var content: some View {
        switch library.state {
        case .idle, .loading:
            VStack(spacing: 10) {
                ProgressView().controlSize(.small)
                Text("Reading the library\u{2026}")
                    .font(Theme.body(12))
                    .foregroundStyle(Theme.textSecondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .unavailable(let reason):
            VStack(spacing: 12) {
                Image(systemName: "exclamationmark.triangle")
                    .font(.system(size: 22, weight: .medium))
                    .foregroundStyle(Theme.warn)
                Text("Can\u{2019}t read the library")
                    .font(Theme.heading)
                    .foregroundStyle(Theme.textPrimary)
                Text(reason)
                    .font(Theme.monoM)
                    .foregroundStyle(Theme.textSecondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 460)
                    .textSelection(.enabled)
                HStack(spacing: 5) {
                    Text("The CLI still works:")
                        .font(Theme.base)
                        .foregroundStyle(Theme.textSecondary)
                    Text("deadreckon library list")
                        .font(Theme.monoL)
                        .foregroundStyle(Theme.textPrimary)
                        .textSelection(.enabled)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(24)
        case .loaded(let envelope):
            if envelope.artifacts.isEmpty {
                emptyState
            } else {
                artifactList(envelope)
            }
        }
    }

    private var emptyState: some View {
        VStack(spacing: 14) {
            Text("Nothing in the library yet")
                .font(Theme.display)
                .foregroundStyle(Theme.textPrimary)
            Text("Approved results are promoted here \u{2014} approve a verified run and it lands in the library.")
                .font(Theme.base)
                .foregroundStyle(Theme.textSecondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 440)
            Text("deadreckon library list")
                .font(Theme.monoL)
                .foregroundStyle(Theme.textPrimary)
                .textSelection(.enabled)
                .padding(14)
                .background(Theme.well,
                            in: RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous)
                    .strokeBorder(Theme.border, lineWidth: 1))
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(24)
    }

    @ViewBuilder private func artifactList(_ envelope: LibraryListEnvelope) -> some View {
        let filtered = LibraryStore.filter(envelope.artifacts, query: query)
        ScrollView {
            VStack(alignment: .leading, spacing: 6) {
                if !query.isEmpty {
                    Text("\(filtered.count) / \(envelope.artifacts.count) match")
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.textTertiary)
                        .monospacedDigit()
                }
                if envelope.unreadableCount > 0 {
                    Text("\(envelope.unreadableCount) unreadable artifact row\(envelope.unreadableCount == 1 ? "" : "s") skipped")
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.warn)
                }
                VStack(spacing: 0) {
                    ForEach(Array(filtered.enumerated()), id: \.element.id) { index, artifact in
                        if index > 0 { Divider().overlay(Theme.border) }
                        LibraryRow(
                            artifact: artifact,
                            fleetItem: fleetItem(for: artifact),
                            onOpenRun: onOpenRun)
                    }
                }
                .cardChrome()
            }
            .padding(20)
            .frame(maxWidth: 860, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
    }

    /// The fleet item whose id matches the artifact's run id, if the run
    /// still exists — the row becomes a doorway to its evidence.
    private func fleetItem(for artifact: LibraryArtifact) -> QueueItem? {
        store.queue.allItems.first(where: { $0.id == artifact.manifest.runID })
    }
}

/// One artifact row (40px): goal · project (plain folder name; full scope
/// in the tooltip, mono) · promoted time · size facts when present · run id
/// mono. Hover + context menu: Reveal in Finder, Copy run id.
private struct LibraryRow: View {
    let artifact: LibraryArtifact
    let fleetItem: QueueItem?
    let onOpenRun: (QueueItem) -> Void

    @State private var hovering = false

    var body: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(artifact.manifest.goal)
                    .font(Theme.baseMedium)
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                factsLine
            }
            Spacer(minLength: 12)
            if hovering {
                Button("Reveal in Finder") { reveal() }
                    .buttonStyle(.themeStandard(compact: true))
            }
            if fleetItem != nil {
                Image(systemName: "chevron.right")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(Theme.textTertiary)
                    .help("This run is still in your runs \u{2014} open its evidence")
            }
        }
        .padding(.horizontal, 12)
        .frame(minHeight: 40)
        .background(hovering ? Theme.panelHover : .clear)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture {
            if let fleetItem { onOpenRun(fleetItem) }
        }
        .contextMenu {
            Button("Reveal in Finder") { reveal() }
            Button("Copy run id") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(artifact.manifest.runID, forType: .string)
            }
        }
    }

    private var factsLine: some View {
        HStack(spacing: 6) {
            Text(Lexicon.projectName(artifact.manifest.scope))
                .foregroundStyle(Theme.textSecondary)
                .help(artifact.manifest.scope)
            if let promoted = artifact.manifest.promotedAt {
                dot
                Text("promoted \(Lexicon.relativeTime(promoted))")
                    .foregroundStyle(Theme.textTertiary)
                    .monospacedDigit()
            }
            if let files = artifact.manifest.payloadFiles,
               let bytes = artifact.manifest.payloadBytes {
                dot
                Text("\(files) file\(files == 1 ? "" : "s") \u{00B7} \(DocsView.bytes(bytes))")
                    .foregroundStyle(Theme.textTertiary)
                    .monospacedDigit()
            }
            dot
            Text(String(artifact.manifest.runID.prefix(8)) + "\u{2026}")
                .font(Theme.mono(10))
                .foregroundStyle(Theme.textTertiary)
                .help(artifact.manifest.runID)
        }
        .font(Theme.small)
        .lineLimit(1)
    }

    private var dot: some View {
        Text("\u{00B7}").foregroundStyle(Theme.textTertiary)
    }

    private func reveal() {
        NSWorkspace.shared.activateFileViewerSelecting(
            [URL(fileURLWithPath: artifact.path)])
    }
}
