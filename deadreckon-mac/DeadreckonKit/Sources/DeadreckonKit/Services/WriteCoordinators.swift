import Foundation

// APP-4 write-surface coordinators: the testable engines the sheets bind to.
// Each one drives exactly one MutationRunner verb family, folds the machine
// result into a typed state, and never renders optimism — state changes
// reach the fleet through file-backed observation (FleetStore/FSEvents),
// never from these objects.

// MARK: - Lay Course (the G2 launch protocol)

/// The supported GUI launch protocol, exactly: `start --json` (read-only
/// preview, will_start:false) -> write the envelope's embedded launch_plan
/// payload to an app-owned scratch file -> `start --plan <file> --yes
/// --json` (execute, plus `--i-know-its-a-lot` ONLY when the typed-amount
/// acknowledgment armed it). Refusal envelopes render verbatim in the
/// sheet; the new job appears via the FleetStore FSEvents path, never
/// optimistically.
@MainActor
public final class LayCourseController: ObservableObject {
    public enum PreviewState: Equatable {
        case idle
        case loading
        /// Launchable: the preview embeds the replayable plan.
        case ready(StartPreviewEnvelope)
        /// Decoded but blocked (no launch_plan): the preview's own words and
        /// try lines say why.
        case blocked(StartPreviewEnvelope)
        case refused(ErrorEnvelope)
        case failed(String)
    }

    public enum ExecuteState: Equatable {
        case idle
        case running
        /// The launch envelope. The fleet row itself arrives from the files.
        case launched(MutationEnvelope)
        case refused(ErrorEnvelope)
        case failed(String)
    }

    @Published public var request = StartRequest(goal: "")
    @Published public private(set) var preview: PreviewState = .idle
    @Published public private(set) var execution: ExecuteState = .idle
    @Published public var acknowledgement = SpendAcknowledgement(capUSD: nil)

    /// Test observability: where the execute leg wrote the plan payload.
    public private(set) var lastPlanFileURL: URL?

    public let runner: MutationRunner

    public init(cli: FleetCLIRunning) {
        self.runner = MutationRunner(cli: cli)
    }

    public var previewCommandLine: String {
        runner.literalCommandLine(for: .startPreview(request))
    }

    /// The execute line as it will run (plan path shown as a placeholder
    /// until the file exists).
    public var executeCommandLine: String {
        let path = lastPlanFileURL?.path ?? "<launch-plan.json>"
        return runner.literalCommandLine(
            for: .startExecute(planFilePath: path, spendAcknowledged: acknowledgement.armsFlag))
    }

    public func runPreview() async {
        // In-flight guard: a double-click enqueues two MainActor tasks
        // before the first suspends; the second must not dispatch again.
        if case .loading = preview { return }
        preview = .loading
        execution = .idle
        // The displayed execute line must never name a plan file from a
        // PREVIOUS preview; the placeholder returns until execute writes one.
        lastPlanFileURL = nil
        let result = await runner.run(.startPreview(request))
        if let refusal = result.refusal {
            preview = .refused(refusal)
            return
        }
        guard let data = result.rawObjects.first,
              let envelope = StartPreviewEnvelope(data: data) else {
            preview = .failed(result.isEnvelopeFree
                ? result.envelopeFreeWords
                : "start --json did not return a decodable preview envelope")
            return
        }
        // The plan is the decision: the acknowledgment keys off the resolved
        // plan ceiling, falling back to the form's cap for blocked previews.
        acknowledgement = SpendAcknowledgement(
            capUSD: envelope.planCeilingUSD ?? request.maxSpendUSD)
        preview = envelope.isLaunchable ? .ready(envelope) : .blocked(envelope)
    }

