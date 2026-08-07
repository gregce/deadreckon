import XCTest

@testable import DeadreckonKit

/// APP-5 notification derivation: stable identities, catch-up dedupe across
/// a simulated relaunch, per-reason filtering, action routing, the
/// bounded-tail selection, sticky per-run corruption, and the approval-class
/// recency waiver — all against fixture files and a fake notifier.
/// No UserNotifications framework, no real binary.
@MainActor
final class AttentionCenterTests: XCTestCase {
    private var home: URL!
    private var defaults: UserDefaults!
    private var suiteName: String!
    private let scope = "deadreckon"

    /// A fixed observation clock; records are stamped relative to it.
    private let now = DeadreckonJSON.date(from: "2026-02-01T13:00:00Z")!

    override func setUp() async throws {
        home = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("drk-attention-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        suiteName = "drk-attention-tests-\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suiteName)
    }

    override func tearDown() async throws {
        try? FileManager.default.removeItem(at: home)
        defaults.removePersistentDomain(forName: suiteName)
    }

    // MARK: - Fixture plumbing

    final class FakeNotifier: UserNotifying {
        var posted: [NotificationIntent] = []
        func post(_ intent: NotificationIntent) { posted.append(intent) }
    }

    private func runRoot(_ runID: String) -> URL {
        home.appendingPathComponent("runstate").appendingPathComponent(scope)
            .appendingPathComponent("runs").appendingPathComponent(runID)
    }

    private func writeNotify(runID: String, lines: [String]) throws {
        let root = runRoot(runID)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let payload = lines.map { $0 + "\n" }.joined()
        try Data(payload.utf8).write(to: root.appendingPathComponent("notify.jsonl"))
    }

    private func appendNotify(runID: String, line: String) throws {
        let url = runRoot(runID).appendingPathComponent("notify.jsonl")
        let handle = try FileHandle(forWritingTo: url)
        defer { try? handle.close() }
        try handle.seekToEnd()
        try handle.write(contentsOf: Data((line + "\n").utf8))
    }

    private func writeProjection(jobID: String, childRunIDs: [String]) throws {
        let dir = home.appendingPathComponent("jobs").appendingPathComponent(jobID)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let ids = childRunIDs.map { "\"\($0)\"" }.joined(separator: ",")
        let json = """
            {"job_id":"\(jobID)","phase":"running","last_sequence":3,
             "current_lease_epoch":1,"attempt_count":\(childRunIDs.count),
             "child_run_ids":[\(ids)],"caveats":[]}
            """
        try Data(json.utf8).write(to: dir.appendingPathComponent("projection.json"))
    }

    private func attentionLine(reason: String, jobID: String?, runID: String?,
                               at: String, summary: String = "fixture summary") -> String {
        let job = jobID.map { "\"job_id\":\"\($0)\"," } ?? ""
        let run = runID.map { "\"run_id\":\"\($0)\"," } ?? ""
        return """
            {"schema_version":1,"kind":"operator_attention","reason":"\(reason)",\(job)\(run)\
            "scope":"\(scope)","at":"\(at)","summary":"\(summary)",\
            "next_actions":["deadreckon status x"]}
            """
    }

    private func makeCenter(notifier: FakeNotifier,
                            rows: [FleetRow],
                            preferences: AttentionPreferences = AttentionPreferences(),
                            seenKey: String = "attention.seen") -> AttentionCenter {
        let center = AttentionCenter(
            home: home,
            notifier: notifier,
            seenStore: NotificationSeenStore(defaults: defaults, key: seenKey),
            preferences: { preferences },
            rowsProvider: { rows })
        center.nowProvider = { self.now }
        return center
    }

    /// A center whose rows snapshot can change mid-test (the fleet is live).
    private func makeMutableCenter(notifier: FakeNotifier,
                                   rows: @escaping () -> [FleetRow],
                                   seenKey: String = "attention.seen") -> AttentionCenter {
        let center = AttentionCenter(
            home: home,
            notifier: notifier,
            seenStore: NotificationSeenStore(defaults: defaults, key: seenKey),
            preferences: { AttentionPreferences() },
            rowsProvider: rows)
        center.nowProvider = { self.now }
        return center
    }

