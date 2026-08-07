import XCTest

@testable import DeadreckonKit

/// Fixture builders shared by the queue, menubar, and store tests. All rows
/// are built through FleetRow's public memberwise initializer so every test
/// speaks the committed M0 rollup shape.
enum FleetFixtures {
    static func row(
        id: String = "job-fixture",
        goal: String = "Fixture goal",
        status: String = "running",
        updatedAt: Date = Date(timeIntervalSince1970: 1_770_000_000),
        phase: JobPhase = .running,
        outcome: JobOutcome? = nil,
        stopReason: StopReason? = nil,
        lease: FleetRow.Lease? = nil,
        spend: FleetRow.Spend? = nil,
        provider: String? = "cli:codex",
        receipt: FleetRow.Receipt? = nil,
        gate: FleetRow.Gate? = nil
    ) -> FleetRow {
        FleetRow(
            jobID: id, scope: "deadreckon", goal: goal, status: status,
            updatedAt: updatedAt, attempts: 1, outcome: outcome,
            stopReason: stopReason,
            projection: FleetRow.Projection(
                phase: phase, outcome: outcome, stopReason: stopReason,
                attemptCount: 1, caveats: []),
            lease: lease, spend: spend, provider: provider, sandbox: nil,
            receipt: receipt, gate: gate)
    }

    static func validReceipt() -> FleetRow.Receipt {
        FleetRow.Receipt(present: true, verified: .valid, error: nil)
    }

    static func invalidReceipt() -> FleetRow.Receipt {
        FleetRow.Receipt(present: true, verified: .invalid, error: "digest mismatch")
    }

    static func freshLease(age: Int = 2) -> FleetRow.Lease {
        FleetRow.Lease(
            ownerID: "supervisor-1", epoch: 3, heartbeatAgeSeconds: age,
            expiresAt: Date(timeIntervalSince1970: 1_770_000_600), fresh: true)
    }

    static func staleLease(age: Int = 71) -> FleetRow.Lease {
        FleetRow.Lease(
            ownerID: "supervisor-1", epoch: 3, heartbeatAgeSeconds: age,
            expiresAt: Date(timeIntervalSince1970: 1_770_000_060), fresh: false)
    }
}

final class QueueDerivationTests: XCTestCase {
    // MARK: - Section membership

    func testTerminalVerifiedWithValidReceiptRanksAtTheGate() {
        let row = FleetFixtures.row(
            id: "job-gate", status: "verified", phase: .terminal,
            outcome: .verified, stopReason: .verified,
            receipt: FleetFixtures.validReceipt(),
            gate: FleetRow.Gate(attempt: 2, nPassed: 5, nTotal: 5))
        let queue = QueueDerivation.derive(rows: [row])
        XCTAssertEqual(queue.atTheGate.map(\.id), ["job-gate"])
        XCTAssertTrue(queue.wrecked.isEmpty)
    }

    /// Trust rule 5: verified_proof_invalid is its own state and an invalid
    /// receipt keeps the row OUT of AT THE GATE, unconditionally.
    func testInvalidReceiptStaysOutOfAtTheGate() {
        let row = FleetFixtures.row(
            id: "job-tampered", status: "verified_proof_invalid", phase: .terminal,
            outcome: .verified, receipt: FleetFixtures.invalidReceipt())
        let queue = QueueDerivation.derive(rows: [row])
        XCTAssertTrue(queue.atTheGate.isEmpty)
        XCTAssertEqual(queue.wrecked.map(\.id), ["job-tampered"])
    }

    /// A "verified" outcome with no receipt rollup at all is not decision-ready
    /// either: no proof, no gate.
    func testMissingReceiptStaysOutOfAtTheGate() {
        let row = FleetFixtures.row(
            id: "job-unproven", phase: .terminal, outcome: .verified, receipt: nil)
        let queue = QueueDerivation.derive(rows: [row])
        XCTAssertTrue(queue.atTheGate.isEmpty)
        XCTAssertEqual(queue.wrecked.map(\.id), ["job-unproven"])
    }

    func testVerifyingPhasesRankApproaching() {
        let checks = FleetFixtures.row(id: "job-checks", phase: .verifyingChecks)
        let meaning = FleetFixtures.row(id: "job-meaning", phase: .verifyingMeaning)
        let queue = QueueDerivation.derive(rows: [checks, meaning])
        XCTAssertEqual(Set(queue.approaching.map(\.id)), ["job-checks", "job-meaning"])
    }