    /// The execute leg. Guarded so the flag cannot be armed by anything but
    /// the typed amount: when the acknowledgment is required and unmatched,
    /// this returns without launching anything. Also guarded against a
    /// double-click: a second call while the first is in flight is a no-op.
    public func execute() async {
        if case .running = execution { return }
        guard case .ready(let envelope) = preview,
              let planData = envelope.launchPlanData else { return }
        guard acknowledgement.readyToStart else {
            execution = .failed(
                "a launch budget above $\(Int(SpendAcknowledgement.ceilingUSD)) requires typing the exact amount")
            return
        }
        execution = .running
        let planFile: URL
        do {
            planFile = try MutationRunner.writeLaunchPlanFile(planData)
        } catch {
            execution = .failed("could not write the launch plan file: \(error.localizedDescription)")
            return
        }
        lastPlanFileURL = planFile
        let result = await runner.run(.startExecute(
            planFilePath: planFile.path, spendAcknowledged: acknowledgement.armsFlag,
            fromPath: request.projectDirectory))
        if let refusal = result.refusal {
            execution = .refused(refusal)
        } else if let primary = result.primary {
            execution = .launched(primary)
        } else {
            // The binary died mid-verb with no envelope: it may have queued
            // the durable Job BEFORE dying, and this side cannot tell. Say
            // so, point at the file-backed fleet, and drop the ready
            // preview so the same one-click dispatch is not left armed — a
            // fresh preview is required before another Start.
            execution = .failed(
                result.envelopeFreeWords
                + "\nno machine envelope landed (exit \(result.exitCode)); the Job may or may "
                + "not have been queued before the failure. The fleet updates from the files "
                + "and will show it if it was. Run the preview again before another Start.")
            preview = .idle
        }
    }
}

/// The Lay Course pickers' catalog: `providers list --json` (failed probes
/// visible-but-disabled with their try lines as fix hints) and
/// `models --json` per provider. Read-only surfaces; each failure degrades
/// into its own words.
@MainActor
public final class LayCourseCatalog: ObservableObject {
    public enum Load<T: Equatable>: Equatable {
        case idle
        case loading
        case loaded(T)
        case failed(String)
    }

    @Published public private(set) var providers: Load<ProvidersEnvelope> = .idle
    @Published public private(set) var models: Load<ModelsEnvelope> = .idle

    private let cli: FleetCLIRunning

    public init(cli: FleetCLIRunning) {
        self.cli = cli
    }

    public func load() async {
        providers = .loading
        models = .loading
        do {
            let result = try await cli.run(arguments: ["providers", "list", "--json"], timeout: 60)
            if result.exitCode == 0,
               let envelope = try? DeadreckonJSON.decoder().decode(
                   ProvidersEnvelope.self, from: Data(result.stdout.utf8)) {
                providers = .loaded(envelope)
            } else {
                let words = result.stderr.isEmpty ? result.stdout : result.stderr
                providers = .failed(String(words.prefix(300)))
            }
        } catch {
            providers = .failed((error as? FleetCLIError)?.errorDescription
                ?? error.localizedDescription)
        }
        do {
            let result = try await cli.run(arguments: ["models", "--json"], timeout: 60)
            if result.exitCode == 0,
               let envelope = try? DeadreckonJSON.decoder().decode(
                   ModelsEnvelope.self, from: Data(result.stdout.utf8)) {
                models = .loaded(envelope)
            } else {
                let words = result.stderr.isEmpty ? result.stdout : result.stderr
                models = .failed(String(words.prefix(300)))
            }
        } catch {
            models = .failed((error as? FleetCLIError)?.errorDescription
                ?? error.localizedDescription)
        }
    }

    public func modelChoices(for provider: String?) -> [ModelsEnvelope.Entry] {
        guard case .loaded(let envelope) = models, let provider else { return [] }
        return envelope.providers.first(where: { $0.provider == provider })?.models ?? []
    }
}

// MARK: - Kill (honest confirmation + file-backed resolution)

/// Dispatches `kill <id> --json` and resolves the sheet's state machine.
/// Amber enters only from the envelope; terminal enters only from a
/// job-events.jsonl terminal event OBSERVED AFTER THIS DISPATCH, which this
/// coordinator tails while the sheet is open (one transient sheet-scoped
/// tail, documented in CONTRACTS.md next to the bounded-tails contract).
/// The tail is primed to end-of-file at dispatch time, so a historical
/// terminal classification already in the ledger (a prior attempt's
/// budget_exhausted on a paused job, needs_review, an earlier failed) can
/// never resolve THIS kill — design 2.4.5: the sheet resolves only when
/// that terminal event lands. The exit code is never an input to
/// resolution, and a corrupt ledger surfaces as resolution-unavailable
/// instead of waiting forever.
@MainActor
public final class KillCoordinator: ObservableObject {
    @Published public private(set) var progress = KillProgress()
    @Published public var escalate = false

    public let jobID: String
    private let runner: MutationRunner
    private let home: URL
    private var tailer: JSONLTailer?
    private var pollTask: Task<Void, Never>?

    public init(jobID: String, cli: FleetCLIRunning, home: URL = DeadreckonHome.url()) {
        self.jobID = jobID
        self.runner = MutationRunner(cli: cli)
        self.home = home
    }