    // MARK: - Identity stability

    /// Same record bytes, derived twice through fresh decoders: identical
    /// record and delivery identities (identity is the record's own fields,
    /// never the time of observation).
    func testIntentIdentityStableAcrossDerivations() throws {
        let line = attentionLine(
            reason: "verified_awaiting_promote", jobID: "job-1", runID: "run-1",
            at: "2026-02-01T12:00:00.250Z")
        let first = try DeadreckonJSON.decoder()
            .decode(NotifyRecord.self, from: Data(line.utf8))
        let second = try DeadreckonJSON.decoder()
            .decode(NotifyRecord.self, from: Data(line.utf8))
        guard case .attention(let rowA) = first, case .attention(let rowB) = second else {
            return XCTFail("expected attention rows")
        }
        let intentA = AttentionDerivation.intent(from: rowA, owningJobID: nil)
        let intentB = AttentionDerivation.intent(from: rowB, owningJobID: nil)
        XCTAssertNotNil(intentA)
        XCTAssertEqual(intentA?.recordIdentity, intentB?.recordIdentity)
        XCTAssertEqual(intentA?.deliveryIdentity, intentB?.deliveryIdentity)
        // The at timestamp is IN the record identity (a genuinely new event
        // of the same reason+subject must fire again)…
        let laterLine = attentionLine(
            reason: "verified_awaiting_promote", jobID: "job-1", runID: "run-1",
            at: "2026-02-01T12:30:00Z")
        let later = try DeadreckonJSON.decoder()
            .decode(NotifyRecord.self, from: Data(laterLine.utf8))
        guard case .attention(let rowLater) = later else { return XCTFail() }
        let intentLater = AttentionDerivation.intent(from: rowLater, owningJobID: nil)
        XCTAssertNotEqual(intentA?.recordIdentity, intentLater?.recordIdentity)
        // …while the DELIVERY identity stays stable, so the reseal row after
        // a rolled-back sealing attempt replaces the banner instead of
        // stacking (TAILING.md dedupe guidance, exemplar stable-ID pattern).
        XCTAssertEqual(intentA?.deliveryIdentity, intentLater?.deliveryIdentity)
    }

    /// The summary renders verbatim; the title is the app-authored reason
    /// label; unknown reasons derive no intent at all.
    func testIntentContentAndUnknownReason() throws {
        let line = attentionLine(
            reason: "paused_at_cap", jobID: nil, runID: "run-9",
            at: "2026-02-01T12:00:00Z", summary: "run run-9 paused at cap")
        guard case .attention(let row) = try DeadreckonJSON.decoder()
            .decode(NotifyRecord.self, from: Data(line.utf8)) else { return XCTFail() }
        let intent = AttentionDerivation.intent(from: row, owningJobID: "job-9")
        XCTAssertEqual(intent?.body, "run run-9 paused at cap")
        XCTAssertEqual(intent?.title, "Paused at a limit")
        // paused_at_cap rows carry no job_id; the owning job of the tailed
        // run root fills in so Open can route.
        XCTAssertEqual(intent?.jobID, "job-9")
        XCTAssertEqual(intent?.categoryID, NotificationIdentity.generalCategoryID)

        let unknownLine = attentionLine(
            reason: "some_future_reason", jobID: "job-1", runID: nil,
            at: "2026-02-01T12:00:00Z")
        guard case .attention(let unknownRow) = try DeadreckonJSON.decoder()
            .decode(NotifyRecord.self, from: Data(unknownLine.utf8)) else { return XCTFail() }
        XCTAssertNil(AttentionDerivation.intent(from: unknownRow, owningJobID: nil))
    }

    // MARK: - Catch-up dedupe across relaunch

