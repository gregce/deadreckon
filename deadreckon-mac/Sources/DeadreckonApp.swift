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
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    let fleet = FleetStore()
    var mainWindow: NSWindow?
    private var terminationPrepared = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        fleet.start()
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
        let patience: TimeInterval = 2
        let signaled = fleet.shutdown(patience: patience)
        guard signaled > 0 else { return .terminateNow }
        // 0.5s past patience gives the SIGKILL (scheduled at +patience) room
        // to be delivered before the process goes away.
        replyWhenChildrenExit(deadline: Date().addingTimeInterval(patience + 0.5))
        return .terminateLater
    }

    private func replyWhenChildrenExit(deadline: Date) {
        if fleet.inFlightChildren == 0 || Date() >= deadline {
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
        let hostingController = NSHostingController(rootView: GateQueueView(store: fleet))

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
        // tile unless the icon is set at runtime. The AppIcon set ships
        // empty until APP-5 polish, so this lookup returns nil (generic
        // tile) today; the assignment is wired now so dropping in the PNGs
        // later fixes the Dock icon with no code change.
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
            MenuBarContent(store: appDelegate.fleet) {
                appDelegate.showMainWindow()
            }
        } label: {
            MenuBarGlyph(store: appDelegate.fleet)
        }
        .menuBarExtraStyle(.menu)
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
            // A template glyph with a count is the honest badge: the number
            // of decisions waiting, not a colored guess.
            Image(systemName: "\(min(count, 50)).circle.fill")
        case .degraded:
            // Amber-class attention without a decision count: a confirmed
            // stale lease or the Watchkeeper down (design 2.4.1). Distinct
            // from the unavailable triangle; the tooltip says which.
            Image(systemName: "exclamationmark.circle")
        case .live:
            // The live variant: visually distinct from the idle helm while
            // anything runs or verifies (the exemplar's live-icon pattern).
            Image(systemName: "sailboat.fill")
        case .idle, .loading:
            if anchorSymbolAvailable {
                Image(systemName: "helm")
            } else {
                Text("\u{2693}\u{FE0E}")
            }
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

/// The status item menu: top queue rows (decision-ready first), then Open
/// and Quit. The full triage popover is APP-5; each row here just opens the
/// main window on the queue.
struct MenuBarContent: View {
    @ObservedObject var store: FleetStore
    let openMainWindow: () -> Void

    var body: some View {
        Group {
            Text(statusLine)

            let top = topRows
            if !top.isEmpty {
                Divider()
                ForEach(top) { item in
                    Button {
                        openMainWindow()
                    } label: {
                        Text(menuTitle(for: item))
                    }
                }
            }

            Divider()
            Button("Open deadreckon") {
                openMainWindow()
            }
            .keyboardShortcut("o")

            Divider()
            Button("Quit deadreckon") {
                NSApp.terminate(nil)
            }
            .keyboardShortcut("q")
        }
    }

    private var statusLine: String {
        switch store.fleet {
        case .loading: return "Reading the fleet"
        case .unavailable: return "Fleet unavailable"
        case .loaded(let queue): return queue.summaryLine
        }
    }

    /// Queue order already ranks decision-readiness first per section; walk
    /// the sections in taxonomy order and take the first five.
    private var topRows: [QueueItem] {
        Array(store.queue.allItems.prefix(5))
    }

    private func menuTitle(for item: QueueItem) -> String {
        switch item.kind {
        case .job(let row):
            let word = item.section == .atTheGate
                ? "at the gate"
                : GlossaryText.statusWord(row.status)
            return "\(row.goal) \u{00B7} \(word)"
        case .quarantined(let inner):
            return "\(inner.goal ?? inner.jobID ?? "row") \u{00B7} \(GlossaryText.unknownState)"
        }
    }
}