    func testRunningQueuedWaitingRankUnderway() {
        let rows = [
            FleetFixtures.row(id: "job-running", phase: .running),
            FleetFixtures.row(id: "job-queued", phase: .queued),
            FleetFixtures.row(id: "job-waiting", phase: .waiting, stopReason: .transientProvider),
        ]
        let queue = QueueDerivation.derive(rows: rows)
        XCTAssertEqual(Set(queue.underway.map(\.id)), ["job-running", "job-queued", "job-waiting"])
    }

    func testTerminalFailuresRankWrecked() {
        let rows = [
            FleetFixtures.row(id: "job-failed", phase: .terminal, outcome: .failed),
            FleetFixtures.row(id: "job-blocked", phase: .terminal, outcome: .blocked, stopReason: .lostContainment),
            FleetFixtures.row(id: "job-cancelled", phase: .terminal, outcome: .cancelled),
            FleetFixtures.row(id: "job-budget", phase: .terminal, outcome: .budgetExhausted),
        ]
        let queue = QueueDerivation.derive(rows: rows)
        XCTAssertEqual(queue.wrecked.count, 4)
        XCTAssertTrue(queue.atTheGate.isEmpty)
    }

    /// A terminal needs_review outcome (judge uncertain or asked for
    /// revision) is an operator decision by definition: its own amber
    /// section below AT THE GATE, never buried beside the wrecks (design C1:
    /// "ranked lower and painted amber, never hidden").
    func testTerminalNeedsReviewGetsItsOwnSectionNotWrecked() {
        let row = FleetFixtures.row(
            id: "job-uncertain", status: "needs_review", phase: .terminal,
            outcome: .needsReview, stopReason: .semanticUncertain)
        let queue = QueueDerivation.derive(rows: [row])
        XCTAssertEqual(queue.needsReview.map(\.id), ["job-uncertain"])
        XCTAssertTrue(queue.wrecked.isEmpty)
        XCTAssertTrue(queue.atTheGate.isEmpty, "needs_review never claims the gate")
        XCTAssertTrue(queue.needsReview[0].needsDecision)
    }

    /// The receipt.valid gate for AT THE GATE is unchanged: needs_review
    /// rows count as decisions and light the badge, but never render as
    /// verified-and-waiting.
    func testNeedsReviewCountsAsDecisionAndBadges() {
        let queue = QueueDerivation.derive(rows: [
            FleetFixtures.row(
                id: "job-uncertain", status: "needs_review", phase: .terminal,
                outcome: .needsReview, stopReason: .semanticUncertain),
        ])
        XCTAssertEqual(queue.decisionCount, 1)
        XCTAssertEqual(QueueDerivation.menuBarState(.loaded(queue)), .attention(1))
        XCTAssertEqual(queue.summaryLine, "1 need review")
    }

    /// An unknown phase word (vocabulary newer than the app) quarantines the
    /// row into the honest unknown section; it never crashes and never gets
    /// guessed into a real section.
    func testUnknownPhaseIsQuarantinedNotCrashed() {
        let row = FleetFixtures.row(id: "job-strange", phase: .unknown)
        let queue = QueueDerivation.derive(rows: [row])
        XCTAssertEqual(queue.unknown.map(\.id), ["job-strange"])
        XCTAssertEqual(queue.items(in: .unknown).first?.section, .unknown)
    }

    func testQuarantinedDecodeFailuresLandInUnknownSection() {
        let quarantined = QuarantinedRow(jobID: "job-corrupt", goal: "Half a row", reason: "row is missing \"projection\"")
        let queue = QueueDerivation.derive(rows: [], quarantined: [quarantined])
        XCTAssertEqual(queue.unknown.count, 1)
        if case .quarantined(let inner) = queue.unknown[0].kind {
            XCTAssertEqual(inner.jobID, "job-corrupt")
        } else {
            XCTFail("expected a quarantined item")
        }
    }

    // MARK: - Ranking (durable facts only)

    /// A waiting row parked at a cap outranks everything else underway and
    /// carries the decision badge, even when other rows are more recent.
    func testPausedAtCapRanksFirstUnderwayAndIsBadged() {
        let older = Date(timeIntervalSince1970: 1_770_000_000)
        let newer = Date(timeIntervalSince1970: 1_770_000_500)
        let rows = [
            FleetFixtures.row(id: "job-running-new", updatedAt: newer, phase: .running),
            FleetFixtures.row(
                id: "job-capped", updatedAt: older, phase: .waiting, stopReason: .spendCap,
                spend: FleetRow.Spend(totalCostUSD: 25.0, capUSD: 25.0, subscription: false, wallTimeSeconds: nil)),
        ]
        let queue = QueueDerivation.derive(rows: rows)
        XCTAssertEqual(queue.underway.map(\.id), ["job-capped", "job-running-new"])
        XCTAssertTrue(queue.underway[0].needsDecision)
        XCTAssertFalse(queue.underway[1].needsDecision)
    }