    /// The first center fires the unseen recent record; a second center
    /// built over the SAME UserDefaults (the relaunch simulation) sees the
    /// persisted seen-set and fires nothing.
    func testCatchUpDedupeSurvivesRelaunch() async throws {
        let jobID = "job-1"
        try writeProjection(jobID: jobID, childRunIDs: ["run-1"])
        try writeNotify(runID: "run-1", lines: [
            attentionLine(reason: "verified_awaiting_promote", jobID: jobID,
                          runID: "run-1", at: "2026-02-01T12:00:00Z")
        ])
        let rows = [FleetFixtures.row(id: jobID, phase: .terminal, outcome: .verified)]

        let firstLaunch = FakeNotifier()
        await makeCenter(notifier: firstLaunch, rows: rows).catchUpScan()
        XCTAssertEqual(firstLaunch.posted.count, 1)
        XCTAssertEqual(firstLaunch.posted.first?.reason, .verifiedAwaitingPromote)

        let secondLaunch = FakeNotifier()
        await makeCenter(notifier: secondLaunch, rows: rows).catchUpScan()
        XCTAssertTrue(secondLaunch.posted.isEmpty)
    }

    /// Unseen records older than the catch-up window are marked seen without
    /// firing — and stay silent on every later scan. (The approval-class
    /// waiver does not apply: a failed job is not a waiting decision.)
    func testStaleRecordsNeverFire() async throws {
        let jobID = "job-old"
        try writeProjection(jobID: jobID, childRunIDs: ["run-old"])
        try writeNotify(runID: "run-old", lines: [
            attentionLine(reason: "failed", jobID: jobID, runID: "run-old",
                          at: "2026-01-15T12:00:00Z")  // far outside 48 h
        ])
        let rows = [FleetFixtures.row(id: jobID, phase: .terminal, outcome: .failed)]
        let notifier = FakeNotifier()
        let center = makeCenter(notifier: notifier, rows: rows)
        await center.catchUpScan()
        await center.catchUpScan()
        XCTAssertTrue(notifier.posted.isEmpty)
    }

    /// The approval-class waiver: a verified_awaiting_promote record OLDER
    /// than the 48 h window still fires when the current rollup row shows
    /// the decision STILL waiting at the gate — the operator returning from
    /// a weekend-plus absence is exactly who this notification summons.
    func testApprovalCatchUpWaivesRecencyWindowWhileDecisionWaits() async throws {
        let jobID = "job-weekend"
        try writeProjection(jobID: jobID, childRunIDs: ["run-weekend"])
        try writeNotify(runID: "run-weekend", lines: [
            attentionLine(reason: "verified_awaiting_promote", jobID: jobID,
                          runID: "run-weekend", at: "2026-01-15T12:00:00Z")  // 17 days old
        ])
        let rows = [FleetFixtures.row(
            id: jobID, phase: .terminal, outcome: .verified,
            receipt: FleetFixtures.validReceipt())]
        let notifier = FakeNotifier()
        await makeCenter(notifier: notifier, rows: rows).catchUpScan()
        XCTAssertEqual(notifier.posted.map(\.reason), [.verifiedAwaitingPromote])
    }

    /// The waiver requires the decision to STILL be waiting: an old
    /// waiting_input record whose job has since moved on stays silent (and
    /// is marked seen, so it never replays).
    func testApprovalWaiverRequiresDecisionStillWaiting() async throws {
        let jobID = "job-moved-on"
        try writeProjection(jobID: jobID, childRunIDs: ["run-moved"])
        try writeNotify(runID: "run-moved", lines: [
            attentionLine(reason: "waiting_input", jobID: jobID,
                          runID: "run-moved", at: "2026-01-15T12:00:00Z")
        ])
        // The job is running again: the waiting decision was resolved.
        let rows = [FleetFixtures.row(id: jobID, phase: .running)]
        let notifier = FakeNotifier()
        let center = makeCenter(notifier: notifier, rows: rows)
        await center.catchUpScan()
        XCTAssertTrue(notifier.posted.isEmpty)
        // Marked seen: even a later snapshot showing waiting cannot replay it.
        let waitingRows = [FleetFixtures.row(id: jobID, phase: .waiting)]
        let second = FakeNotifier()
        await makeCenter(notifier: second, rows: waitingRows).catchUpScan()
        XCTAssertTrue(second.posted.isEmpty)
    }