    public var commandLine: String {
        runner.literalCommandLine(for: .kill(id: jobID, escalate: escalate))
    }

    /// Dispatch once: a double-click's second task finds the phase already
    /// past idle and dispatches nothing (kill is idempotent binary-side,
    /// but the sheet still speaks one story).
    public func dispatch() async {
        guard case .idle = progress.phase else { return }
        // Prime the tail past the ledger's existing history BEFORE the verb
        // runs: only rows appended after this point may resolve the sheet.
        primeTailerToEnd()
        progress.dispatched()
        let result = await runner.run(.kill(id: jobID, escalate: escalate))
        progress.fold(result: result)
        startPolling()
    }

    private func makeTailerIfNeeded() {
        guard tailer == nil else { return }
        let url = home.appendingPathComponent("jobs")
            .appendingPathComponent(jobID)
            .appendingPathComponent("job-events.jsonl")
        tailer = JSONLTailer(url: url, mode: .jobEvents)
    }

    /// Read and DISCARD everything already in job-events.jsonl. Historical
    /// rows are another story's terminal classifications; folding them
    /// would resolve this kill on stale facts. A pre-existing corruption
    /// verdict is sticky in the tailer and surfaces on the first pollOnce.
    private func primeTailerToEnd() {
        makeTailerIfNeeded()
        guard let tailer else { return }
        var drained = false
        while !drained {
            switch tailer.poll() {
            case .lines:
                continue
            case .none, .restarted, .corrupt:
                drained = true
            }
        }
    }

    /// One tail poll over jobs/<id>/job-events.jsonl: decode new rows, fold
    /// terminal kinds; a corrupt verdict folds into the honest
    /// resolution-unavailable phase. Public for deterministic tests.
    public func pollOnce() {
        makeTailerIfNeeded()
        guard let tailer else { return }
        switch tailer.poll() {
        case .lines(let lines):
            let decoder = DeadreckonJSON.decoder()
            for line in lines {
                guard let data = line.data(using: .utf8),
                      let event = try? decoder.decode(JobEvent.self, from: data) else { continue }
                progress.fold(jobEventKind: event.kind)
            }
        case .corrupt(let reason):
            progress.fold(tailCorruption:
                "job-events.jsonl reported corrupt: \(reason)")
        case .none, .restarted:
            break
        }
    }

    public func startPolling(interval: TimeInterval = 1) {
        guard pollTask == nil else { return }
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                self.pollOnce()
                if case .terminal = self.progress.phase { return }
                // Corruption is sticky in the tailer: further polls can
                // only repeat the verdict.
                if case .resolutionUnavailable = self.progress.phase { return }
                try? await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
            }
        }
    }

    /// Sheet dismissal: drop the transient tail and the loop.
    public func stop() {
        pollTask?.cancel()
        pollTask = nil
        tailer = nil
    }
}

// MARK: - Steer (workbench + popover quick-steer)

/// Submits `steer <id> "note" --json` and tracks queued -> delivered. The
/// delivered flip is fed by the caller (JobDetailStore's steerDeliveries
/// from the events tail); the popover, which holds no tail, honestly stays
/// at the queued chip with the envelope's own delivery wording.
@MainActor
public final class SteerCoordinator: ObservableObject {
    @Published public private(set) var tracker = SteerDeliveryTracker()

    public let targetID: String
    private let runner: MutationRunner

    public init(targetID: String, cli: FleetCLIRunning) {
        self.targetID = targetID
        self.runner = MutationRunner(cli: cli)
    }

    public func commandLine(note: String) -> String {
        runner.literalCommandLine(for: .steer(id: targetID, note: note))
    }

    public func submit(note: String) async {
        // A second Return-key/auto-repeat submit while one is in flight
        // must not queue a duplicate steer.
        if case .submitting = tracker.phase { return }
        let trimmed = note.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        tracker.submitted()
        let result = await runner.run(.steer(id: targetID, note: trimmed))
        tracker.fold(result: result)
    }

    /// Feed newly observed typed steer_delivered events from the events
    /// tail. Order-independent: the tracker buffers facts observed while
    /// the envelope is still in flight.
    public func observe(deliveries: [SteerDeliveredFact]) {
        tracker.fold(deliveries: deliveries)
    }

    public func reset() {
        tracker.reset()
    }
}

