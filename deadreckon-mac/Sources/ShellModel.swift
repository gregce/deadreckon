import DeadreckonKit
import Foundation
import SwiftUI

/// Shared shell state between the AppKit menu bar, the notification router,
/// and the SwiftUI window content (APP-5). Two responsibilities only:
/// pending navigation requests (consumed by GateQueueView, which owns the
/// NavigationStack) and the currently-open workbench item (published by
/// GateQueueView so menu enablement can follow the same facts the buttons
/// use). No fleet facts live here; the FleetStore stays the read model.
@MainActor
final class ShellModel: ObservableObject {
    enum Request: Equatable {
        /// Pop the window to the Gate Queue home.
        case gateQueue
        /// Show the Command-K palette.
        case search
        /// Open the window onto a job's workbench.
        case openJob(String)
        /// Open the window onto the job AND the promote sheet (the
        /// notification's Review at Gate action). The sheet's own gate stays
        /// authoritative; this only navigates.
        case reviewAtGate(String)
        /// Focus the workbench rudder field (Job > Steer). The field's
        /// steerable{} gating stays authoritative.
        case focusSteer
    }

    /// Consumed by GateQueueView. openJob/reviewAtGate stay pending while
    /// the fleet is still loading, then resolve or drop once it is loaded.
    @Published var request: Request?

    /// The workbench item currently open in the window (nil at the queue
    /// home). A snapshot from navigation time; menu enablement re-reads the
    /// LIVE row from the FleetStore by id, same discipline as the Binnacle.
    @Published var openedItem: QueueItem?
}

extension Notification.Name {
    /// Posted by the shell when Job > Steer wants the rudder field focused.
    static let deadreckonFocusSteer = Notification.Name("deadreckonFocusSteer")
}