    // MARK: - Per-reason filtering

    func testPerReasonFiltering() async throws {
        let jobID = "job-2"
        try writeProjection(jobID: jobID, childRunIDs: ["run-2"])
        try writeNotify(runID: "run-2", lines: [
            attentionLine(reason: "failed", jobID: jobID, runID: "run-2",
                          at: "2026-02-01T12:10:00Z"),
            attentionLine(reason: "verified_awaiting_promote", jobID: jobID,
                          runID: "run-2", at: "2026-02-01T12:20:00Z"),
        ])
        let rows = [FleetFixtures.row(id: jobID, phase: .terminal, outcome: .verified)]
        var preferences = AttentionPreferences()
        preferences.enabledReasons.remove(.failed)

        let notifier = FakeNotifier()
        await makeCenter(notifier: notifier, rows: rows, preferences: preferences).catchUpScan()
        XCTAssertEqual(notifier.posted.map(\.reason), [.verifiedAwaitingPromote])
    }

    func testMasterToggleSilencesEverything() async throws {
        let jobID = "job-3"
        try writeProjection(jobID: jobID, childRunIDs: ["run-3"])
        try writeNotify(runID: "run-3", lines: [
            attentionLine(reason: "blocked", jobID: jobID, runID: "run-3",
                          at: "2026-02-01T12:10:00Z")
        ])
        let rows = [FleetFixtures.row(id: jobID, phase: .terminal, outcome: .blocked)]
        let notifier = FakeNotifier()
        await makeCenter(notifier: notifier, rows: rows,
                         preferences: AttentionPreferences(masterEnabled: false)).catchUpScan()
        XCTAssertTrue(notifier.posted.isEmpty)
    }

    /// A reason filtered at observation time is marked seen: re-enabling the
    /// toggle later does not replay old news.
    func testFilteredRowsDoNotReplayWhenReenabled() async throws {
        let jobID = "job-4"
        try writeProjection(jobID: jobID, childRunIDs: ["run-4"])
        try writeNotify(runID: "run-4", lines: [
            attentionLine(reason: "cancelled", jobID: jobID, runID: "run-4",
                          at: "2026-02-01T12:10:00Z")
        ])
        let rows = [FleetFixtures.row(id: jobID, phase: .terminal, outcome: .cancelled)]
        var muted = AttentionPreferences()
        muted.enabledReasons.remove(.cancelled)
        let first = FakeNotifier()
        await makeCenter(notifier: first, rows: rows, preferences: muted).catchUpScan()
        XCTAssertTrue(first.posted.isEmpty)

        let second = FakeNotifier()
        await makeCenter(notifier: second, rows: rows).catchUpScan()
        XCTAssertTrue(second.posted.isEmpty)
    }

    // MARK: - Action routing derivation

    func testActionRoutingDerivation() {
        let info = [NotificationIdentity.jobIDUserInfoKey: "job-7",
                    NotificationIdentity.reasonUserInfoKey: "verified_awaiting_promote"]
        XCTAssertEqual(
            AttentionDerivation.route(
                actionIdentifier: NotificationIdentity.defaultActionID, userInfo: info),
            .openJob(jobID: "job-7"))
        XCTAssertEqual(
            AttentionDerivation.route(
                actionIdentifier: NotificationIdentity.openActionID, userInfo: info),
            .openJob(jobID: "job-7"))
        XCTAssertEqual(
            AttentionDerivation.route(
                actionIdentifier: NotificationIdentity.reviewAtGateActionID, userInfo: info),
            .reviewAtGate(jobID: "job-7"))
        // No job to route to: the honest no-op.
        XCTAssertNil(AttentionDerivation.route(
            actionIdentifier: NotificationIdentity.openActionID, userInfo: [:]))
        // A dismissal routes nowhere.
        XCTAssertNil(AttentionDerivation.route(
            actionIdentifier: "com.apple.UNNotificationDismissActionIdentifier", userInfo: info))
    }