/// Popover quick-steer: lazily checks eligibility via `status <id> --json`
/// steerable{} when the row expands (disabled states name the envelope's
/// reason), then submits through the same SteerCoordinator machinery. A verb
/// refusal after `steerable: true` stays authoritative (trust rule 4): the
/// tracker's refused state downgrades the control.
@MainActor
public final class QuickSteerController: ObservableObject {
    public enum Eligibility: Equatable {
        case unknown
        case checking
        case eligible
        case ineligible(String)
        case unavailable(String)
    }

    @Published public private(set) var eligibility: Eligibility = .unknown
    @Published public private(set) var tracker = SteerDeliveryTracker()

    public let targetID: String
    private let cli: FleetCLIRunning
    private let runner: MutationRunner

    public init(targetID: String, cli: FleetCLIRunning) {
        self.targetID = targetID
        self.cli = cli
        self.runner = MutationRunner(cli: cli)
    }

    public func checkEligibility() async {
        eligibility = .checking
        do {
            let result = try await cli.run(arguments: ["status", targetID, "--json"], timeout: 30)
            guard result.exitCode == 0,
                  let envelope = try? DeadreckonJSON.decoder().decode(
                      JobStatusEnvelope.self, from: Data(result.stdout.utf8)) else {
                let words = result.stderr.isEmpty ? result.stdout : result.stderr
                eligibility = .unavailable(String(words.prefix(200)))
                return
            }
            guard let steerable = envelope.currentSteerable else {
                eligibility = .ineligible("no run attempt yet")
                return
            }
            if steerable.steerable {
                eligibility = .eligible
            } else {
                eligibility = .ineligible(Self.reasonWords(steerable.reason))
            }
        } catch {
            eligibility = .unavailable((error as? FleetCLIError)?.errorDescription
                ?? error.localizedDescription)
        }
    }

    public func submit(note: String) async {
        if case .submitting = tracker.phase { return }
        let trimmed = note.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        tracker.submitted()
        let result = await runner.run(.steer(id: targetID, note: trimmed))
        tracker.fold(result: result)
    }

    public static func reasonWords(_ reason: SteerIneligibleReason?) -> String {
        switch reason {
        case .driverFenced: return "driver fenced"
        case .notExecuting: return "not executing"
        case .providerNotSteerable: return "provider not steerable"
        case .unknown, nil: return GlossaryText.unknownState
        }
    }
}

// MARK: - Promote (Binnacle)

/// The Binnacle's engine: `finish <id> --dry-run --json` for the CANDIDATE
/// preview (decoding the G4 finish_plan shape exactly; degrading honestly
/// while the vendored binary predates R-M2), and `finish <id> ... --yes
/// --json` for PROMOTE. Fail-closed refusals surface verbatim; there is no
/// retry path with different authority because PlannedVerb cannot express
/// one, and this coordinator dispatches at most the one verb it was asked.
@MainActor
public final class PromoteCoordinator: ObservableObject {
    public enum PreviewState: Equatable {
        case idle
        case loading
        case plan(FinishPlanEnvelope)
        case refused(ErrorEnvelope)
        /// The committed binary rejected --dry-run at parse time (exit 2,
        /// prose, no envelope — the G1 carve-out): preview needs M2.
        case unsupported(String)
        case failed(String)
    }

    public enum PromotionState: Equatable {
        case idle
        case running
        case succeeded(MutationEnvelope)
        case refused(ErrorEnvelope)
        case failed(String)
    }

    @Published public var destination: FinishDestinationChoice = .apply(autostash: true, cleanup: false)
    @Published public private(set) var preview: PreviewState = .idle
    @Published public private(set) var promotion: PromotionState = .idle
    /// The destination the CURRENT preview was computed for, so the sheet
    /// can say honestly when the shown plan no longer matches the selected
    /// destination instead of pairing them silently.
    @Published public private(set) var previewDestination: FinishDestinationChoice?

    public let jobID: String
    private let runner: MutationRunner

    public init(jobID: String, cli: FleetCLIRunning) {
        self.jobID = jobID
        self.runner = MutationRunner(cli: cli)
    }

    public var dryRunCommandLine: String {
        runner.literalCommandLine(for: .finishDryRun(id: jobID, destination: destination))
    }

    public var finishCommandLine: String {
        runner.literalCommandLine(for: .finish(id: jobID, destination: destination))
    }

