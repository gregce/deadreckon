import XCTest

@testable import DeadreckonKit

/// §R2: the first-run completeness derivation. Unknown facts never claim
/// incompleteness — a setup panel summoned by a broken probe would nag a
/// configured operator forever.
final class SetupDerivationTests: XCTestCase {
    func testAllUnknownIsNeitherIncompleteNorResolved() {
        let inputs = SetupDerivation.Inputs()
        XCTAssertFalse(SetupDerivation.isIncomplete(inputs))
        XCTAssertFalse(SetupDerivation.isResolved(inputs))
    }

    func testConfigAbsentIsIncomplete() {
        XCTAssertTrue(SetupDerivation.isIncomplete(
            .init(configPresent: false, agentRouteCount: 2, serviceNotInstalled: false)))
    }

    func testZeroAgentRoutesIsIncomplete() {
        XCTAssertTrue(SetupDerivation.isIncomplete(
            .init(configPresent: true, agentRouteCount: 0, serviceNotInstalled: false)))
    }

    func testServiceNotInstalledIsIncomplete() {
        XCTAssertTrue(SetupDerivation.isIncomplete(
            .init(configPresent: true, agentRouteCount: 2, serviceNotInstalled: true)))
    }

    /// Stopped/degraded service states are Settings remediation, not
    /// first-run: only a positive "not installed" summons the panel.
    func testServiceStoppedDoesNotSummonThePanel() {
        XCTAssertFalse(SetupDerivation.isIncomplete(
            .init(configPresent: true, agentRouteCount: 2, serviceNotInstalled: false)))
    }

    func testAllKnownGoodIsCompleteAndResolved() {
        let inputs = SetupDerivation.Inputs(
            configPresent: true, agentRouteCount: 1, serviceNotInstalled: false)
        XCTAssertFalse(SetupDerivation.isIncomplete(inputs))
        XCTAssertTrue(SetupDerivation.isResolved(inputs))
    }

    /// A known-bad fact decides immediately, resolved or not: the panel may
    /// appear while other probes are still answering.
    func testKnownBadFactDecidesBeforeFullResolution() {
        let inputs = SetupDerivation.Inputs(configPresent: false)
        XCTAssertTrue(SetupDerivation.isIncomplete(inputs))
        XCTAssertFalse(SetupDerivation.isResolved(inputs))
    }
}
