import XCTest

@testable import DeadreckonKit

/// Menubar state derivation: fixed precedence
/// (unavailable > attention > degraded > live > idle; loading before the
/// first fetch). Degraded inputs (confirmed-stale leases, supervisor down)
/// are supplied by FleetStore; their debounce lives in FleetStoreTests.
final class MenuBarStateTests: XCTestCase {
    func testLoadingBeforeFirstFetch() {
        XCTAssertEqual(QueueDerivation.menuBarState(.loading), .loading)
    }

    func testUnavailableWhenBinaryMissing() {
        let state = QueueDerivation.menuBarState(
            .unavailable(reason: "The bundled CLI binary is missing"))
        XCTAssertEqual(state, .unavailable)
    }

    func testAttentionWhenGateSectionNonEmpty() {
        let queue = QueueDerivation.derive(rows: [
            FleetFixtures.row(
                id: "job-gate", phase: .terminal, outcome: .verified,
                receipt: FleetFixtures.validReceipt()),
        ])
        XCTAssertEqual(QueueDerivation.menuBarState(.loaded(queue)), .attention(1))
    }

    func testAttentionWhenDecisionShapedWaitingRowExists() {
        let queue = QueueDerivation.derive(rows: [
            FleetFixtures.row(id: "job-capped", phase: .waiting, stopReason: .spendCap),
        ])
        XCTAssertEqual(QueueDerivation.menuBarState(.loaded(queue)), .attention(1))
    }

    /// A pending decision outranks live activity: the badge wins.
    func testAttentionOutranksLive() {
        let queue = QueueDerivation.derive(rows: [
            FleetFixtures.row(
                id: "job-gate", phase: .terminal, outcome: .verified,
                receipt: FleetFixtures.validReceipt()),
            FleetFixtures.row(id: "job-running", phase: .running),
        ])
        XCTAssertEqual(QueueDerivation.menuBarState(.loaded(queue)), .attention(1))
    }

    func testLiveWhenRunningOrVerifying() {
        let queue = QueueDerivation.derive(rows: [
            FleetFixtures.row(id: "job-running", phase: .running),
            FleetFixtures.row(id: "job-checks", phase: .verifyingChecks),
        ])
        XCTAssertEqual(QueueDerivation.menuBarState(.loaded(queue)), .live(2))
    }

    /// Queued and non-decision waiting rows are not "live": template glyph.
    func testIdleWhenFleetQuietOrOnlyQueued() {
        XCTAssertEqual(QueueDerivation.menuBarState(.loaded(.empty)), .idle)
        let queue = QueueDerivation.derive(rows: [
            FleetFixtures.row(id: "job-queued", phase: .queued),
            FleetFixtures.row(id: "job-waiting", phase: .waiting, stopReason: .transientProvider),
        ])
        XCTAssertEqual(QueueDerivation.menuBarState(.loaded(queue)), .idle)
    }

    /// Wrecked rows alone neither badge nor light up: returning to a failed
    /// fleet is an idle glyph with honest rows behind it.
    func testWreckedAloneStaysIdle() {
        let queue = QueueDerivation.derive(rows: [
            FleetFixtures.row(id: "job-failed", phase: .terminal, outcome: .failed),
        ])
        XCTAssertEqual(QueueDerivation.menuBarState(.loaded(queue)), .idle)
    }

    // MARK: Degraded (design 2.4.1: badge on stale-lease / supervisor-down)

    /// A confirmed-stale lease or a stopped Watchkeeper outranks plain live
    /// activity: the amber the operator returns for.
    func testDegradedOutranksLive() {
        let queue = QueueDerivation.derive(rows: [
            FleetFixtures.row(id: "job-running", phase: .running),
        ])
        XCTAssertEqual(
            QueueDerivation.menuBarState(.loaded(queue), staleLeaseCount: 1),
            .degraded(staleLeases: 1, supervisorDown: false))
        XCTAssertEqual(
            QueueDerivation.menuBarState(.loaded(queue), supervisorDown: true),
            .degraded(staleLeases: 0, supervisorDown: true))
    }

    /// A pending decision still outranks degradation: the count badge wins.
    func testAttentionOutranksDegraded() {
        let queue = QueueDerivation.derive(rows: [
            FleetFixtures.row(id: "job-capped", phase: .waiting, stopReason: .spendCap),
        ])
        XCTAssertEqual(
            QueueDerivation.menuBarState(.loaded(queue), staleLeaseCount: 2, supervisorDown: true),
            .attention(1))
    }

    /// A missing binary still outranks everything, degraded inputs included.
    func testUnavailableOutranksDegraded() {
        XCTAssertEqual(
            QueueDerivation.menuBarState(
                .unavailable(reason: "binary missing"), staleLeaseCount: 3, supervisorDown: true),
            .unavailable)
    }
}