    /// The verified category (with Review at Gate) is reserved for
    /// verified_awaiting_promote; the userInfo payload round-trips the route.
    func testVerifiedCategoryAndUserInfoRoundTrip() throws {
        let line = attentionLine(
            reason: "verified_awaiting_promote", jobID: "job-8", runID: "run-8",
            at: "2026-02-01T12:00:00Z")
        guard case .attention(let row) = try DeadreckonJSON.decoder()
            .decode(NotifyRecord.self, from: Data(line.utf8)) else { return XCTFail() }
        let intent = AttentionDerivation.intent(from: row, owningJobID: nil)!
        XCTAssertEqual(intent.categoryID, NotificationIdentity.verifiedCategoryID)
        let info = AttentionDerivation.userInfo(for: intent)
        XCTAssertEqual(
            AttentionDerivation.route(
                actionIdentifier: NotificationIdentity.reviewAtGateActionID, userInfo: info),
            .reviewAtGate(jobID: "job-8"))
    }

    // MARK: - Bounded tail selection

    func testBoundedTailSelection() {
        let reference = now
        var rows: [FleetRow] = []
        // 15 live rows, staggered updatedAt (newest first should win).
        for index in 0..<15 {
            rows.append(FleetFixtures.row(
                id: "live-\(index)",
                updatedAt: reference.addingTimeInterval(TimeInterval(-index * 10)),
                phase: .running))
        }
        // A recently-terminal row (inside the window) and an old wreck.
        rows.append(FleetFixtures.row(
            id: "fresh-terminal", updatedAt: reference.addingTimeInterval(-60),
            phase: .terminal, outcome: .verified))
        rows.append(FleetFixtures.row(
            id: "old-wreck", updatedAt: reference.addingTimeInterval(-7200),
            phase: .terminal, outcome: .failed))

        let plan = AttentionTailPlan.select(rows: rows, limit: 12, now: reference)
        XCTAssertEqual(plan.active.count, 12)
        let activeIDs = Set(plan.active.map(\.jobID))
        // The recently-terminal row is tail-worthy and fresh enough to rank.
        XCTAssertTrue(activeIDs.contains("fresh-terminal"))
        // The old terminal row is NEVER tailed…
        XCTAssertFalse(activeIDs.contains("old-wreck"))
        // …but stays covered by the fallback sweep.
        XCTAssertTrue(plan.fallback.map(\.jobID).contains("old-wreck"))
        // Active ranks newest-first; the slowest live rows spill to fallback.
        XCTAssertTrue(activeIDs.contains("live-0"))
        XCTAssertFalse(activeIDs.contains("live-14"))
        XCTAssertTrue(plan.fallback.map(\.jobID).contains("live-14"))
        // Nothing is dropped: every row is either active or fallback.
        XCTAssertEqual(plan.active.count + plan.fallback.count, rows.count)
    }