    func testOperatorInputRequiredAndWallCapAreDecisionShaped() {
        for reason in [StopReason.operatorInputRequired, .wallCap, .spendCap] {
            let row = FleetFixtures.row(id: "job-\(reason.rawValue)", phase: .waiting, stopReason: reason)
            XCTAssertTrue(QueueDerivation.isDecisionShaped(row), "\(reason) should be decision-shaped")
        }
    }

    /// A waiting row with a non-decision stop reason (loop will resume on its
    /// own) is not badged and does not outrank.
    func testTransientWaitingIsNotDecisionShaped() {
        let row = FleetFixtures.row(id: "job-transient", phase: .waiting, stopReason: .transientProvider)
        XCTAssertFalse(QueueDerivation.isDecisionShaped(row))
    }

    /// Decision-shaped is a waiting-phase fact: the same stop reason on a
    /// running row (a stale stop reason from the last attempt) does not badge.
    func testDecisionShapeRequiresWaitingPhase() {
        let row = FleetFixtures.row(id: "job-resumed", phase: .running, stopReason: .spendCap)
        XCTAssertFalse(QueueDerivation.isDecisionShaped(row))
    }

    /// verifying_meaning is past the deterministic checks, hence nearer the
    /// gate than verifying_checks: pipeline position ranks it first.
    func testApproachingRanksMeaningBeforeChecks() {
        let older = Date(timeIntervalSince1970: 1_770_000_000)
        let newer = Date(timeIntervalSince1970: 1_770_000_900)
        let rows = [
            FleetFixtures.row(id: "job-checks-new", updatedAt: newer, phase: .verifyingChecks),
            FleetFixtures.row(id: "job-meaning-old", updatedAt: older, phase: .verifyingMeaning),
        ]
        let queue = QueueDerivation.derive(rows: rows)
        XCTAssertEqual(queue.approaching.map(\.id), ["job-meaning-old", "job-checks-new"])
    }

    func testAtTheGateRanksByRecency() {
        let rows = [
            FleetFixtures.row(
                id: "job-older", updatedAt: Date(timeIntervalSince1970: 1_770_000_000),
                phase: .terminal, outcome: .verified, receipt: FleetFixtures.validReceipt()),
            FleetFixtures.row(
                id: "job-newer", updatedAt: Date(timeIntervalSince1970: 1_770_000_900),
                phase: .terminal, outcome: .verified, receipt: FleetFixtures.validReceipt()),
        ]
        let queue = QueueDerivation.derive(rows: rows)
        XCTAssertEqual(queue.atTheGate.map(\.id), ["job-newer", "job-older"])
    }

    // MARK: - Counts and summary

    func testDecisionCountCombinesGateAndDecisionWaiting() {
        let rows = [
            FleetFixtures.row(id: "job-gate", phase: .terminal, outcome: .verified, receipt: FleetFixtures.validReceipt()),
            FleetFixtures.row(id: "job-capped", phase: .waiting, stopReason: .spendCap),
            FleetFixtures.row(id: "job-running", phase: .running),
        ]
        let queue = QueueDerivation.derive(rows: rows)
        XCTAssertEqual(queue.decisionCount, 2)
        XCTAssertEqual(queue.liveCount, 1)
        XCTAssertEqual(queue.summaryLine, "1 at the gate \u{00B7} 2 underway")
    }

    /// Deliberate deviation from the C1 mock, documented in CONTRACTS.md:
    /// APPROACHING is its own count in the summary line, never folded into
    /// "underway", so the verifying stage is visible at a glance.
    func testSummaryLineReportsApproachingSeparatelyFromUnderway() {
        let rows = [
            FleetFixtures.row(id: "job-checks", phase: .verifyingChecks),
            FleetFixtures.row(id: "job-running", phase: .running),
            FleetFixtures.row(id: "job-queued", phase: .queued),
        ]
        let queue = QueueDerivation.derive(rows: rows)
        XCTAssertEqual(queue.summaryLine, "1 approaching \u{00B7} 2 underway")
    }

    // MARK: - Stale lease facts (display honesty)

