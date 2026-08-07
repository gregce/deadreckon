import XCTest

@testable import DeadreckonKit

/// The 5-question spine, mirrored from crates/deadreckon/src/tui/spine.rs
/// (run surface). Each of the five questions is exercised across running,
/// failing, and verifying-shaped states.
final class SpineDerivationTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_700_000_000)

    private func inputs(
        status: String = "executing",
        turn: Int = 6,
        phase: String? = "implement",
        spend: Double = 1.25,
        stateCap: Double? = 10,
        planCap: Double? = nil,
        pauseReason: String? = nil,
        failureReason: String? = nil,
        eventAge: TimeInterval? = 5,
        errorMessage: String? = nil,
        reshape: Bool = false,
        pendingSteers: Int = 0
    ) -> RunSpineInputs {
        RunSpineInputs(
            runID: "run-1", status: status, turn: turn, activePhaseName: phase,
            totalSpendUSD: spend, stateMaxSpendUSD: stateCap,
            launchPlanCeilingUSD: planCap, totalWallSeconds: 120,
            stateMaxWallSeconds: 600, launchPlanWallSeconds: nil,
            pauseReason: pauseReason, failureReason: failureReason,
            updatedAt: now.addingTimeInterval(-300),
            newestEventTimestamp: eventAge.map { now.addingTimeInterval(-$0) },
            newestErrorMessage: errorMessage,
            hasReshapeProposal: reshape, pendingSteerCount: pendingSteers)
    }

    // MARK: alive

    func testAliveIsLiveFromRecentEventAge() {
        let spine = SpineDerivation.deriveRun(inputs(eventAge: 7), now: now)
        XCTAssertEqual(spine.alive, .live(lastEventAgeSeconds: 7))
    }

    func testAliveGoesStalePastThirtySecondsAndExecutingGainsStallAttention() {
        let spine = SpineDerivation.deriveRun(inputs(eventAge: 45), now: now)
        XCTAssertEqual(spine.alive, .stale(ageSeconds: 45))
        XCTAssertTrue(spine.wrong.contains(
            SpineSnapshot.Attention(kind: .stall, summary: "no run event for 45s")))
    }

    func testAliveFallsBackToStateUpdatedAtWithoutEvents() {
        let spine = SpineDerivation.deriveRun(inputs(eventAge: nil), now: now)
        XCTAssertEqual(spine.alive, .stale(ageSeconds: 300))
    }

    func testFailedRunIsDeadWithTheRecordedReason() {
        let spine = SpineDerivation.deriveRun(
            inputs(status: "failed", failureReason: "provider exited"), now: now)
        XCTAssertEqual(spine.alive, .dead(reason: "provider exited"))
        XCTAssertTrue(spine.wrong.contains(
            SpineSnapshot.Attention(kind: .failure, summary: "provider exited")))
    }

    func testKilledRunIsDeadWithFallbackWords() {
        let spine = SpineDerivation.deriveRun(inputs(status: "killed"), now: now)
        XCTAssertEqual(spine.alive, .dead(reason: "run killed"))
        XCTAssertTrue(spine.wrong.contains(
            SpineSnapshot.Attention(kind: .killed, summary: "run killed")))
    }

    func testCompletedRunIsDone() {
        let spine = SpineDerivation.deriveRun(inputs(status: "completed"), now: now)
        XCTAssertEqual(spine.alive, .done)
    }

    // MARK: doing

    func testDoingUsesGlossaryStatusWordTurnAndActivePhase() {
        let spine = SpineDerivation.deriveRun(
            inputs(status: "executing", turn: 14, phase: "verify"), now: now)
        XCTAssertEqual(spine.doing, "running - turn 14 - verify")
    }

    func testDoingWithoutActivePhaseSaysSo() {
        let spine = SpineDerivation.deriveRun(inputs(phase: nil), now: now)
        XCTAssertEqual(spine.doing, "running - turn 6 - no active phase")
    }

    // MARK: on track

    func testOnTrackReadsSpendAndCeilingFromState() {
        let spine = SpineDerivation.deriveRun(inputs(spend: 2.5, stateCap: 25), now: now)
        XCTAssertEqual(spine.onTrack.spendUsedUSD, 2.5)
        XCTAssertEqual(spine.onTrack.spendCeilingUSD, 25)
        XCTAssertEqual(spine.onTrack.turnsUsed, 6)
    }

    func testLaunchPlanCeilingWinsOverStateCeiling() {
        // Mirrors launch_plan_spend_ceiling(...).or(state.max_spend_usd).
        let spine = SpineDerivation.deriveRun(
            inputs(stateCap: 10, planCap: 12.5), now: now)
        XCTAssertEqual(spine.onTrack.spendCeilingUSD, 12.5)
    }

    // MARK: wrong

    func testPauseReasonLeadsTheAttentionList() {
        let spine = SpineDerivation.deriveRun(
            inputs(pauseReason: "spend cap reached", errorMessage: "429 from provider"),
            now: now)
        XCTAssertEqual(spine.wrong.first,
            SpineSnapshot.Attention(kind: .pausedAtCap, summary: "spend cap reached"))
        XCTAssertTrue(spine.wrong.contains(
            SpineSnapshot.Attention(kind: .providerError, summary: "429 from provider")))
    }

    func testCleanRunningStateHasNoAttention() {
        let spine = SpineDerivation.deriveRun(inputs(), now: now)
        XCTAssertEqual(spine.wrong, [])
        XCTAssertEqual(spine.wrongText, "none")
    }

    func testPendingSteersSurfaceAsAttention() {
        let spine = SpineDerivation.deriveRun(inputs(pendingSteers: 2), now: now)
        XCTAssertTrue(spine.wrong.contains(SpineSnapshot.Attention(
            kind: .steerPending, summary: "2 pending steers waiting for delivery")))
    }

    // MARK: next

    func testNextIsAttachWhileExecuting() {
        XCTAssertEqual(SpineDerivation.deriveRun(inputs(), now: now).next,
                       "deadreckon attach run-1")
    }

    func testNextIsResumeAfterFailure() {
        XCTAssertEqual(
            SpineDerivation.deriveRun(inputs(status: "failed"), now: now).next,
            "deadreckon resume run-1")
    }

    func testNextIsFinishWhenCompleted() {
        XCTAssertEqual(
            SpineDerivation.deriveRun(inputs(status: "completed"), now: now).next,
            "deadreckon finish run-1")
    }

    func testReshapeProposalWinsNextAndAddsAttention() {
        let spine = SpineDerivation.deriveRun(inputs(reshape: true), now: now)
        XCTAssertEqual(spine.next, "deadreckon reshape run-1")
        XCTAssertTrue(spine.wrong.contains(SpineSnapshot.Attention(
            kind: .reshapeProposed,
            summary: "reshape proposal waiting for explicit operator acceptance")))
        // The proposal alone must not mark the run paused or dead
        // (spine.rs reshape_proposal_does_not_mark_run_paused_or_dead).
        XCTAssertEqual(spine.alive, .live(lastEventAgeSeconds: 5))
        XCTAssertFalse(spine.wrong.contains(where: { $0.kind == .pausedAtCap }))
    }

    func testNextIsExactlyOneCommand() {
        for status in ["pending", "planned", "executing", "completed", "failed", "killed"] {
            let next = SpineDerivation.deriveRun(inputs(status: status), now: now).next
            XCTAssertFalse(next.contains("\n"), next)
            XCTAssertFalse(next.contains(" && "), next)
            XCTAssertFalse(next.contains(" ; "), next)
            XCTAssertFalse(next.contains(" | "), next)
        }
    }

    // MARK: band text (exact spine.rs spine_plain_lines parity: plain
    // hyphens everywhere, matching on_track_text/attention_text; the
    // CONTRACTS.md "mirrors the wording" claim is load-bearing)

    func testOnTrackTextMatchesRustFormatterExactly() {
        // on_track_text: "{gate} - {spend} - {turns}" with gate "gate -"
        // when no counts exist (runs never carry gate counts).
        let spine = SpineDerivation.deriveRun(inputs(spend: 1.25, stateCap: 10), now: now)
        XCTAssertEqual(spine.onTrackText, "gate - - $1.25/$10.00 - turn 6")
    }

    func testOnTrackTextUnboundedSpendWording() {
        let spine = SpineDerivation.deriveRun(inputs(spend: 0, stateCap: nil), now: now)
        XCTAssertEqual(spine.onTrackText, "gate - - spend unbounded - turn 6")
    }

    func testWrongTextUsesPlainHyphenLikeAttentionText() {
        let spine = SpineDerivation.deriveRun(
            inputs(pauseReason: "spend cap reached"), now: now)
        XCTAssertEqual(spine.wrongText, "1 attention - spend cap reached")
    }

    func testAliveTextDeadUsesPlainHyphen() {
        let spine = SpineDerivation.deriveRun(
            inputs(status: "failed", failureReason: "provider exited"), now: now)
        XCTAssertEqual(spine.aliveText, "dead - provider exited")
    }

    // MARK: unknown vocabulary

    func testUnknownStatusDerivesHonestStaleNeverGuessedLive() {
        let spine = SpineDerivation.deriveRun(inputs(status: "warp-drive"), now: now)
        XCTAssertEqual(spine.alive, .stale(ageSeconds: 300))
        XCTAssertEqual(spine.doing, "warp-drive - turn 6 - implement")
        XCTAssertEqual(spine.next, "deadreckon attach run-1")
    }
}
