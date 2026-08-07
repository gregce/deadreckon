import AppKit
import DeadreckonKit
import SwiftUI

/// The real main-menu bar (APP-5): File > New Job, View > Gate Queue /
/// Search, and a Job menu whose enabled states follow the SAME durable facts
/// the on-screen buttons use — re-read from the LIVE FleetStore row, never
/// the navigation-time snapshot. Every destructive item routes to its
/// confirmation sheet; nothing in a menu fires a verb directly.
struct DeadreckonCommands: Commands {
    @ObservedObject var fleet: FleetStore
    @ObservedObject var router: WriteSurfaceRouter
    @ObservedObject var shell: ShellModel
    let showMainWindow: () -> Void

    var body: some Commands {
        CommandGroup(replacing: .appInfo) {
            Button("About deadreckon") {
                NSApp.activate(ignoringOtherApps: true)
                NSApp.orderFrontStandardAboutPanel(options: aboutOptions)
            }
        }

        CommandGroup(replacing: .newItem) {
            Button("New Job\u{2026}") {
                showMainWindow()
                router.pending = .layCourse
            }
            .keyboardShortcut("n", modifiers: .command)
        }

        CommandGroup(before: .toolbar) {
            Button("Gate Queue") {
                showMainWindow()
                shell.request = .gateQueue
            }
            .keyboardShortcut("1", modifiers: .command)

            Button("Search Fleet") {
                showMainWindow()
                shell.request = .search
            }
            .keyboardShortcut("k", modifiers: .command)

            Divider()
        }

        CommandMenu("Job") {
            // Same eligibility as the workbench rudder's visibility: a job
            // open and not terminal. The field's steerable{} gate (and any
            // verb refusal after it) stays authoritative — this only focuses.
            Button("Steer") {
                showMainWindow()
                shell.request = .focusSteer
            }
            .disabled(!(liveOpenedRow.map { $0.projection.phase != .terminal } ?? false))

            // Same facts as the workbench decision bar and the row context
            // menu: Kill for non-terminal rows, Promote for terminal rows
            // (the Binnacle's PromoteGate stays the real guard). Guarded:
            // both route to their confirmation sheets.
            Button("Kill\u{2026}") {
                if let row = liveOpenedRow {
                    showMainWindow()
                    router.pending = .kill(row)
                }
            }
            .keyboardShortcut(.delete, modifiers: .command)
            .disabled(!(liveOpenedRow.map { $0.projection.phase != .terminal } ?? false))

            Button("Promote\u{2026}") {
                if let row = liveOpenedRow {
                    showMainWindow()
                    router.pending = .promote(row)
                }
            }
            .disabled(!(liveOpenedRow.map { $0.projection.phase == .terminal } ?? false))

            Divider()

            Button("Open in Terminal") {
                if let row = liveOpenedRow {
                    let command = "deadreckon attach \(row.jobID)"
                    Task { _ = await TerminalLauncher.launch(command: command) }
                }
            }
            .keyboardShortcut("t", modifiers: .command)
            .disabled(liveOpenedRow == nil)
        }
    }

    /// The LIVE row for the opened workbench item (falling back to the
    /// navigation snapshot only while the fleet has not re-polled yet).
    private var liveOpenedRow: FleetRow? {
        guard let opened = shell.openedItem else { return nil }
        if case .loaded = fleet.fleet,
           let live = fleet.queue.allItems.first(where: { $0.id == opened.id }) {
            return live.row
        }
        return opened.row
    }

    private var aboutOptions: [NSApplication.AboutPanelOptionKey: Any] {
        var options: [NSApplication.AboutPanelOptionKey: Any] = [:]
        if let binaryVersion = fleet.binaryVersion {
            options[.credits] = NSAttributedString(
                string: "vendored CLI: \(binaryVersion)",
                attributes: [.font: NSFont.systemFont(ofSize: 11)])
        }
        return options
    }
}
