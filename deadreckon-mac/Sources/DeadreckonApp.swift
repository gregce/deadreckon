import AppKit
import DeadreckonKit
import Foundation
import SwiftUI

/// Manages the main window, handles reopen events, and flips the activation
/// policy so the Dock icon appears only while the desktop window is open
/// (the specstory-mac Granola pattern for an LSUIElement app).
///
/// Owns the FleetStore: window visibility drives the poll cadence, and quit
/// runs the exemplar `applicationShouldTerminate` teardown so in-flight CLI
/// children get SIGTERM (SIGKILL after patience) before the process — and
/// their pipes — go away.
///
/// Deliberately NOT copied from the exemplar: its debug env-var UI hooks in
/// the app delegate (a named scar in the design doc).
/// APP-4 fix pass: ONE production CLI client for every write surface (the
/// sheets, the workbench rudder, the popover quick-steer). Per-sheet clients
/// would each hold their own in-flight children invisible to the quit-time
/// teardown sweep — a `finish --yes` child could be orphaned. One shared
/// client keeps every write child inside `applicationShouldTerminate`'s
/// SIGTERM sweep and in-flight count. (FleetStore keeps its own read client,
/// already swept via `fleet.shutdown`.)
@MainActor
enum WriteCLI {
    static let client = DeadreckonCLIClient()
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    let fleet = FleetStore()
    /// APP-4: one router for every write surface — the queue, the workbench,
    /// and the popover all open the same confirmation sheets through it.
    let writeRouter = WriteSurfaceRouter()
    /// APP-5: navigation requests + the opened workbench item, shared
    /// between the main menu, the notification router, and the window.
    let shell = ShellModel()
    /// APP-5: the UNUserNotificationCenter side of the Kit's posting seam
    /// (lazy auth resolved per post, stable IDs — exemplar pattern).
    let notificationAdapter = UserNotificationAdapter()
    /// APP-5: tails notify.jsonl across the fleet and derives notifications
    /// with stable identities; all logic in the Kit, tested with fakes.
    private(set) lazy var attention = AttentionCenter(
        notifier: notificationAdapter,
        seenStore: NotificationSeenStore(),
        preferences: { AttentionPreferences.load(from: .standard) },
        rowsProvider: { [weak self] in
            guard let self, case .loaded = self.fleet.fleet else { return [] }
            return self.fleet.queue.allItems.compactMap(\.row)
        })
    var mainWindow: NSWindow?
    private var terminationPrepared = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        fleet.start()
        // Housekeeping: launch-plan scratch files from earlier sessions.
        MutationRunner.sweepLaunchPlanFiles()
        // Notification taps route through the Kit's pure derivation: Open
        // lands on the job, Review at Gate lands on the promote sheet.
        notificationAdapter.onRoute = { [weak self] route in
            guard let self else { return }
            self.showMainWindow()
            switch route {
            case .openJob(let jobID):
                self.shell.request = .openJob(jobID)
            case .reviewAtGate(let jobID):
                self.shell.request = .reviewAtGate(jobID)
            }
        }
        // Launch-time catch-up runs once the fleet first loads (the center
        // defers it while the rows provider is empty); the seen-set keeps
        // relaunches from re-firing anything.
        attention.start()
        // Crowded-menu-bar rescue: on a full menu bar macOS silently hides
        // overflow status items (no API opts out), and a hidden item makes
        // an LSUIElement app completely unreachable. Once the MenuBarExtra
        // has settled, check whether our status-bar window actually made it
        // on screen; if not, open the fleet window so launching the app
        // always lands the operator somewhere real.
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
            guard let self, self.mainWindow?.isVisible != true else { return }
            if !Self.statusItemIsOnScreen() {
                self.showMainWindow()
            }
        }
    }

    /// True when a status-bar-level window owned by this process is
    /// actually on screen. Own-process window info needs no screen
    /// recording permission; layer 25 is the status bar.
    private static func statusItemIsOnScreen() -> Bool {
        guard let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID)
            as? [[String: Any]]
        else {
            // Indeterminate: do not force a window the operator did not ask
            // for on machines where the item is probably fine.
            return true
        }
        let pid = Int32(ProcessInfo.processInfo.processIdentifier)
        return windows.contains { window in
            (window["kCGWindowOwnerPID"] as? Int32) == pid
                && (window["kCGWindowLayer"] as? Int) == 25
        }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        showMainWindow()
        return true
    }

    /// Stop polling, SIGTERM any in-flight children, then keep the process
    /// alive until the children actually exit — or past the SIGTERM patience
    /// so the SIGKILL escalation can fire for a child that ignores SIGTERM.
    /// Replying before either point would orphan the child (and skip the
    /// escalation entirely, since it is scheduled inside this process).
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard !terminationPrepared else { return .terminateNow }
        terminationPrepared = true
        // Read-only notify tails; stopped for symmetry, nothing to signal.
        attention.stop()
        let patience: TimeInterval = 2
        // Both clients: the fleet's read children AND the shared write
        // client's children (a mid-flight `finish --yes` must meet its
        // SIGTERM/SIGKILL here, never be orphaned).
        let signaled = fleet.shutdown(patience: patience)
            + WriteCLI.client.terminateInFlight(patience: patience)
        guard signaled > 0 else { return .terminateNow }
        // 0.5s past patience gives the SIGKILL (scheduled at +patience) room
        // to be delivered before the process goes away.
        replyWhenChildrenExit(deadline: Date().addingTimeInterval(patience + 0.5))
        return .terminateLater
    }

    private func replyWhenChildrenExit(deadline: Date) {
        if (fleet.inFlightChildren + WriteCLI.client.inFlightCount) == 0 || Date() >= deadline {
            NSApp.reply(toApplicationShouldTerminate: true)
            return
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { [weak self] in
            self?.replyWhenChildrenExit(deadline: deadline)
        }
    }

    /// Builds the window lazily so it never depends on launch ordering.
    private func makeWindowIfNeeded() {
        guard mainWindow == nil else { return }
        let hostingController = NSHostingController(
            rootView: GateQueueView(
                store: fleet, router: writeRouter, shell: shell, attention: attention))

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1080, height: 720),
            styleMask: [.titled, .closable, .resizable, .miniaturizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.contentViewController = hostingController
        window.title = "deadreckon"
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 760, height: 480)
        window.delegate = self
        window.setFrameAutosaveName("DeadreckonMainWindow")
        if !window.setFrameUsingName("DeadreckonMainWindow") {
            window.center()
        }
        mainWindow = window
    }

    func showMainWindow() {
        makeWindowIfNeeded()
        guard let mainWindow else { return }
        NSApp.setActivationPolicy(.regular)
        // An LSUIElement app that flips to .regular gets the generic Dock
        // tile unless the icon is set at runtime. APP-5 filled the AppIcon
        // set (flat anchor on deep blue; master SVG in scripts/app-icon.svg,
        // rendered via rsvg-convert), so this resolves the real icon.
        NSApp.applicationIconImage = NSImage(named: "AppIcon")
        mainWindow.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        // Window visible: fast poll cadence.
        fleet.setWindowVisible(true)
    }

    func windowWillClose(_ notification: Notification) {
        // Back to a pure menu bar presence when the desktop window closes;
        // the fleet keeps polling at the slower menubar cadence.
        NSApp.setActivationPolicy(.accessory)
        fleet.setWindowVisible(false)
    }
}

