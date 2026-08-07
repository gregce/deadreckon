import DeadreckonKit
import SwiftUI

/// The main window (REDESIGN-SPEC §B1): a two-pane split — the sidebar's
/// projects → runs tree on the left, the selected run (or the Overview)
/// filling the center. Selection swaps the center in place; the old
/// queue → detail push is gone. Every write surface still opens its
/// confirmation sheet through the one WriteSurfaceRouter, the Command-K
/// palette overlays the window, and ShellModel requests (menu bar,
/// notifications, popover deep-links) land here.
struct MainWindowView: View {
    @ObservedObject var store: FleetStore
    @ObservedObject var router: WriteSurfaceRouter
    @ObservedObject var shell: ShellModel
    @ObservedObject var attention: AttentionCenter

    /// The selected run — a snapshot for identity; the center re-resolves
    /// the LIVE item by id every body pass so facts never go stale.
    @State private var selection: QueueItem?
    @State private var paletteShown = false
    /// Set by the fresh-install invitation: the next New Goal sheet opens
    /// with the folder chooser armed (§B4).
    @State private var newGoalArmsChooser = false

    var body: some View {
        ZStack {
            HStack(spacing: 0) {
                SidebarView(store: store, attention: attention,
                            selection: liveSelectionBinding,
                            onNewGoal: { router.pending = .newGoal })
                Divider().overlay(Theme.border)
                center
            }
            // The transparent-titlebar window (fullSizeContentView): the
            // ground must paint up behind the traffic lights too.
            .background(Theme.windowBg.ignoresSafeArea())

            // The window-level overlay layer: the palette renders ABOVE the
            // split view, so Command-K from any state shows a visible
            // palette; opening a match selects that run.
            if paletteShown {
                CommandPalette(store: store, isShown: $paletteShown) { item in
                    selection = item
                }
            }
        }
        // Layered Escape: the palette consumes it first while shown;
        // otherwise Escape steps back to the Overview.
        .onExitCommand {
            if paletteShown {
                paletteShown = false
            } else if selection != nil {
                selection = nil
            }
        }
        .sheet(item: $router.pending, onDismiss: { newGoalArmsChooser = false }) { surface in
            switch surface {
            case .newGoal:
                NewGoalSheet(chooseFolderOnOpen: newGoalArmsChooser)
            case .kill(let row):
                StopSheet(row: row)
            case .promote(let row):
                ReviewApproveSheet(
                    row: row,
                    fleet: store,
                    onSendBack: { router.pending = .sendBack(row) },
                    onKill: { router.pending = .kill(row) })
            case .sendBack(let row):
                SendBackSheet(row: row)
            }
        }
        .background(shortcutButtons)
        // The opened run item, published for menu enablement (APP-5).
        .onChange(of: selection) { _, newSelection in
            shell.openedItem = newSelection
        }
        // Navigation requests from the menu bar and notification actions.
        // openJob/reviewAtGate stay pending while the runs are loading and
        // resolve (or drop, honestly) once a loaded queue can answer.
        .onChange(of: shell.request) { _, _ in consumeShellRequest() }
        .onChange(of: store.lastRefreshed) { _, _ in consumeShellRequest() }
        .onAppear { consumeShellRequest() }
    }

    /// The freshest queue item for the selected id; falls back to the
    /// selection snapshot while a poll is in flight.
    private var liveSelection: QueueItem? {
        guard let selection else { return nil }
        return store.queue.allItems.first(where: { $0.id == selection.id }) ?? selection
    }

    /// The sidebar reads the live selection (so its highlight follows the
    /// row across polls) and writes the plain selection.
    private var liveSelectionBinding: Binding<QueueItem?> {
        Binding(get: { liveSelection }, set: { selection = $0 })
    }

    @ViewBuilder private var center: some View {
        if let item = liveSelection {
            RunDetailView(fleet: store, item: item)
                .environmentObject(router)
        } else {
            OverviewView(
                store: store,
                attention: attention,
                onOpen: { selection = $0 },
                onReview: { router.pending = .promote($0) },
                onNewGoal: { armed in
                    newGoalArmsChooser = armed
                    router.pending = .newGoal
                })
        }
    }

    private func consumeShellRequest() {
        guard let request = shell.request else { return }
        switch request {
        case .gateQueue:
            selection = nil
            shell.request = nil
        case .search:
            paletteShown = true
            shell.request = nil
        case .focusSteer:
            if selection != nil {
                NotificationCenter.default.post(name: .deadreckonFocusSteer, object: nil)
            }
            shell.request = nil
        case .openJob(let jobID):
            guard case .loaded = store.fleet else { return }  // retry on next poll
            if let item = store.queue.allItems.first(where: { $0.id == jobID }) {
                selection = item
            }
            shell.request = nil
        case .reviewAtGate(let jobID):
            guard case .loaded = store.fleet else { return }  // retry on next poll
            if let item = store.queue.allItems.first(where: { $0.id == jobID }),
               let row = item.row {
                selection = item
                // The review sheet's own gate (PromoteGate, live row) stays
                // authoritative; this only navigates onto it.
                router.pending = .promote(row)
            }
            shell.request = nil
        }
    }

    /// Hidden buttons so the shortcuts work regardless of focus.
    private var shortcutButtons: some View {
        Group {
            Button("") { router.pending = .newGoal }
                .keyboardShortcut("n", modifiers: .command)
            Button("") { paletteShown.toggle() }
                .keyboardShortcut("k", modifiers: .command)
        }
        .opacity(0)
        .frame(width: 0, height: 0)
        .accessibilityHidden(true)
    }
}