    public func loadPreview() async {
        if case .loading = preview { return }
        preview = .loading
        let requested = destination
        let result = await runner.run(.finishDryRun(id: jobID, destination: requested))
        previewDestination = requested
        if let refusal = result.refusal {
            preview = .refused(refusal)
            return
        }
        if result.isEnvelopeFree {
            // The G1 envelope contract begins after a successful PARSE, and
            // the only parse-time shape here is clap rejecting an unknown
            // --dry-run: exit 2 with usage prose. Every other envelope-free
            // result (watchdog SIGTERM, locator failure, crash, task
            // cancellation) is a plain failure and renders its own words —
            // never a fabricated version-gap diagnosis.
            if result.exitCode == 2 {
                preview = .unsupported(
                    "promote preview requires the M2 binary (finish --dry-run); "
                    + "the vendored binary said: \(result.envelopeFreeWords)")
            } else {
                preview = .failed(
                    "no machine envelope landed (exit \(result.exitCode)): "
                    + result.envelopeFreeWords)
            }
            return
        }
        let decoder = DeadreckonJSON.decoder()
        for object in result.rawObjects.reversed() {
            if let plan = try? decoder.decode(FinishPlanEnvelope.self, from: object),
               plan.kind == "finish_plan" {
                preview = .plan(plan)
                return
            }
        }
        preview = .failed("finish --dry-run returned no decodable finish_plan envelope")
    }

    /// PROMOTE. The gate is enforced here as well as in the view: a disabled
    /// gate never dispatches, and a second click while one finish is in
    /// flight dispatches nothing. On refusal the state renders the envelope
    /// verbatim and STOPS — the only recovery affordances are the envelope's
    /// own try lines and next actions.
    public func promote(gate: PromoteGate) async {
        if case .running = promotion { return }
        guard gate.promoteEnabled else {
            promotion = .failed(gate.disabledReason ?? "promote is not eligible")
            return
        }
        promotion = .running
        let result = await runner.run(.finish(id: jobID, destination: destination))
        if let refusal = result.refusal {
            promotion = .refused(refusal)
        } else if let primary = result.primary {
            promotion = .succeeded(primary)
        } else {
            promotion = .failed(result.envelopeFreeWords)
        }
    }
}

// MARK: - Send back (extend --note)

/// The Gate Review middle button: `extend <parent> "goal" --note "..."
/// --yes --json`. The note is recorded as typed provenance on the PARENT
/// run (G9); the envelope acknowledges with note_recorded and the app keeps
/// the note text app-side (the binary does not echo it back).
@MainActor
public final class SendBackCoordinator: ObservableObject {
    public enum State: Equatable {
        case idle
        case running
        case queued(MutationEnvelope)
        case refused(ErrorEnvelope)
        case failed(String)
    }

    @Published public var goal = ""
    @Published public var note = ""
    @Published public private(set) var state: State = .idle
    /// Set when an extend died mid-verb with no envelope: the continuation
    /// Job may already have queued (the crash could have landed after the
    /// queue write), so the one-click dispatch stays DISARMED until the
    /// operator explicitly re-arms after checking the fleet.
    @Published public private(set) var mayHaveQueued = false

    public let parentID: String
    private let runner: MutationRunner

    public init(parentID: String, cli: FleetCLIRunning) {
        self.parentID = parentID
        self.runner = MutationRunner(cli: cli)
    }

    public var commandLine: String {
        runner.literalCommandLine(for: plannedVerb)
    }

    public var canSubmit: Bool {
        !goal.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !mayHaveQueued
    }

    /// The operator's explicit fresh confirmation after an envelope-free
    /// failure: re-arms the dispatch (the sheet's button says to check the
    /// fleet first).
    public func rearmAfterPossibleQueue() {
        mayHaveQueued = false
        state = .idle
    }

    private var plannedVerb: PlannedVerb {
        let trimmedNote = note.trimmingCharacters(in: .whitespacesAndNewlines)
        return .extendJob(
            parentID: parentID,
            goal: goal.trimmingCharacters(in: .whitespacesAndNewlines),
            note: trimmedNote.isEmpty ? nil : trimmedNote)
    }

    public func submit() async {
        if case .running = state { return }
        guard canSubmit else { return }
        state = .running
        let result = await runner.run(plannedVerb)
        if let refusal = result.refusal {
            state = .refused(refusal)
        } else if let primary = result.primary {
            state = .queued(primary)
        } else {
            // Crashed mid-verb with no envelope: the continuation Job may
            // already have queued. Disarm the dispatch and say so.
            mayHaveQueued = true
            state = .failed(
                result.envelopeFreeWords
                + "\nno machine envelope landed (exit \(result.exitCode)); the continuation "
                + "Job may or may not have been queued before the failure. Check the fleet "
                + "(rows update from the files) before sending again.")
        }
    }
}