@main
struct DeadreckonApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        MenuBarExtra {
            // APP-4 (operator decision 4): the popover carries the same
            // write verbs as the window — quick-steer inline, and
            // Kill/Promote/Send back items that open the window directly
            // onto the respective confirmation sheet. The .window style
            // hosts the real controls a menu cannot.
            MenuBarPopover(store: appDelegate.fleet, router: appDelegate.writeRouter,
                           shell: appDelegate.shell) {
                appDelegate.showMainWindow()
            }
        } label: {
            MenuBarGlyph(store: appDelegate.fleet)
        }
        .menuBarExtraStyle(.window)

        // APP-5: the Settings scene (Command-comma lands in the app menu
        // automatically) and the real main-menu bar. The commands read the
        // LIVE fleet row for the opened job, so menu enablement follows the
        // same durable facts the on-screen buttons use.
        Settings {
            SettingsView(fleet: appDelegate.fleet, attention: appDelegate.attention)
        }
        .commands {
            DeadreckonCommands(
                fleet: appDelegate.fleet,
                router: appDelegate.writeRouter,
                shell: appDelegate.shell) {
                appDelegate.showMainWindow()
            }
        }
    }
}

/// State-driven menubar glyph (the exemplar's five-state enum pattern):
/// template anchor when idle, live variant while anything runs, badged when
/// a decision waits at the gate, honest error mark when the binary is gone.
struct MenuBarGlyph: View {
    @ObservedObject var store: FleetStore