    /// The stale dot must say WHY, from the recorded heartbeat age only.
    func testStaleLeaseReasonNamesTheHeartbeatAge() {
        let lease = FleetFixtures.staleLease(age: 71)
        XCTAssertFalse(lease.fresh)
        XCTAssertEqual(GlossaryText.leaseStaleReason(lease), "no heartbeat 71s")
    }

    // MARK: - Glossary words

    func testGlossaryTranslatesEnumWordsNotRawNames() {
        XCTAssertEqual(GlossaryText.phaseWord(.verifyingChecks), "verifying checks")
        XCTAssertEqual(GlossaryText.phaseWord(.unknown), "unknown state")
        XCTAssertEqual(GlossaryText.outcomeWord(.needsReview), "needs review")
        XCTAssertEqual(GlossaryText.stopReasonWord(.spendCap), "paused at spend cap")
        XCTAssertEqual(GlossaryText.statusWord("verified_proof_invalid"), "proof invalid")
        XCTAssertEqual(GlossaryText.statusWord("budget_exhausted"), "budget exhausted")
        XCTAssertEqual(GlossaryText.verdictVerified, "VERIFIED")
        XCTAssertEqual(GlossaryText.proofWord(.valid), "VERIFIED")
        XCTAssertEqual(GlossaryText.proofWord(.invalid), "proof invalid")
    }

    func testSpendLineIsDurableFactsOnly() {
        let capped = FleetRow.Spend(totalCostUSD: 9.12, capUSD: 25, subscription: false, wallTimeSeconds: nil)
        XCTAssertEqual(GlossaryText.spendLine(capped), "$9.12 / $25.00")
        let uncapped = FleetRow.Spend(totalCostUSD: 3.876, capUSD: nil, subscription: true, wallTimeSeconds: nil)
        XCTAssertEqual(GlossaryText.spendLine(uncapped), "$3.88")
    }

    func testGateCountsRenderOnlyFromSignedMarkerCounts() {
        XCTAssertEqual(GlossaryText.gateCounts(FleetRow.Gate(attempt: 3, nPassed: 2, nTotal: 5)), "2/5 checks")
    }

    // MARK: - The judge chip is dropped by design

    /// FleetRow (the M0 rollup) carries no semantic-judgment field, so the
    /// C1 judge chip is dropped rather than re-derived from raw files. A row
    /// that grows a "judge" key decodes cleanly and exposes no judgment.
    func testRollupCarriesNoJudgmentFieldSoJudgeChipIsDropped() throws {
        let json = """
            {
              "job_id": "job-judge", "scope": "s", "goal": "g", "status": "verified",
              "updated_at": "2026-08-06T02:19:42Z", "attempts": 1,
              "outcome": "verified", "stop_reason": "verified",
              "projection": {"phase": "terminal", "outcome": "verified",
                             "stop_reason": "verified", "attempt_count": 1, "caveats": []},
              "lease": null, "spend": null, "provider": null, "sandbox": null,
              "receipt": {"present": true, "verified": "valid", "error": null},
              "gate": null,
              "judge": {"verdict": "uncertain"}
            }
            """
        let row = try DeadreckonJSON.decoder().decode(FleetRow.self, from: Data(json.utf8))
        XCTAssertEqual(row.jobID, "job-judge")
        let mirror = Mirror(reflecting: row)
        XCTAssertFalse(
            mirror.children.contains { $0.label == "judge" },
            "if the rollup ever grows a judgment field, surface the chip and update this test")
    }

    // MARK: - Lenient envelope decoding

    func testOneCorruptRowQuarantinesItselfOthersSurvive() throws {
        let envelope = """
            {
              "kind": "list", "id": "all", "status": "ok",
              "next_actions": [], "try_lines": [],
              "jobs": [
                {"job_id": "job-good", "scope": "s", "goal": "Good row", "status": "running",
                 "updated_at": "2026-08-06T02:19:42Z", "attempts": 1,
                 "outcome": null, "stop_reason": null,
                 "projection": {"phase": "running", "outcome": null, "stop_reason": null,
                                "attempt_count": 1, "caveats": []},
                 "lease": null, "spend": null, "provider": "cli:codex",
                 "sandbox": null, "receipt": null, "gate": null},
                {"job_id": "job-bad", "goal": "Broken row", "projection": {"phase": 7}},
                {"job_id": "job-good-2", "scope": "s", "goal": "Second good row", "status": "queued",
                 "updated_at": "2026-08-06T02:20:00Z", "attempts": 0,
                 "outcome": null, "stop_reason": null,
                 "projection": {"phase": "queued", "outcome": null, "stop_reason": null,
                                "attempt_count": 0, "caveats": []},
                 "lease": null, "spend": null, "provider": null,
                 "sandbox": null, "receipt": null, "gate": null}
              ],
              "runs": []
            }
            """
        let result = try FleetDecoder.decodeList(Data(envelope.utf8))
        XCTAssertEqual(result.rows.map(\.jobID), ["job-good", "job-good-2"])
        XCTAssertEqual(result.quarantined.count, 1)
        XCTAssertEqual(result.quarantined[0].jobID, "job-bad")
        XCTAssertEqual(result.quarantined[0].goal, "Broken row")
        XCTAssertFalse(result.quarantined[0].reason.isEmpty)
    }