    /// The activeTailLimit cap holds THROUGH pollOnce's reconcile (not just
    /// the pure plan), and a fleet reorder evicts the oldest tail for the
    /// newly freshest row.
    func testActiveTailCapAndEvictionThroughPollOnce() async throws {
        var rows: [FleetRow] = []
        for index in 0..<15 {
            rows.append(FleetFixtures.row(
                id: "live-\(index)",
                updatedAt: now.addingTimeInterval(TimeInterval(-index * 10)),
                phase: .running))
        }
        var snapshot = rows
        let notifier = FakeNotifier()
        let center = makeMutableCenter(notifier: notifier, rows: { snapshot })

        await center.pollOnce()
        XCTAssertEqual(center.activeTailJobIDs.count, 12)
        XCTAssertTrue(center.activeTailJobIDs.contains("live-0"))
        XCTAssertTrue(center.activeTailJobIDs.contains("live-11"))
        XCTAssertFalse(center.activeTailJobIDs.contains("live-14"))

        // live-14 becomes the freshest row: it gains a tail, the cap holds,
        // and the now-slowest tailed row (live-11) is evicted to fallback.
        snapshot[14] = FleetFixtures.row(
            id: "live-14", updatedAt: now.addingTimeInterval(10), phase: .running)
        await center.pollOnce()
        XCTAssertEqual(center.activeTailJobIDs.count, 12)
        XCTAssertTrue(center.activeTailJobIDs.contains("live-14"))
        XCTAssertFalse(center.activeTailJobIDs.contains("live-11"))
    }

    // MARK: - Live tail behavior

    /// A row appended after the first tick posts exactly once; re-polling
    /// and even a fresh tail from offset 0 cannot re-fire it. The owning job
    /// is attached from the tailed run root.
    func testLiveTailPostsOnceWithOwningJob() async throws {
        let jobID = "job-live"
        try writeProjection(jobID: jobID, childRunIDs: ["run-live"])
        try writeNotify(runID: "run-live", lines: [])
        let rows = [FleetFixtures.row(id: jobID, phase: .running)]
        let notifier = FakeNotifier()
        let center = makeCenter(notifier: notifier, rows: rows)

        await center.pollOnce()
        XCTAssertTrue(notifier.posted.isEmpty)
        XCTAssertEqual(center.activeTailJobIDs, [jobID])

        try appendNotify(runID: "run-live", line: attentionLine(
            reason: "paused_at_cap", jobID: nil, runID: "run-live",
            at: "2026-02-01T12:55:00Z"))
        await center.pollOnce()
        XCTAssertEqual(notifier.posted.count, 1)
        XCTAssertEqual(notifier.posted.first?.jobID, jobID)

        await center.pollOnce()
        XCTAssertEqual(notifier.posted.count, 1)
    }

    /// A corrupt notify.jsonl (blessed standard tail) gets a STICKY verdict:
    /// issues[jobID] carries the tailer's words, the tail is dropped, later
    /// valid appends never post, and the verdict survives the row leaving
    /// the active tail set (sweeps skip the corrupt run too).
    func testCorruptTailPublishesIssueAndStaysSticky() async throws {
        let jobID = "job-corrupt"
        try writeProjection(jobID: jobID, childRunIDs: ["run-corrupt"])
        try writeNotify(runID: "run-corrupt", lines: ["this is not json"])
        var snapshot = [FleetFixtures.row(id: jobID, phase: .running)]
        let notifier = FakeNotifier()
        let center = makeMutableCenter(notifier: notifier, rows: { snapshot })

        // catch-up sweep is lenient (skips the bad line); the live tail then
        // hits the same line strictly and records the sticky verdict.
        await center.pollOnce()
        XCTAssertNotNil(center.issues[jobID])
        XCTAssertTrue(notifier.posted.isEmpty)
        XCTAssertFalse(center.activeTailJobIDs.contains(jobID))

        // A valid line appended AFTER the verdict must never post: the file
        // already lied once (TAILING.md sticky corruption).
        try appendNotify(runID: "run-corrupt", line: attentionLine(
            reason: "failed", jobID: jobID, runID: "run-corrupt",
            at: "2026-02-01T12:50:00Z"))
        await center.pollOnce()
        XCTAssertTrue(notifier.posted.isEmpty)
        XCTAssertNotNil(center.issues[jobID])

        // The row goes old-terminal (leaves the active set, fallback only):
        // sweeps must ALSO skip the corrupt run, across a full sweep cycle.
        snapshot = [FleetFixtures.row(
            id: jobID, updatedAt: now.addingTimeInterval(-7200),
            phase: .terminal, outcome: .failed)]
        for _ in 0..<24 { await center.pollOnce() }
        XCTAssertTrue(notifier.posted.isEmpty)
        XCTAssertNotNil(center.issues[jobID])
    }

