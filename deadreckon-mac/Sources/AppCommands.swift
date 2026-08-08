import AppKit
import DeadreckonKit
import SwiftUI

/// The real main-menu bar (APP-5): File > New Goal, View > Overview /
/// Search, and a Run menu whose enabled states follow the SAME durable
/// facts the on-screen buttons use — re-read from the LIVE FleetStore row,
/// never the navigation-time snapshot. Every destructive item routes to its
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
            Button("New Goal\u{2026}") {
                showMainWindow()
                router.pending = .newGoal
            }
            .keyboardShortcut("n", modifiers: .command)
        }

        CommandGroup(before: .toolbar) {
            Button("Overview") {
                showMainWindow()
                shell.request = .gateQueue
            }
            .keyboardShortcut("1", modifiers: .command)

            Button("Search Runs\u{2026}") {
                showMainWindow()
                shell.request = .search
            }
            .keyboardShortcut("k", modifiers: .command)

            Button("Library") {
                showMainWindow()
                shell.request = .library
            }
            .keyboardShortcut("l", modifiers: .command)

            Divider()
        }

        CommandMenu("Run") {
            // Same eligibility as the guide bar's visibility: a run open
            // and not terminal. The field's steerable{} gate (and any verb
            // refusal after it) stays authoritative — this only focuses.
            Button("Guide\u{2026}") {
                showMainWindow()
                shell.request = .focusSteer
            }
            .keyboardShortcut("g", modifiers: .command)
            .disabled(!(liveOpenedRow.map { $0.projection.phase != .terminal } ?? false))

            // Same facts as the decision bar and the row context menu: Stop
            // for non-terminal rows, Review & Approve for terminal rows
            // (the review sheet's PromoteGate stays the real guard).
            // Guarded: both route to their confirmation sheets.
            Button("Stop\u{2026}") {
                if let row = liveOpenedRow {
                    showMainWindow()
                    router.pending = .kill(row)
                }
            }
            .keyboardShortcut(.delete, modifiers: .command)
            .disabled(!(liveOpenedRow.map { $0.projection.phase != .terminal } ?? false))

            Button("Review & Approve\u{2026}") {
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
                    Task {
                        // The TCC Automation fallback must be visible on the
                        // menu path too (CONTRACTS.md honesty standard): the
                        // workbench rudder bar renders the copied note. This
                        // item is enabled only with a workbench open, so the
                        // surface exists whenever the note can fire.
                        if await TerminalLauncher.launch(command: command) == .copiedToPasteboard {
                            NotificationCenter.default.post(
                                name: .deadreckonTerminalFallback, object: nil)
                        }
                    }
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