    /// Two undecodable rows with no salvageable job_id and identical decode
    /// reasons still get distinct, launch-stable SwiftUI identities (their
    /// index in jobs[]), so ForEach never sees duplicate IDs.
    func testQuarantinedRowsWithSameReasonKeepDistinctStableIDs() throws {
        let envelope = """
            {"kind": "list", "id": "all", "status": "ok",
             "next_actions": [], "try_lines": [],
             "jobs": [
               {"goal": "first broken"},
               {"goal": "second broken"}
             ],
             "runs": []}
            """
        let result = try FleetDecoder.decodeList(Data(envelope.utf8))
        XCTAssertEqual(result.quarantined.count, 2)
        let queue = QueueDerivation.derive(rows: [], quarantined: result.quarantined)
        let ids = queue.unknown.map(\.id)
        XCTAssertEqual(ids, ["quarantined-0", "quarantined-1"])
    }

    /// Integer fields must survive the row-splitting path un-laundered
    /// (a 5 that came back as 5.0 would fail Int decoding).
    func testRowSplittingPreservesIntegers() throws {
        let envelope = """
            {
              "kind": "list", "id": "all", "status": "ok",
              "next_actions": [], "try_lines": [],
              "jobs": [
                {"job_id": "job-ints", "scope": "s", "goal": "g", "status": "verified",
                 "updated_at": "2026-08-06T02:19:42.913402Z", "attempts": 4,
                 "outcome": "verified", "stop_reason": "verified",
                 "projection": {"phase": "terminal", "outcome": "verified",
                                "stop_reason": "verified", "attempt_count": 4, "caveats": []},
                 "lease": {"owner_id": "sup", "epoch": 9, "heartbeat_age_seconds": 3,
                           "expires_at": "2026-08-06T02:20:12Z", "fresh": true},
                 "spend": {"total_cost_usd": 9.12, "cap_usd": 25.0,
                           "subscription": false, "wall_time_seconds": 100.5},
                 "provider": "cli:claude-code", "sandbox": null,
                 "receipt": {"present": true, "verified": "valid", "error": null},
                 "gate": {"attempt": 4, "n_passed": 5, "n_total": 5}}
              ],
              "runs": []
            }
            """
        let result = try FleetDecoder.decodeList(Data(envelope.utf8))
        XCTAssertEqual(result.rows.count, 1)
        XCTAssertEqual(result.rows[0].lease?.epoch, 9)
        XCTAssertEqual(result.rows[0].gate?.nPassed, 5)
        XCTAssertEqual(result.rows[0].spend?.totalCostUSD ?? 0, 9.12, accuracy: 0.0001)
    }

    func testLegacyRunsAreCountedNotQueued() throws {
        let envelope = """
            {
              "kind": "list", "id": "all", "status": "ok",
              "next_actions": [], "try_lines": [],
              "jobs": [],
              "runs": [
                {"run_id": "run-legacy", "scope": "s", "goal": "old run", "status": "completed",
                 "updated_at": "2026-08-06T01:00:00Z", "state_path": "/tmp/state.json",
                 "provider": null, "spend": null}
              ]
            }
            """
        let result = try FleetDecoder.decodeList(Data(envelope.utf8))
        XCTAssertTrue(result.rows.isEmpty)
        XCTAssertEqual(result.runs.map(\.runID), ["run-legacy"])
    }

    func testWrongKindEnvelopeThrows() {
        let data = Data(#"{"kind": "error", "jobs": []}"#.utf8)
        XCTAssertThrowsError(try FleetDecoder.decodeList(data)) { error in
            XCTAssertEqual(error as? FleetDecodeError, .wrongKind("error"))
        }
    }

    func testNonObjectEnvelopeThrows() {
        XCTAssertThrowsError(try FleetDecoder.decodeList(Data("[1,2,3]".utf8))) { error in
            XCTAssertEqual(error as? FleetDecodeError, .notAnObject)
        }
    }
}