    /// Only a NEW attempt (a different current runID) clears the sticky
    /// verdict: the fresh run's file earns fresh trust and posts normally.
    func testNewAttemptClearsCorruptVerdict() async throws {
        let jobID = "job-retry"
        try writeProjection(jobID: jobID, childRunIDs: ["run-r1"])
        try writeNotify(runID: "run-r1", lines: ["garbage line"])
        let rows = [FleetFixtures.row(id: jobID, phase: .running)]
        let notifier = FakeNotifier()
        let center = makeCenter(notifier: notifier, rows: rows)

        await center.pollOnce()
        XCTAssertNotNil(center.issues[jobID])

        // The supervisor launches attempt 2: projection points at run-r2.
        try writeProjection(jobID: jobID, childRunIDs: ["run-r1", "run-r2"])
        try writeNotify(runID: "run-r2", lines: [
            attentionLine(reason: "waiting_input", jobID: jobID, runID: "run-r2",
                          at: "2026-02-01T12:45:00Z")
        ])
        await center.pollOnce()
        XCTAssertNil(center.issues[jobID])
        XCTAssertEqual(notifier.posted.map(\.reason), [.waitingInput])
    }

    /// Delivery-attempt rows (no kind) never post; undecodable complete
    /// lines in a sweep are skipped, not fatal.
    func testDeliveryAttemptRowsNeverPost() async throws {
        let jobID = "job-quiet"
        try writeProjection(jobID: jobID, childRunIDs: ["run-quiet"])
        try writeNotify(runID: "run-quiet", lines: [
            """
            {"ts":"2026-02-01T12:00:00Z","transition":"paused","channel":"desktop",\
            "ok":false,"detail":"no notifier"}
            """
        ])
        let rows = [FleetFixtures.row(id: jobID, phase: .running)]
        let notifier = FakeNotifier()
        await makeCenter(notifier: notifier, rows: rows).catchUpScan()
        XCTAssertTrue(notifier.posted.isEmpty)
    }

    /// No projection.json at all: the job's own id stands in as the run id
    /// (the Single-shape fallback verdict-on-JOB uses).
    func testSingleShapeRunRootFallback() async throws {
        let jobID = "job-single"
        try writeNotify(runID: jobID, lines: [
            attentionLine(reason: "waiting_input", jobID: jobID, runID: jobID,
                          at: "2026-02-01T12:40:00Z")
        ])
        let rows = [FleetFixtures.row(id: jobID, phase: .waiting)]
        let notifier = FakeNotifier()
        await makeCenter(notifier: notifier, rows: rows).catchUpScan()
        XCTAssertEqual(notifier.posted.map(\.reason), [.waitingInput])
    }

    /// Launch order honesty: at app launch the fleet is still loading, so
    /// the start()-time scan sees no rows. The first tick with a non-empty
    /// fleet completes the catch-up IMMEDIATELY — a fallback-only row (old
    /// terminal, never tailed) does not wait out a sweep interval for its
    /// missed notification.
    func testDeferredCatchUpFiresOnFirstNonEmptyFleet() async throws {
        let jobID = "job-overnight"
        try writeProjection(jobID: jobID, childRunIDs: ["run-overnight"])
        try writeNotify(runID: "run-overnight", lines: [
            attentionLine(reason: "verified_awaiting_promote", jobID: jobID,
                          runID: "run-overnight", at: "2026-02-01T06:00:00Z")
        ])
        // Terminal 2 h ago: outside the recently-terminal tail window, so
        // only a sweep can reach its notify file.
        var rows: [FleetRow] = []
        let notifier = FakeNotifier()
        let center = AttentionCenter(
            home: home,
            notifier: notifier,
            seenStore: NotificationSeenStore(defaults: defaults, key: "deferred"),
            preferences: { AttentionPreferences() },
            rowsProvider: { rows })
        center.nowProvider = { self.now }

        await center.catchUpScan()  // fleet still loading: nothing to scan
        await center.pollOnce()
        XCTAssertTrue(notifier.posted.isEmpty)

        rows = [FleetFixtures.row(
            id: jobID, updatedAt: now.addingTimeInterval(-7200),
            phase: .terminal, outcome: .verified,
            receipt: FleetFixtures.validReceipt())]
        await center.pollOnce()
        XCTAssertEqual(notifier.posted.map(\.reason), [.verifiedAwaitingPromote])

        // And only once: later sweeps re-read the same file harmlessly.
        for _ in 0..<24 { await center.pollOnce() }
        XCTAssertEqual(notifier.posted.count, 1)
    }