    var body: some View {
        label
            .accessibilityLabel("deadreckon")
            .help(helpText)
    }

    @ViewBuilder private var label: some View {
        switch store.menuBarState {
        case .unavailable:
            Image(systemName: "exclamationmark.triangle")
        case .attention(let count):
            // The helm keeps the app's identity in the state the operator
            // most needs to FIND it; the decision count rides beside it as
            // the honest badge (a number, never a colored guess). An
            // anonymous "N.circle" glyph read as a different app entirely.
            HStack(spacing: 2) {
                identityGlyph
                Text("\(min(count, 50))")
                    .font(.system(size: 11, weight: .semibold))
                    .monospacedDigit()
            }
        case .degraded:
            // Amber-class attention without a decision count: a confirmed
            // stale lease or the Watchkeeper down (design 2.4.1). Distinct
            // from the unavailable triangle; the tooltip says which.
            Image(systemName: "exclamationmark.circle")
        case .live:
            // The live variant: visually distinct from the idle helm while
            // anything runs or verifies (the exemplar's live-icon pattern).
            // The helm-to-sailboat shape swap is a deliberate divergence,
            // recorded in CONTRACTS.md.
            Image(systemName: "sailboat.fill")
        case .idle, .loading:
            identityGlyph
        }
    }

    /// The brand anchor: the helm, or the anchor text glyph where the SF
    /// symbol is unavailable.
    @ViewBuilder private var identityGlyph: some View {
        if anchorSymbolAvailable {
            Image(systemName: "helm")
        } else {
            Text("\u{2693}\u{FE0E}")
        }
    }

    private var anchorSymbolAvailable: Bool {
        NSImage(systemSymbolName: "helm", accessibilityDescription: nil) != nil
    }

    private var helpText: String {
        switch store.menuBarState {
        case .unavailable: return "deadreckon: fleet unavailable"
        case .attention(let count): return "deadreckon: \(count) decision\(count == 1 ? "" : "s") waiting"
        case .degraded(let staleLeases, let supervisorDown):
            var parts: [String] = []
            if staleLeases > 0 {
                parts.append("\(staleLeases) stale lease\(staleLeases == 1 ? "" : "s")")
            }
            if supervisorDown { parts.append("supervisor stopped") }
            return "deadreckon: " + parts.joined(separator: ", ")
        case .live(let count): return "deadreckon: \(count) running"
        case .idle: return "deadreckon: fleet quiet"
        case .loading: return "deadreckon: reading the fleet"
        }
    }
}

// MenuBarContent (the APP-2 plain menu) was replaced by MenuBarPopover in
// APP-4: operator decision 4 gives the popover real write affordances, which
// a .menu-style status item cannot host.
