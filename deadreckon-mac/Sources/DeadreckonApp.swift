import AppKit
import Foundation
import SwiftUI

/// Manages the main window, handles reopen events, and flips the activation
/// policy so the Dock icon appears only while the desktop window is open
/// (the specstory-mac Granola pattern for an LSUIElement app).
///
/// Deliberately NOT copied from the exemplar: its debug env-var UI hooks in
/// the app delegate (a named scar in the design doc). Automated verification
/// hooks, when needed, belong in the Kit behind contracts.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    var mainWindow: NSWindow?

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        showMainWindow()
        return true
    }

    /// Builds the window lazily so it never depends on launch ordering.
    private func makeWindowIfNeeded() {
        guard mainWindow == nil else { return }
        let hostingController = NSHostingController(rootView: PhasePlanView())

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
    }

    func windowWillClose(_ notification: Notification) {
        // Back to a pure menu bar presence when the desktop window closes.
        NSApp.setActivationPolicy(.accessory)
    }
}

@main
struct DeadreckonApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        MenuBarExtra {
            Button("Open deadreckon") {
                appDelegate.showMainWindow()
            }
            .keyboardShortcut("o")
            Divider()
            Button("Quit deadreckon") {
                NSApp.terminate(nil)
            }
            .keyboardShortcut("q")
        } label: {
            MenuBarGlyph()
        }
        .menuBarExtraStyle(.menu)
    }
}

/// Anchor glyph placeholder. APP-5 replaces this with the state-driven
/// template/live/badged icon set (template when idle, colored when jobs run,
/// badged on needs-decision, stale-lease, or supervisor-down).
struct MenuBarGlyph: View {
    var body: some View {
        if NSImage(systemSymbolName: "helm", accessibilityDescription: nil) != nil {
            Image(systemName: "helm")
        } else {
            Text("\u{2693}\u{FE0E}")
        }
    }
}