    // MARK: - Seen store bounds

    func testSeenStoreLRUBoundsAndPersistence() {
        let store = NotificationSeenStore(defaults: defaults, key: "lru-test", capacity: 3)
        store.markSeen("a")
        store.markSeen("b")
        store.markSeen("c")
        store.markSeen("a")  // refresh a's recency
        store.markSeen("d")  // evicts b (oldest)
        XCTAssertTrue(store.contains("a"))
        XCTAssertFalse(store.contains("b"))
        XCTAssertTrue(store.contains("c"))
        XCTAssertTrue(store.contains("d"))
        XCTAssertEqual(store.count, 3)

        // Persistence: a second store over the same defaults sees the set.
        let reloaded = NotificationSeenStore(defaults: defaults, key: "lru-test", capacity: 3)
        XCTAssertTrue(reloaded.contains("d"))
        XCTAssertFalse(reloaded.contains("b"))
    }

    /// The center refreshes LRU recency on EVERY observation (not just first
    /// sight): identities still present in a swept file can never age out of
    /// the bounded store and re-fire, even under eviction pressure.
    func testSweepRefreshesSeenRecencyAgainstEviction() async throws {
        let jobID = "job-refresh"
        try writeProjection(jobID: jobID, childRunIDs: ["run-refresh"])
        try writeNotify(runID: "run-refresh", lines: [
            attentionLine(reason: "failed", jobID: jobID, runID: "run-refresh",
                          at: "2026-02-01T12:30:00Z")
        ])
        let rows = [FleetFixtures.row(
            id: jobID, updatedAt: now.addingTimeInterval(-7200),
            phase: .terminal, outcome: .failed)]
        let notifier = FakeNotifier()
        let seen = NotificationSeenStore(defaults: defaults, key: "refresh-test", capacity: 3)
        let center = AttentionCenter(
            home: home, notifier: notifier, seenStore: seen,
            preferences: { AttentionPreferences() }, rowsProvider: { rows })
        center.nowProvider = { self.now }

        await center.catchUpScan()
        XCTAssertEqual(notifier.posted.count, 1)

        // Churn the tiny store almost to eviction, then re-sweep: the sweep
        // refreshes the identity's recency, so further churn evicts OTHER
        // entries and the record never re-fires.
        seen.markSeen("noise-1")
        seen.markSeen("noise-2")
        await center.catchUpScan()  // refresh: our identity is newest again
        seen.markSeen("noise-3")
        seen.markSeen("noise-4")  // would have evicted the record pre-refresh
        await center.catchUpScan()
        XCTAssertEqual(notifier.posted.count, 1)
    }

    func testPreferencesRoundTrip() {
        var preferences = AttentionPreferences()
        preferences.masterEnabled = true
        preferences.enabledReasons.remove(.pausedAtCap)
        preferences.save(to: defaults)
        let loaded = AttentionPreferences.load(from: defaults)
        XCTAssertEqual(loaded, preferences)
        XCTAssertFalse(loaded.allows(.pausedAtCap))
        XCTAssertTrue(loaded.allows(.failed))
        XCTAssertFalse(loaded.allows(.unknown))
    }
}
