# Module contracts

Public API shapes each module must expose, so independently built modules integrate without rework. Implementers may add members but must not rename or reshape what is written here. Language mode Swift 5.9, `SWIFT_STRICT_CONCURRENCY=minimal`, macOS 14, no external dependencies, XCTest for tests. Style mirrors specstory-mac: `final class` services, async throwing methods, `AsyncStream` for child output, no Combine. No em dashes in user-visible strings.

Ground truth for every shape below is the committed M0 Rust surface: `crates/deadreckon/src/commands/inspection.rs` (`job_list_row`/`run_list_row`), `docs/schemas/` (job-lease, job-event, spend-record, projections/run-view), and `docs/TAILING.md`. When the binary and this file disagree, the binary wins and this file gets fixed in the same change.

## Trust rules (every module, non-negotiable)

These restate design doc sections 2.4.4 to 2.4.6 and the section 8 roadmap trust rule as implementation law:

1. **The app never invokes `dr-gate`, never signs, never writes a marker.** There is no code path that launches the gate toolchain, and none may be added.
2. **No override affordance exists anywhere.** A failed digest, a failed gate, or a fail-closed refusal renders exactly what the binary said (message + try_lines verbatim). There is no "force", "skip", or "promote anyway" control, including in debug builds.
3. **`gate-keys/` is never read**, listed, stat'ed, or watched. No file API touches any path containing `gate-keys`.
4. **Verb refusals are authoritative.** If a surface said an action was possible (`steerable: true`) and the verb then refused, the refusal wins: downgrade the control, render the typed reason, do not retry. This is load-bearing for the G6 legacy caveat (pre-ownership-stamp runs can read `steerable: true` yet still receive the plan-lineage fence refusal from the verb).
5. **Fleet rows derive from `list --json` projection fields plus glossary words only.** No invented status language, no forward-looking estimates, no readiness scores (operator decision 8: durable facts only). `verified_proof_invalid` is its own rendered state, never collapsed into Verified.
6. **"VERIFIED" renders only from the shared proof classifier** (`receipt.verified == "valid"`, or a fresh `verdict --json` at render time). Gate counts render only when `gate{}` is present, which means a signature-verified acceptance marker exists; failed attempts have no counts, rank them on phase/stop_reason.
7. **Tailed rows never confer authority** (TAILING.md): acceptance-progress rows are display only; spend rows are provider evidence, not a billing source of truth; notify rows are delivery observability, not a delivery log.
8. **Single-writer discipline:** the app never creates, modifies, or deletes anything under `DEADRECKON_HOME`. Every mutation shells out to the manifest-pinned binary, and mutation sheets display the literal CLI line they will run.
9. **Kill/steer honesty:** kill confirmations state the real mechanics (CancelRequested, SIGTERM to the group, grace, SIGKILL, supervisor-proven Cancelled) and resolve only on the file-backed terminal event.

## DeadreckonKit/Sources/DeadreckonKit

### Decoding substrate (Models/DeadreckonJSON.swift)

```swift
public enum DeadreckonJSON {
    /// RFC 3339 with-or-without fractional seconds (chrono emits both).
    public static func date(from raw: String) -> Date?
    /// The ONLY decoder used for deadreckon JSON. `.iso8601` rejects
    /// fractional seconds and must never be used for these surfaces.
    public static func decoder() -> JSONDecoder
}

/// Arbitrary JSON for open schema fields (JobEvent.detail is `"detail": true`).
public enum JSONValue: Codable, Equatable, Sendable {
    case null, bool(Bool), number(Double), string(String)
    case array([JSONValue]), object([String: JSONValue])
}
```

### Glossary enums (Models/GlossaryEnums.swift)

```swift
/// Unrecognized string -> .unknown, never a decode failure. A non-string
/// value still throws (schema violation, not vocabulary growth).
public protocol ForgivingStringEnum: RawRepresentable, Codable, Equatable, Sendable
where RawValue == String {
    static var unknown: Self { get }
}

public enum JobPhase: String, ForgivingStringEnum, CaseIterable
// queued, running, verifying_checks, verifying_meaning, waiting, terminal, unknown

public enum JobOutcome: String, ForgivingStringEnum, CaseIterable
// verified, needs_review, blocked, budget_exhausted, deadline_reached,
// retry_exhausted, cancelled, failed, unknown

public enum StopReason: String, ForgivingStringEnum, CaseIterable
// the 18 protocol reasons (verified ... legacy_unknown) plus unknown

/// The shared three-valued proof classifier (job_proof_status). NOT a Bool.
public enum ProofStatus: String, ForgivingStringEnum, CaseIterable
// valid, invalid, "not-applicable" (note the hyphen), unknown

public enum SteerIneligibleReason: String, ForgivingStringEnum, CaseIterable
// driver_fenced, not_executing, provider_not_steerable, unknown
// (provider_not_steerable is historical: since the M1 steer widening the
// binary never emits it — any provider is steerable while Executing)

public enum JobEventKind: String, ForgivingStringEnum, CaseIterable
// the 31 job-event.schema.json kinds plus unknown

public enum NotifyTransition: String, ForgivingStringEnum, CaseIterable
// accepted, paused, failed, unknown
```

Invariants:
- Rendering `.unknown` shows the words "unknown state" (or the raw string in evidence panes), never a guessed state.
- New words the binary grows land here as cases in the same change that bumps the vendored manifest.

### Glossary words (Models/GlossaryText.swift)

Mirrors `crates/deadreckon-core/src/glossary.rs`: the ONLY source of user-facing status words. Raw enum names never reach the UI.

```swift
public enum GlossaryText {
    public static let nounRun, nounChain, nounPlan, nounChild, nounVerifiedRun: String
    public static let phraseVerifiedByDrGate, nounDoneContract: String
    public static let verdictVerified: String       // "VERIFIED", trust rule 6
    public static let unknownState: String          // "unknown state"
    public static func phaseWord(_ phase: JobPhase) -> String
    public static func outcomeWord(_ outcome: JobOutcome) -> String
    public static func stopReasonWord(_ reason: StopReason) -> String
    public static func proofWord(_ proof: ProofStatus) -> String
    public static func statusWord(_ raw: String) -> String  // job_status_label words
    public static func spendLine(_ spend: FleetRow.Spend) -> String     // "$9.12 / $25.00"
    public static func leaseStaleReason(_ lease: FleetRow.Lease) -> String  // "no heartbeat 71s"
    public static func gateCounts(_ gate: FleetRow.Gate) -> String      // "5/5 checks"
}
```

Invariants:
- `statusWord("verified_proof_invalid")` renders "proof invalid", its own state, never any variant of plain Verified (trust rule 5).
- `proofWord(.valid)` is the only expression that returns VERIFIED.
- Provenance is two-tier and stated honestly: the named constants mirror glossary.rs verbatim, but the phase/outcome/stop-reason words are APP-AUTHORED translations — glossary.rs has no vocabulary for JobPhase/JobOutcome/StopReason yet (the CLI renders raw snake_case). One word-per-label, nothing predictive, no readiness language (decision 8). **Rust-side gap, needs registering:** `job_phase_label`/`job_outcome_label`/`stop_reason_label` in glossary.rs (same pattern as `run_status_label`), ridden by job.rs and mirrored here, so the two vocabularies cannot drift.

### Harbor read-models (Models/HarborModels.swift)

```swift
public enum ProviderProbeStatus: String, ForgivingStringEnum   // ok, failed, skipped, unknown
public struct ProviderProbeRow: Codable, Equatable, Sendable {  // providers[] subset
    public let id: String
    public let displayName: String?    // display_name
    public let status: ProviderProbeStatus
}
public struct ProvidersEnvelope: Codable, Equatable, Sendable { // providers list --json
    public let kind: String            // "providers"
    public let providers: [ProviderProbeRow]
    public let missingProviders: [String]
    public let active: String?
    public var okCount: Int            // status == ok
    public var totalCount: Int         // providers.count + missing
}
public struct DoctorEnvelope: Codable, Equatable, Sendable {    // doctor --json subset
    public struct Finding: Codable, Equatable, Sendable { status, subject, detail: String }
    public let kind: String            // "doctor"
    public let status: String          // verdict word, rendered not interpreted
    public let findings: [Finding]
    public var failedCount, warningCount: Int
}
public enum SupervisorInstallState: String, ForgivingStringEnum
// unsupported, not_installed, stale, current, unknown
public struct SupervisorStatusReport: Codable, Equatable, Sendable { // supervisor status --json subset
    public let schemaVersion: Int
    public let manager: String
    public let installed: SupervisorInstallState
    public let loaded: Bool?
    public let active: String?
    public var isRunning: Bool  // mirrors Rust ServiceManagerRuntime::is_running exactly
}
```

Invariant: these are quiet-chip facts. `isRunning` is display data; the `checkpoint` live-evidence block is deliberately not modeled until a surface needs it (validate_supervisor_service_live_evidence stays a Rust-side authority).

### Queue derivation (Models/QueueDerivation.swift)

Pure functions from rollup rows to the Quarterdeck taxonomy. Unit-tested in QueueDerivationTests/MenuBarStateTests.

```swift
public struct QuarantinedRow: Equatable, Hashable, Sendable {
    public let jobID: String?          // salvaged from the raw object when present
    public let goal: String?
    public let reason: String          // decode failure, the error's words
    public let ordinal: Int            // index in jobs[]: stable identity when jobID is nil
}

public enum QueueSection: String, CaseIterable, Sendable {
    case atTheGate, needsReview, approaching, underway, wrecked, unknown
    public var title: String           // "AT THE GATE", "NEEDS REVIEW", ...
    public var subtitle: String?
}

public struct QueueItem: Equatable, Hashable, Sendable, Identifiable {
    public enum Kind: Equatable, Sendable { case job(FleetRow), quarantined(QuarantinedRow) }
    public let kind: Kind
    public let section: QueueSection
    public let needsDecision: Bool     // waiting + decision-shaped stop reason, or needsReview
    public var id: String              // jobID, else "quarantined-<ordinal>"
    public var row: FleetRow?
}

public struct GateQueue: Equatable, Sendable {
    public let atTheGate, needsReview, approaching, underway, wrecked, unknown: [QueueItem]
    public static let empty: GateQueue
    public func items(in section: QueueSection) -> [QueueItem]
    public var allItems: [QueueItem]           // taxonomy order
    public var nonEmptySections: [QueueSection]
    public var isEmpty: Bool
    public var jobCount: Int
    public var decisionCount: Int      // gate rows + needsReview rows + decision-shaped waiting rows
    public var liveCount: Int          // running | verifying_* rows
    public var summaryLine: String     // "2 at the gate · 1 approaching · 3 underway" (counts, not prose)
    // summaryLine deviates from the C1 mock on purpose: APPROACHING is its own
    // count, never folded into "underway", so the verifying stage survives the
    // ten-second glance. Documented deviation, tested.
}

public enum QueueDerivation {
    public static let decisionShapedStopReasons: Set<StopReason>
    // operator_input_required, spend_cap, wall_cap
    public static func derive(rows: [FleetRow], quarantined: [QuarantinedRow]) -> GateQueue
    public static func classify(_ row: FleetRow) -> QueueSection
    public static func isDecisionShaped(_ row: FleetRow) -> Bool
    /// staleLeaseCount MUST be the debounced count (FleetStore's confirmed set,
    /// never one raw poll); supervisorDown only for a positively-reported
    /// stopped Watchkeeper (an unknown chip never badges).
    public static func menuBarState(_ fleet: FleetStore.FleetState,
                                    staleLeaseCount: Int = 0,
                                    supervisorDown: Bool = false) -> MenuBarFleetState
}

public enum MenuBarFleetState: Equatable, Sendable {
    case unavailable          // binary missing / scan failed: error glyph
    case attention(Int)       // decisionCount > 0: badge wins over everything below
    case degraded(staleLeases: Int, supervisorDown: Bool)
                              // design 2.4.1 badge: confirmed-stale lease or
                              // Watchkeeper stopped; outranks live, below attention
    case live(Int)            // liveCount > 0
    case idle
    case loading              // nothing fetched yet
}

public struct FleetDecodeResult: Equatable, Sendable {
    public let rows: [FleetRow]
    public let quarantined: [QuarantinedRow]
    public let runs: [RunRow]
}
public enum FleetDecodeError: Error, LocalizedError, Equatable { case notAnObject, wrongKind(String) }
public enum FleetDecoder {
    /// Row-by-row lenient decode of the list envelope (JSONSerialization
    /// split, so integers are never laundered into doubles).
    public static func decodeList(_ data: Data) throws -> FleetDecodeResult
}
```

Invariants (each tested):
- AT THE GATE requires all three durable facts: `projection.phase == terminal` AND `projection.outcome == verified` AND `receipt.verified == valid`. An invalid or absent proof lands the row in WRECKED, unconditionally.
- NEEDS REVIEW is terminal + `outcome == needs_review` (judge uncertain / asked for revision): an operator decision by definition (design C1: "ranked lower and painted amber, never hidden"). Its rows carry `needsDecision == true`, count into `decisionCount`, badge the menubar, and never claim a VERIFIED chip. The receipt.valid gate for AT THE GATE is unchanged.
- WRECKED is every other terminal row (failed/blocked/cancelled/exhausted/proof-invalid); the row's own glossary words say which.
- Unknown phase quarantines into the `unknown` section, never guessed, never crashed. A row that fails FleetRow decode costs exactly that row.
- Ranking is durable facts only: decision-readiness first (decision-shaped waiting outranks within UNDERWAY; verifying_meaning outranks verifying_checks within APPROACHING as pipeline position), then recency. Gate counts NEVER rank (failed attempts have no marker and no counts).
- Decision-shaped requires `phase == waiting`; the same stop reason on any other phase does not badge.
- The rollup carries no semantic-judgment field, so the C1 judge chip is dropped (not re-derived from raw files). If `list --json` grows a judgment field, add it to FleetRow and surface the chip in the same change.
- Legacy runs are counted, not queued: RunRow lacks the durable-Job facts the taxonomy speaks, so runs surface as a count until the v1.x voyage-tree/legacy surface. Plans/chains ride the envelope undecoded until then (unknown keys ignored).

### Fleet engine (Services/FleetCLIClient.swift, Services/JobsWatcher.swift)

```swift
public protocol FleetCLIRunning: AnyObject {
    func run(arguments: [String], timeout: TimeInterval) async throws -> CLIRunResult
    @discardableResult
    func terminateInFlight(patience: TimeInterval) -> Int
    /// Children still running; quit-time teardown polls this so the process
    /// outlives its children (or their SIGKILL escalation).
    var inFlightCount: Int { get }
}
public enum FleetCLIError: Error, LocalizedError, Equatable {
    case binaryUnavailable(String)     // typed: locator failure words, verbatim
}
/// Locates via BinaryLocator (fail closed), tracks live CLIRunner children,
/// SIGTERMs them all on demand. A hung child is terminated after `timeout`
/// and reports its real exit.
public final class DeadreckonCLIClient: FleetCLIRunning {
    public init(workingDirectory: String, environment: [String: String])
}

public enum DeadreckonHome {                 // mirrors DeadreckonPaths::discover
    public static func url() -> URL          // non-empty DEADRECKON_HOME env, else ~/.deadreckon
    public static func jobsDirectory() -> URL
}
public protocol FleetWatching: AnyObject {
    func start(onChange: @escaping () -> Void)
    func stop()
    var isActive: Bool { get }   // false after a no-op start (directory absent): retry later
}
/// FSEventStream over DEADRECKON_HOME/jobs only; a wake-up hint, never a
/// read path (TAILING.md: polling is the mechanism). Watching the jobs
/// subtree keeps gate-keys/ outside the watched path by construction
/// (trust rule 3). Inert when the directory does not exist yet
/// (isActive == false); FleetStore retries start after successful polls.
/// Thread discipline: stream and onChange are confined to the private
/// FSEvents delivery queue (start/stop/deinit tear down via queue.sync), so
/// the callback's onChange read is ordered against mutation and no callback
/// can outlive stop() — the unretained stream context is safe by construction.
public final class JobsDirectoryWatcher: FleetWatching {
    public init(directory: URL, latency: CFTimeInterval)
}
```

### FleetStore (Services/FleetStore.swift)

```swift
@MainActor
public final class FleetStore: ObservableObject {
    public struct Cadence: Equatable, Sendable {   // injectable for tests
        public var windowVisible: TimeInterval     // default 2 s
        public var menubarOnly: TimeInterval       // default 10 s
        public var harbor: TimeInterval            // default 60 s
        public static let standard: Cadence
    }
    public enum FleetState: Equatable {
        case loading
        case loaded(GateQueue)
        case unavailable(reason: String)   // typed; never fake rows
    }
    public struct HarborState: Equatable {
        public enum Providers: Equatable { case unknown(String), counted(ok: Int, total: Int) }
        public enum Supervisor: Equatable { case unknown(String), running, stopped(SupervisorInstallState) }
        public enum Doctor: Equatable { case unknown(String), ok(warnings: Int), failed(Int) }
        public var providers: Providers
        public var supervisor: Supervisor
        public var doctor: Doctor
        public static let initial: HarborState
    }

    @Published public private(set) var fleet: FleetState
    @Published public private(set) var harbor: HarborState
    @Published public private(set) var lastRefreshed: Date?
    @Published public private(set) var legacyRunCount: Int
    @Published public private(set) var binaryVersion: String?
    /// Debounced stale-lease verdicts: a job appears only after
    /// staleLeaseConfirmationPolls consecutive stale reports of the same
    /// epoch. Feeds LeaseDotView amber and the menubar degraded badge.
    @Published public private(set) var confirmedStaleLeaseJobIDs: Set<String>
    public static let staleLeaseConfirmationPolls: Int   // 2
    public private(set) var windowVisible: Bool
    public var menuBarState: MenuBarFleetState
    public var supervisorDown: Bool                // true only for .stopped, never .unknown
    public var inFlightChildren: Int               // cli.inFlightCount passthrough
    public var queue: GateQueue                    // .empty unless loaded

    public init(cli: FleetCLIRunning, watcher: FleetWatching?, cadence: Cadence)
    public convenience init()                      // DeadreckonCLIClient + JobsDirectoryWatcher
    public func start()
    public func stop()
    @discardableResult
    public func shutdown(patience: TimeInterval) -> Int  // fence + stop + SIGTERM in-flight children
    public func setWindowVisible(_ visible: Bool)  // reschedules cadence, refreshes immediately
    public func refreshNow() async                 // one list --all --json poll, coalescing
    public func refreshHarborNow() async           // providers + supervisor + doctor + --version
}
```

Invariants (each tested in FleetStoreTests):
- Polls `list --all --json`; FSEvents hints coalesce through refreshNow's in-flight guard so a burst cannot stack children.
- Degradation ladder: binary missing / launch failure / nonzero list exit / undecodable envelope -> `.unavailable` with the failing surface's own words; one bad row -> quarantined, siblings survive; one bad Harbor surface -> that chip `.unknown` with THAT failure's words (a timeout says timeout, a decode failure says decode), independently.
- The store reflects the latest poll: an unavailable poll replaces stale rows rather than presenting them as live.
- Every fleet fact rendered comes off the M0 rollup row; nothing is re-derived from raw files that the rollup already joins.
- Stale-lease debounce (design Bridge risk): one raw `fresh == false` poll never paints amber or badges; confirmation requires consecutive stale polls of the same job+epoch (an epoch change or fresh report resets). The unconfirmed render still says "no heartbeat Ns" in neutral ink — honest words, no false alarm, never an optimistic "fresh".
- Teardown fence: after `shutdown()` no code path launches a child — queued follow-up refreshes, direct refresh calls, and mid-sweep Harbor stages are all guarded. The caller must keep the process alive until `inFlightChildren == 0` or past `patience` so the SIGKILL escalation can fire.
- Poll loops re-bind `self` weakly each iteration and exit when the store is gone: a FleetStore discarded without stop() leaks no immortal sleep loop.
- Watcher retry: after a successful poll, an inert watcher (`isActive == false`, jobs/ absent at launch) is started again, so the first job of a fresh home upgrades menubar latency from slow-cadence to FSEvents without a relaunch.

### Fleet read-models (Models/FleetRow.swift)

```swift
/// One Job row from `list --json` (G3 rollup). Read-only display data.
public struct FleetRow: Codable, Equatable, Sendable {
    public struct Projection: Codable, Equatable, Sendable {
        public let phase: JobPhase
        public let outcome: JobOutcome?
        public let stopReason: StopReason?
        public let attemptCount: Int
        public let caveats: [String]
    }
    public struct Lease: Codable, Equatable, Sendable {
        public let ownerID: String        // owner_id
        public let epoch: Int
        public let heartbeatAgeSeconds: Int
        public let expiresAt: Date        // expires_at (RFC 3339)
        public let fresh: Bool
    }
    public struct Spend: Codable, Equatable, Sendable {
        public let totalCostUSD: Double   // total_cost_usd, a running total
        public let capUSD: Double?
        public let subscription: Bool
        public let wallTimeSeconds: Double?
    }
    public struct Receipt: Codable, Equatable, Sendable {
        public let present: Bool
        public let verified: ProofStatus  // three-valued, never a Bool
        public let error: String?
    }
    public struct Gate: Codable, Equatable, Sendable {
        public let attempt: Int
        public let nPassed: Int           // n_passed
        public let nTotal: Int            // n_total
    }

    public let jobID: String              // job_id
    public let scope: String
    public let goal: String
    public let status: String             // Rust job_status_label glossary word
    public let updatedAt: Date
    public let attempts: Int
    public let outcome: JobOutcome?
    public let stopReason: StopReason?
    public let projection: Projection
    public let lease: Lease?              // null when no readable checkpoint
    public let spend: Spend?              // null when no ledger
    public let provider: String?
    public let sandbox: String?
    public let receipt: Receipt?
    public let gate: Gate?                // present ONLY with a signed marker
}

/// One legacy run row from `list --json`. Leases, receipts, and gate stamps
/// are durable-Job facts and stay absent rather than fabricated.
public struct RunRow: Codable, Equatable, Sendable {
    public let runID: String
    public let scope: String
    public let goal: String
    public let status: String
    public let updatedAt: Date
    public let statePath: String
    public let provider: String?
    public let spend: FleetRow.Spend?
}

/// `list --json` top-level envelope. Plans/chains ride the envelope
/// undecoded (rows-with-child-counts surface is v1.x per design 6.2 scope).
public struct ListEnvelope: Codable, Equatable, Sendable {
    public let kind: String               // "list"
    public let id: String
    public let status: String
    public let nextActions: [String]
    public let tryLines: [String]
    public let jobs: [FleetRow]
    public let runs: [RunRow]
}
```

Invariants:
- `lease.fresh` is display data for a fleet board, never a reclamation decision; `claim_job_lease` in the binary is the only judge of ownership.
- `spend.totalCostUSD` is the spend head, already cumulative; never sum rows across a fleet without labeling it a sum of heads.
- A missing `gate{}` on a failed attempt is correct behavior, not missing data: the only durable per-check evidence is the signed marker.

### Ledger read-models (Models/JobLease.swift)

```swift
/// Full lease.json checkpoint (job-lease.schema.json), for evidence panes.
public struct JobLease: Codable, Equatable, Sendable {
    public let schemaVersion: Int
    public let jobID: String
    public let ownerID: String
    public let epoch: Int
    public let pid: Int
    public let processGroup: Int
    public let childPID: Int?             // child_pid, may be absent
    public let bootID: String
    public let processStartIdentity: String?  // absent on old checkpoints
    public let acquiredAt: Date
    public let heartbeatAt: Date
    public let expiresAt: Date
}

/// One job-events.jsonl line (job-event.schema.json). Strictly sequenced.
public struct JobEvent: Codable, Equatable, Sendable {
    public let schemaVersion: Int
    public let jobID: String
    public let eventID: String
    public let causationID: String
    public let sequence: Int              // 1..N, no gaps
    public let kind: JobEventKind
    public let leaseEpoch: Int            // 0 = trusted controller pre-lease
    public let timestamp: Date
    public let detail: JSONValue
}

/// One spend.jsonl line (spend-record.schema.json). Shared ledger:
/// kind == "loop" rows are run spend; "narrator" rows are not. Never sum
/// across kinds. Defaults mirror the schema: kind "loop", subscription
/// false, estimated false, so legacy rows still parse.
public struct SpendRecord: Codable, Equatable, Sendable {
    public let timestamp: Date
    public let turn: Int
    public let provider: String
    public let model: String
    public let inputTokens: Int
    public let outputTokens: Int
    public let costUSD: Double
    public let totalCostUSD: Double
    public let capUSD: Double?
    public let kind: String
    public let subscription: Bool
    public let estimated: Bool
    public let wallTimeSeconds: Double?
    public let wallTimeCapSeconds: Double?
}

/// steerable{} from status --json / show --json / RunView (G6).
public struct SteerEligibility: Codable, Equatable, Sendable {
    public let steerable: Bool
    public let reason: SteerIneligibleReason?   // absent when steerable
}

/// One notify.jsonl line (TAILING.md, notify-event.schema.json). Since M1
/// the file carries TWO row shapes, distinguished by the presence of `kind`:
/// typed operator-attention signals and the historical delivery-attempt
/// rows. Best-effort, display-only observability — never authority.
public enum NotifyRecord: Codable, Equatable, Sendable {
    case attention(OperatorAttentionRow)        // rows WITH kind: "operator_attention"
    case deliveryAttempt(NotifyDeliveryAttempt) // rows WITHOUT kind
}

/// notify-event.schema.json OperatorAttentionEvent. `summary`/`nextActions`
/// verbatim (trust rule 2); dedupe app-side with stable notification IDs.
public struct OperatorAttentionRow: Codable, Equatable, Sendable {
    public let schemaVersion: Int
    public let kind: String                     // "operator_attention"
    public let reason: OperatorAttentionReason  // forgiving enum: verified_awaiting_promote |
                                                // paused_at_cap | blocked | failed | cancelled |
                                                // waiting_input | unknown
    public let jobID: String?
    public let runID: String?
    public let scope: String?
    public let at: Date
    public let summary: String
    public let nextActions: [String]
}

/// The historical delivery-attempt shape. Rows record *attempts*,
/// including failures (`ok: false` with a `detail`).
public struct NotifyDeliveryAttempt: Codable, Equatable, Sendable {
    public let ts: Date
    public let transition: NotifyTransition
    public let channel: String
    public let ok: Bool
    public let detail: String?
}

/// One proofs/acceptance-progress.jsonl line (TAILING.md, gate.rs
/// AcceptanceProgressEntry). Display data only, never evidence. `status`
/// is a raw string on the Rust side, so it stays a raw string here.
public struct AcceptanceProgressRow: Codable, Equatable, Sendable {
    public struct CheckResult: Codable, Equatable, Sendable {
        public let kind: String
        public let passed: Bool
        public let mustPass: Bool
        public let detail: String
        public let command: String?
        public let cwd: String?
        public let durationMS: Int?
        public let stdout: String?
        public let stderr: String?
    }
    public let checkedAt: Date
    public let status: String
    public let index: Int
    public let total: Int
    public let result: CheckResult?
}
```

### Engine (Services/BinaryLocator.swift, Services/CLIRunner.swift)

```swift
public enum BinaryLocator {
    // Order: DEADRECKON_BIN env override (dev/tests; absolute, executable;
    // skips manifest verification because a locally built binary cannot
    // match the pinned hashes), then Bundle.main
    // Resources/bin/deadreckon_darwin_{arm64|x86_64} by machine arch,
    // verified against Resources/bin/manifest.json sha256. Verification
    // failure throws; there is no unverified fallback. Bundle verification
    // is cached per process.
    public static func locate() throws -> URL
}

public enum BinaryLocatorError: Error, LocalizedError, Equatable {
    case overrideNotAbsolute(String)
    case overrideNotExecutable(String)
    case unsupportedArchitecture(String)
    case binaryNotBundled(String)
    case manifestMissing
    case manifestUnreadable(String)
    case checksumMismatch(binary: String, expected: String, actual: String)
}

public enum CLIRunnerEvent: Sendable, Equatable {
    case stdoutLine(String)
    case stderrLine(String)
    case terminated(exitCode: Int32)      // exactly one, always last
}

/// One-shot result. deadreckon's exit codes 0/1/2/130 are machine signal:
/// a nonzero exit is an outcome to render, not a launch error.
public struct CLIRunResult: Sendable {
    public let stdout: String
    public let stderr: String
    public let exitCode: Int32
}

/// One child process per invocation, never a pool. stdout/stderr parsed
/// line-wise into one AsyncStream (trailing partial lines flushed at EOF).
/// terminate() sends SIGTERM synchronously and escalates to SIGKILL only
/// after `patience` (default 5 s). Mutable runner state (`launched`) is
/// confined to the private queue (launch/terminate/isRunning all
/// synchronize), ahead of the move to strict concurrency.
public final class CLIRunner {
    public init(binary: URL, arguments: [String], workingDirectory: String, environment: [String: String])
    public var events: AsyncStream<CLIRunnerEvent> { get }
    public func launch() throws
    public func terminate(patience: TimeInterval)
    public var isRunning: Bool { get }

    public static func run(binary: URL, arguments: [String], workingDirectory: String, environment: [String: String], timeout: TimeInterval) async throws -> String
    public static func runDetailed(binary: URL, arguments: [String], workingDirectory: String, environment: [String: String], timeout: TimeInterval) async throws -> CLIRunResult
}
```

### Tailer (Services/JSONLTailer.swift)

The docs/TAILING.md reader algorithm, exactly. Blessed files only; anything else carries no guarantees.

```swift
public final class JSONLTailer {
    public enum Mode: Equatable, Sendable {
        case standard            // events/spend/traces/flight-events/notify
        case jobEvents           // + strict sequence 1..N validation
        case acceptanceProgress  // restart-on-anomaly special rule
    }
    public enum PollResult: Equatable, Sendable {
        case none                // nothing new (incl. file not yet created)
        case lines([String])     // complete lines, in order, each valid JSON
        case restarted([String]) // acceptanceProgress only: full re-read
        case corrupt(String)     // strict files only; sticky
    }
    public init(url: URL, mode: Mode)
    public var offset: UInt64 { get }
    public var lastSequence: UInt64 { get }   // jobEvents mode
    public func poll() -> PollResult
}
```

Behavioral invariants (each has a test in JSONLTailerTests):
- **Torn append:** an unterminated final line is retained unparsed and retried; it is never parsed and never corruption.
- **Corruption is strict and sticky** on standard/jobEvents files: a complete line that fails to parse, a shrink below the offset, or a vanished file stops trust permanently; the row is reported, never skipped.
- **jobEvents sequence:** each row's `sequence` must be exactly `lastSequence + 1`; any gap or missing field is corruption. Render "unknown", never a guessed state. (Duplicate `event_id` with different bytes is also corruption per TAILING.md; detecting it requires history retention and lives with the APP-2 read-model layer, not the tailer.)
- **acceptanceProgress anomaly rule:** shrink below offset, disappearance, or ANY parse failure means restart: reset offset to 0, discard retained rows, re-read from the top. If the fresh read still fails to parse (rewrite in flight), stay reset and retry next poll. Never corruption for this file.
- **Polling is the mechanism.** FSEvents is a wake-up hint only; the read path runs as written regardless.
- Not thread-safe; exactly one owner polls a given tailer.

Fleet cadence (exemplar WatchSupervisor precedent): the queue home polls `list --all --json` at ~2 s while the window is visible and ~10 s menubar-only (FleetStore.Cadence, injectable). The bounded-tails contract, as built in APP-3, is stricter than the design's ~8-tail LRU sketch: ONLY the selected job's `JobDetailStore` holds active tails (nine: six run ledgers + job-events + supervisor.out/err), created on workbench open and dropped on close/selection change. The Gate Queue itself needs no tailing, only the rollup.

### Text tailer (Services/TextFileTailer.swift)

```swift
/// Plain-text tail for supervisor.out/err (P5 drawer terminals). NOT a
/// blessed JSONL ledger: no schema, no append-only promise, no corruption
/// verdicts. A shrink resets honestly to a full fresh read.
public final class TextFileTailer {
    public enum PollResult: Equatable, Sendable {
        case none                 // nothing new (incl. file absent)
        case appended(String)     // newly appended bytes, may end mid-line
        case reset(String)        // file shrank: payload is the fresh read
    }
    public init(url: URL)
    public var offset: UInt64 { get }
    public func poll() -> PollResult
}
```

JSONLTailer addition (APP-3): `public var hasRetainedTail: Bool` — true while an unterminated final line is retained (TAILING.md torn append, being retried, never corruption). Feeds the drawer's torn-tail badge.

## APP-3 Chartroom read-models (Models/DetailModels.swift)

Decode-only display shapes; each names its Rust source of truth. All ride `DeadreckonJSON.decoder()`; unknown keys are ignored; absent optionals stay nil, never guessed.

```swift
public struct RunStateDoc          // state.json (PipelineState subset): status word
                                   // (kebab-case RunStatus), turn, phases[] (Timeline),
                                   // spend/wall totals + caps, pause/failure reasons,
                                   // workingDir; activePhaseName computed.
                                   // total_wall_seconds serde(default) -> 0 on legacy.
public struct JobProjectionDoc     // jobs/<id>/projection.json: phase/outcome/stopReason,
                                   // lastSequence, currentLeaseEpoch, attemptCount,
                                   // childRunIDs (LAST = current attempt's run root),
                                   // lastGateAttempt?, caveats.
public struct JobStatusEnvelope    // status <job> --json subset: status word,
                                   // nextActions (next_actions, absent decodes []),
                                   // verifiedProof{status: ProofStatus, error},
                                   // workClock?, job.job.policy.execution.gate.network,
                                   // job.attempts[].{id{scope,runID}, status, steerable}.
                                   // currentSteerable == LAST attempt's steerable{} (G6);
                                   // there is NO top-level steerable on job envelopes.
                                   // nextActions feeds the spine's job-altitude NEXT
                                   // (spine simplification 6).
public struct JobReportEnvelope    // report <job> --json subset (JobReport): the frozen
                                   // AcceptanceSpec as ContractCheck rows (no YAML parsing
                                   // app-side), approved/current sha + matchesApprovedDigest,
                                   // semantic.judgment (decision word + verbatim summary),
                                   // deterministicChecks, receipt{status, contained,
                                   // sandboxBackend, signatureValidationError}, attempts[].
public struct VerdictEnvelope      // verdict <id> --receipt --json subset: status word
                                   // (verified|regressed|unverified, rendered verbatim),
                                   // hadSignedMarker/markerValid, checks[], and
                                   // receiptAudit.facts[{name, pass, detail}] (G7).
                                   // MODELED BUT NOT WIRED: the committed binary's
                                   // verdict refuses JOB refs, so JobDetailStore never
                                   // invokes it (see the store's verdict invariant and
                                   // registered Rust-side gap); decode stays tested so
                                   // the rail can light up the day the verb widens.
public struct DiffSummaryModel     // show <run> --diff --json (DiffSummary) + patches[]?
public struct PatchModel           // PatchEntry: unified, truncated (binary's honesty flag), note
public struct NarrativeStateDoc    // narrative/state.json subset
public struct NarrativeSnapshotDoc // one snapshots.jsonl beat; isUnverifiedOverlay ==
                                   // (status != "deterministic"): fails TOWARD the label
public enum NarrativeStaleness     // fresh/stale/unknown; staleAfterSeconds = 90 (2x the
                                   // narrator's 45 s cadence window)
public struct RunEventRecord       // one events.jsonl line; unknown kinds keep raw kind;
                                   // activityLine renders the ledger's own words
public struct TraceRow             // one traces.jsonl line (detail deliberately not decoded)
public struct FlightManifestDoc    // flight-manifest.json subset (sessions)
public struct CheckpointManifestDoc // checkpoints/<id>/manifest.json subset; fileCount only
public struct DocEntry             // one file under <working_dir>/.deadreckon/docs
```

Invariants:
- `JobStatusEnvelope.currentSteerable` is display eligibility only; a verb refusal after `steerable: true` stays authoritative (trust rule 4).
- `VerdictEnvelope.status` renders verbatim; it never produces the VERIFIED chip — that stays with the shared proof classifier (trust rule 6).
- `NarrativeSnapshotDoc.isUnverifiedOverlay` treats every non-"deterministic" status as overlay: unknown vocabulary can only add the unverified label, never remove it.

## APP-3 spine derivation (Models/SpineDerivation.swift)

```swift
public struct RunSpineInputs       // everything spine_for_run_with_events reads, as values
public struct SpineSnapshot {      // alive/doing/onTrack/wrong/next + band text helpers
    public enum Aliveness { case live(lastEventAgeSeconds: Int), stale(ageSeconds: Int),
                                 dead(reason: String), done }
    public enum AttentionKind { case pausedAtCap, failure, killed, providerError,
                                     stall, reshapeProposed, steerPending }
}
public enum SpineDerivation {
    public static let staleAfterSeconds = 30      // SPINE_STALE_AFTER_SECONDS
    public static func deriveRun(_ inputs: RunSpineInputs, now: Date) -> SpineSnapshot
}
```

Mirrors `crates/deadreckon/src/tui/spine.rs` run-surface semantics (aliveness from newest event age falling back to `state.updated_at`; doing = `run_status_label - turn N - active phase`; on-track ceiling = launch-plan budget before `state.max_*`; attention order pause/failure/killed/provider-error/stall then reshape then pending steers; next = attach/resume/finish, reshape wins). Band text (`aliveText`/`onTrackText`/`wrongText`) matches `spine_plain_lines` wording exactly — plain hyphens, `on_track_text`'s `{gate} - {spend} - {turns}` shape with the gate cell built from the same match so a future job-level count slots in — and the exact strings are pinned in SpineDerivationTests. Documented simplifications vs spine.rs:
1. **Run surface only.** Plan/chain/campaign spines are not ported: the Chartroom observes a durable Job's current attempt, which is always a Run (plan/chain rows open in `attach` per the v1 scope decision).
2. **Reshape detection** is presence-plus-parses-as-JSON-object; spine.rs additionally validates the file as a launch plan.
3. **Pending steers** count `steer-inbox.jsonl` rows with `status == "pending"` via a lenient line scan (malformed lines skipped), equivalent to `pending_steers` on well-formed files.
4. **Gate counts stay `-`** in the on-track cell (run spine parity: spine.rs leaves gate None for runs too; job-level counts render from the signed marker's rollup elsewhere).
5. An unrecognized status word derives an honest stale/attach spine and renders the raw word, never a guessed live state.
6. **Job-altitude NEXT override (deliberate deviation, applied by JobDetailStore, not the pure derivation).** Every Chartroom attempt is a job-owned run, and the run-surface fallback's `deadreckon resume <run>` is refused by the ownership fence for exactly those runs (design 1.2 retires public resume for Jobs). Unless a reshape proposal exists (reshape still wins, the spine.rs invariant), the store replaces NEXT with the job status envelope's own `next_actions[0]` (the friendliness contract, `SpineSnapshot.replacingNext`); when the envelope is unavailable and the run is failed/killed, NEXT becomes `deadreckon status <job>` — observe, never suggest a fenced verb. The pure `SpineDerivation.deriveRun` stays exact run-surface parity (resume and all), tested separately.

## APP-3 turns + integrity (Models/TurnsDerivation.swift)

```swift
public struct TurnModel            // turn, startedAt, token/cost accumulators, entries[]
public enum TurnsDerivation {
    /// Persistent incremental grouping: fold newly polled rows as they
    /// arrive; per-fold cost is proportional to the NEW rows (entries
    /// re-sort only inside touched turns). Sendable, so a large first fold
    /// can run off the main actor.
    public struct Accumulator {
        public init()
        public mutating func fold(events: [RunEventRecord], traces: [TraceRow]) -> [TurnModel]
    }
    /// One-shot grouping (identical semantics to a single fold): groups
    /// events.jsonl + traces.jsonl by turn; entries interleave by timestamp
    /// (ledger-order stable on ties). token_usage_delta/spend_delta
    /// accumulate as counters; unknown kinds land as raw-kind entries;
    /// events without a turn number stay in the Activity feed only.
    public static func group(events: [RunEventRecord], traces: [TraceRow]) -> [TurnModel]
}
public enum JobEventsIntegrity {   // .none | .contiguous(count) | .corrupt(String)
    /// Folds a jobEvents-mode poll into the drawer chip. Corruption is
    /// sticky, mirroring the tailer. label: "events 1..N contiguous" or the
    /// tailer's failure words verbatim.
    public static func derive(previous: JobEventsIntegrity,
                              poll: JSONLTailer.PollResult,
                              lastSequence: UInt64) -> JobEventsIntegrity
}
```

## APP-3 JobDetailStore (Services/JobDetailStore.swift)

```swift
@MainActor
public final class JobDetailStore: ObservableObject {
    public struct Cadence { poll: TimeInterval = 2; reportEveryTicks: Int = 5 }
    public struct ActivityEntry { ordinal, timestamp?, line }
    public struct SpendMeter { loopTotalUSD, narratorTotalUSD, capUSD?, lastLoopTurn, recordCount }
    public struct FlightState { manifest?, eventCount, lastEventSummary?, checkpoints }
    public struct NarrativePane { stateDoc?, latestSnapshot?, latestDeterministic?,
                                  staleness, skippedMalformedRows }

    // Documented ceilings (see the bounded-copies invariant below):
    public static let rawEventLineCeiling: Int            // 2000 trailing raw drawer lines
    public static let supervisorTextCeiling: Int          // ~256 KB per supervisor pane

    // Published: status/statusIssue, report/reportIssue, projection/projectionIssue,
    // lease, runState, spine, activity, rawEventLines/rawEventsDropped,
    // activityIssue, tracesIssue, spendIssue, flightIssue, turns, spendMeter,
    // flight, narrative, liveChecks, docs, integrity, jobEventsTornTail,
    // supervisorOut/Err (+ supervisorOutTruncated/ErrTruncated),
    // changes/changesIssue, patches/patchIssues, currentRunID, isOpen.

    public var nowProvider: () -> Date                    // injectable clock
    public var activeTailCount: Int                       // test observability
    public init(jobID: String, scope: String, goal: String,
                cli: FleetCLIRunning, home: URL = DeadreckonHome.url(),
                cadence: Cadence = .standard)
    public func open()                                    // idempotent; starts the loop
    public func close()                                   // tears every tail down; reopen-safe
    public func pollOnce() async                          // one deterministic tick (tests)
    public func refreshChanges() async                    // show <run> --diff --json, on demand
    public func loadPatch(path: String) async             // + --patch --file <path>
}
```

Invariants (each tested in JobDetailStoreTests):
- **Per-selected-job lifecycle:** created on workbench open, torn down on close. `close()` drops every tailer (`activeTailCount == 0`), SIGTERMs in-flight CLI children of this store, and fences the loop: no CLI child launches after close. Only the SELECTED job holds active tails (bounded-tails contract).
- **Reopen-safe teardown:** `close()` also resets run resolution (`currentRunID = nil`), the supervisor text, and the integrity chip, so `open()` on the SAME store resumes cleanly: run tailers rebuild from offset 0 and the re-read supervisor text lands in cleared strings instead of duplicating. Tested open -> close -> open. The window-close path reaches this (see the views section).
- **Run resolution:** the current attempt is `projection.json.child_run_ids.last`; its run root is `home/runstate/<scope>/runs/<id>`. A new attempt rebuilds the run tailers and resets scrollback/meters (ledgers are per run); no cross-attempt mixing.
- **Projection reads never fabricate absence:** projection.json distinguishes file-absent (honest nil) from exists-but-unreadable (mid-write/corruption). On a transient failure the last good checkpoint is KEPT, `projectionIssue` carries the reason (rendered in the drawer's Job events pane), and the run tailers do not churn. Only a successfully decoded projection can re-point the current attempt.
- **Composition:** CLI reads ride the FleetCLIRunning seam only (`status`, `report`, `show --diff [--patch --file]` on demand); ledgers ride JSONLTailer; supervisor.out/err ride TextFileTailer; projection/lease/state/narrative-state/flight-manifest/checkpoints/docs are plain JSON/file reads. Nothing writes under DEADRECKON_HOME; no path touches gate-keys/ (reads are rooted at `jobs/<id>/`, `runstate/<scope>/runs/<id>/`, and `<working_dir>/.deadreckon/docs` by construction, trust rule 3).
- **`verdict` is deliberately NOT invoked** (tested: zero verdict children for terminal jobs). The committed binary's `verdict` accepts RUN_LIKE references only (reference.rs VERB_REF_SPECS), a Single-shape job's id resolves to the Job kind (the identical-id run match is deduped away), and public verdict on the job-owned child run is refused by the driver fence — so the call can only produce a typed refusal. The evidence rail's receipt band derives from `report --json` (receipt block + recorded deterministic_checks) and says so. **Rust-side gap, needs registering:** teach `verdict` to accept JOB refs (map to the current attempt; `--receipt` audit is read-only, so either skip the driver fence for inspection or make the sidecar write best-effort). The G7 per-digest audit facts and the fresh checks re-run land in the rail with it (VerdictEnvelope stays modeled and tested for that day).
- **Single-shape `show --diff` aliasing is named, not a generic error** (tested). For a Single-shape job the attempt's run id IS the job id (supervisor.rs stamps run_id = job_id), the resolver hands `show` the Job, and the Job branch returns job status ignoring `--diff`/`--patch` (main.rs show_command). When the diff decode fails and the payload is a job_status envelope, changesIssue/patchIssues explain the aliasing explicitly. **Rust-side gap, needs registering:** `show --diff` handed a Job ref should delegate to the current attempt's run DiffSummary (same shape).
- **Tail conformance is stated precisely:** eight of the nine tails are TAILING.md-blessed files read with the blessed algorithm (events/traces/spend/flight-events/acceptance-progress/job-events via JSONLTailer, supervisor.out/err via TextFileTailer's documented plain-text semantics). `narrative/snapshots.jsonl` is NOT blessed by docs/TAILING.md ("files not listed here carry no tailing guarantees"), so it deliberately rides the restart-on-anomaly mode: a rewrite/shrink re-folds the fresh content (tested) instead of freezing the operator's primary pane on a sticky corruption verdict the file's contract never earned. **Rust-side gap, needs registering:** bless narrative/snapshots.jsonl (append-only already holds in narrative.rs) in docs/TAILING.md with a conformance-test row; this tail then upgrades to `.standard` in the same change.
- **Every strict tail surfaces its own corruption** (tested): events -> activityIssue, traces -> tracesIssue, spend -> spendIssue, flight-events -> flightIssue, job-events -> the integrity chip. A `.corrupt` verdict freezes trust in that file but keeps the already-read data visible with the reason rendered in the owning pane; nothing freezes silently.
- **Acceptance-progress restart rule:** `.restarted` replaces the live band wholesale; rows never mix across gate attempts. Live rows are display only, never evidence (TAILING.md), and the empty state names the strict-gate stream-nothing behavior.
- **Integrity chip:** derived exclusively from the jobEvents tailer's verification (contiguous claim, sticky corruption verbatim, torn-tail badge from `hasRetainedTail`).
- **Spend meter:** loop head = LAST `kind == "loop"` row's `total_cost_usd`; narrator split = app-side sum of narrator rows' per-row `cost_usd` (the narrator keeps no head in the shared ledger — documented derivation); the two are never summed together.
- **Narrative:** the latest deterministic beat is retained separately from provider-refreshed beats, so the pane can always render the projection while the overlay carries the "overlay — unverified" label; malformed snapshot rows are counted, not guessed at.
- **Typed degradation:** each CLI surface fails independently into its own `*Issue` string (the failing surface's words); file-backed panes stay live through CLI unavailability. Every CLI refresh captures the generation before its await and discards the result if the store was closed/reopened meanwhile, so a SIGTERMed child's exit words never land in a closed (or reopened) store.
- **Per-tick cost is O(new rows), off-thread when large** (the beachball fix): turns grouping folds only newly polled rows into a persistent `TurnsDerivation.Accumulator` (entries re-sort only inside touched turns); the newest event timestamp and newest error message are running values, never rescans; `steer-inbox.jsonl` re-reads only when its size changed. A fold larger than ~1500 rows (first open of a long history) runs off the main actor with only the `@Published` assignment returning to it. Known residual: the initial backlog's per-line JSON decode still runs on the main actor; a one-time hitch on first open of a very large ledger, revisit if it bites.
- **Bounded copies with honest ceilings** (tested): the parsed Activity scrollback stays unbounded (the searchable surface the pane promises), but the drawer's raw-line pane keeps only the trailing `rawEventLineCeiling` (2000) lines with `rawEventsDropped` counted and rendered ("N older raw lines dropped ... full ledger in events.jsonl"), and supervisorOut/Err are trimmed at line boundaries to ~`supervisorTextCeiling` (256 KB) each with a visible truncation note. Ledger history is never lost — the files on disk remain the source of truth.
- **Spine NEXT rides the job altitude** (tested): see the spine derivation section, simplification 6.

## APP-3 views (Sources, app target — views + shell only)

`JobDetailView.swift` (Chartroom three-pane inside the existing window: live fleet sidebar from the SAME FleetStore, `JobWorkbenchView` keyed by `.id(jobID)` so selection change tears down the previous JobDetailStore via onDisappear; a `WindowVisibilityObserver` (NSViewRepresentable) additionally binds the store to the HOSTING WINDOW's lifecycle — the AppDelegate retains the window, so window close does not fire onDisappear; willClose drives `close()` (no tails or CLI cadence survive a closed window) and didBecomeKey drives the idempotent `open()`, which the store's reopen-safe teardown makes correct; `DetailHeaderView` goal/phase/lease-with-WHY (reuses `LeaseDotView` + confirmed-stale verdicts; the rollup row + FleetStore debounce stay the lease FRESHNESS source, while the full lease.json checkpoint renders as evidence facts in the drawer's Job events pane)/spend-vs-cap (plus a visible "spend tail stopped" note on spendIssue)/wall; `SpineBandView`; `SteerBarView` — LIVE since APP-4 (see the APP-4 views section): steer submit gated on `steerable{}`, kill/promote/send-back route to their confirmation sheets, and 'Open in Terminal' is live), `DetailCenterTabs.swift` (Narrative with deterministic projection + labeled overlay, Activity with search + unbounded scrollback — the filter computes once per body pass — Turns collapsible with a visible tracesIssue note, Timeline phases + event density), `EvidenceRail.swift` (Contract & Checks: frozen spec check-by-check + digest cross-check + network authority + live band + two keys ⚿/⚖ + a RECEIPT EVIDENCE band from `report --json`'s recorded deterministic checks, honestly labeled — no fresh verdict re-run exists for job-owned attempts in the committed binary, see the store's verdict invariant; Changes: diffstat + on-demand per-file patch with truncation honesty; Flight: checkpoint cards + flightIssue note, rewind-apply disabled naming APP-4; Docs: run docs listing or honest empty state), `DetailDrawer.swift` (P5 drawer: Terminal supervisor.out/err with truncation captions | Raw events bounded with a dropped-count note | Job events with integrity chip + torn-tail badge + projectionIssue + lease evidence), `TerminalLauncher.swift` (specstory-mac mechanism: AppleScript into iTerm2/Terminal.app via the apple-events entitlement, executed async on a dedicated serial queue so the Apple-event roundtrip and first-run TCC prompt never block the main thread; TCC denial degrades honestly to pasteboard + open-terminal with a visible note). The narrative overlay NEVER renders in the evidence rail or any promote-adjacent surface; no override affordance exists anywhere in these views.

Accepted transient (documented, not a bug): on sidebar selection change, SwiftUI may fire the incoming workbench's onAppear before the outgoing one's onDisappear, so two JobDetailStores can hold tails for one runloop turn before the old store closes. The bounded-tails wording "only the selected job holds active tails" is therefore steady-state; the overlap is bounded to a single frame by the `.id(jobID)` teardown.

## Sources (app target)

The app target is views + shell only; anything testable lives in the Kit. Per-domain observable models with contracts, never a single god-model (a named exemplar scar).

```swift
/// LSUIElement menubar shell. Lazily-built NSWindow via NSHostingController;
/// activation policy flips .accessory <-> .regular so the Dock icon appears
/// only while the desktop window is open. Owns the FleetStore: window
/// visibility drives poll cadence (showMainWindow -> setWindowVisible(true),
/// windowWillClose -> false). No debug env-var UI hooks in the app delegate
/// (a named exemplar scar).
@MainActor final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    let fleet: FleetStore
    func showMainWindow()
    // applicationShouldTerminate: fleet.shutdown(patience: 2); when children
    // were signaled, .terminateLater and the reply waits until
    // fleet.inFlightChildren == 0 or patience + 0.5 s has passed — so a
    // SIGTERM-ignoring child still meets its in-process SIGKILL escalation
    // instead of being orphaned by an early reply. Else .terminateNow.
}
```

APP-2 views (Sources/Views, views + shell only, all facts from the Kit): `Theme.swift` (paper/card/hairline/ink tiers, `dynamicColor(light:dark:)`, display/body/mono, card chrome, tactile styles, `StatusChip`, and the overlay tokens `scrim`/`overlayShadow`/`onFill` — no view invents its own colors, including filled-chip text), `GateQueueView.swift` (queue home: section list per the Quarterdeck taxonomy including the amber NEEDS REVIEW section, Bridge column discipline per row, `LeaseDotView` amber only on FleetStore's confirmed-stale verdict, header counts + doctor/providers/supervisor chips, P7 empty state, typed-unavailable banner, NavigationStack destination -> `JobDetailView` (the APP-3 Chartroom, replacing the APP-2 stub), `LayCoursePlaceholderSheet` for Command-N), `CommandPalette.swift` (Command-K text filter over goal/id/provider; Enter opens the first match). Since APP-4, rows also carry contextual write verbs (context menu: Promote/Send back/Kill per durable facts) that open the confirmation sheets through WriteSurfaceRouter — every mutation still flows through MutationRunner, never from a row directly.

Startup handshake (partial, APP-2): `deadreckon --version` + `doctor --json` land as Harbor facts (version chip, doctor ok/warn/failed chip from the report's own finding counts). The schema-version refusal (refuse to operate on a `DEADRECKON_HOME` written by a newer binary than the vendored one) still needs a committed binary surface that reports the home's schema version; it lands with that surface, and until then the doctor chip is the honest health signal. **Rust-side gap, needs registering:** no gap-register entry (G1-G10) covers a home-schema-version surface; it fits `doctor --json` (a finding whose detail carries the home's schema version). Register it Rust-side before RELEASE so the section 9 refusal has a landing slot; tracked here until then.

## APP-4 write parity (as built against the committed M1 binary)

The R-M1 shapes landed and APP-4 built against them. Ground truth:
`crates/deadreckon/src/machine_json.rs` (emitter), the per-verb fact
builders (`kill_outcome_facts` main.rs / `kill_job_facts` job.rs, steer.rs,
`print_materialized` + `apply_outcome_facts` lifecycle.rs,
`extend_queue_facts` run.rs, `emit_start_read_only_result` start.rs), and
HOWTO.md "Confirmation flags and the machine launch protocol".

### Envelope models (Models/MutationEnvelopes.swift)

```swift
/// Splits concatenated pretty-printed JSON objects (campaign kill emits one
/// envelope per killed sub-plan, then the campaign envelope). Brace-walk
/// outside strings; returns each object's exact bytes.
public enum EnvelopeStreamParser {
    public static func objects(in text: String) -> [Data]
}

public struct ErrorEnvelope: Codable, Equatable, Sendable {   // G1 refusal
    public let kind: String        // "error"
    public let code: Int           // the process exit code, unchanged (1/2)
    public let verb: String
    public let message: String
    public let tryLines: [String]  // "try_lines", absent decodes []
}

/// Verb facts, decoded eagerly via JSONSerialization (integers never
/// laundered into doubles). Facts a verb did not emit stay nil.
public struct KillFacts       // signal "SIGTERM"|"SIGKILL"|"none" (job kill with
                              // nothing to signal), escalated,
                              // terminalPhaseObserved ("terminal_phase_observed"
                              // as built — NOT the sketched "terminal_phase"),
                              // processesSignalled? (plan kills)
public struct SteerFacts      // queuedAtRaw (verbatim: the correlator against the
                              // typed steer_delivered event), queuedAt: Date?,
                              // inboxSeq, source?, delivery? ("active or next
                              // provider turn" | "next turn boundary")
public struct DeliveryFacts   // finish/materialize/apply: destinationKind
                              // ("export"|"in-place"|"git-branch"), destination
                              // (path or target), stagedFileCount?,
                              // receiptValidated? (DERIVED, see G1 honesty note),
                              // strategy?, cleaned?, alreadyApplied?, source?
public struct ExtendFacts     // parentID?, parentRunID?, contract
                              // ("inherited"|"replaced"), noteRecorded?, queued?

/// One G1 success envelope: shared scaffold + the armed verb's facts.
public struct MutationEnvelope {
    public let kind: String    // the VERB word ("kill","steer","extend","finish",
                               // "launch","job_status" never spoofable: facts
                               // cannot overwrite the scaffold, machine_json.rs)
    public let id, status: String?
    public let nextActions, tryLines: [String]
    public let primaryAction: String?
    public let kill: KillFacts?; public let steer: SteerFacts?
    public let delivery: DeliveryFacts?; public let extend: ExtendFacts?
    public let queued: Bool?
    public init?(data: Data)
}

/// One invocation's complete machine result.
public struct MutationResult {
    public let envelopes: [MutationEnvelope]  // stream order; campaign kill
                                              // surfaces ALL of them
    public let refusal: ErrorEnvelope?        // authoritative when present
    public let rawObjects: [Data]             // for richer decodes (preview, plan)
    public let exitCode: Int32; public let stdout, stderr: String
    public var primary: MutationEnvelope?     // the LAST success envelope
    public var isSuccess, isEnvelopeFree: Bool
    public var envelopeFreeWords: String
    public static func classify(stdout:stderr:exitCode:) -> MutationResult
}
```

Invariants (MutationEnvelopeTests):
- The G1 carve-out is modeled honestly: clap parse failures (exit 2, prose,
  no envelope — including flags the vendored binary predates, like
  `--dry-run` before R-M2) classify as `isEnvelopeFree`; no envelope is ever
  invented from prose.
- Campaign kill streams decode as concatenated objects; every envelope is
  surfaced; the campaign envelope is `primary`.
- `message`/`tryLines` render verbatim (trust rule 2); a refusal envelope is
  authoritative (trust rule 4).

### Launch protocol models (G2)

```swift
/// start --json read-only preview (will_start always false). Decoded via
/// JSONSerialization; launchPlanData is the embedded `launch_plan` payload
/// re-serialized with JSONSerialization (NSNumber integer-ness preserved —
/// never through Codable where 60 would become 60.0) for byte-honest replay.
/// Blocked previews omit the plan and are not launchable. planCeilingUSD is
/// budget.ceiling_usd from the embedded plan: the >$50 acknowledgment keys
/// off the RESOLVED plan, not the form field.
public struct StartPreviewEnvelope {
    public let kind: String            // "start"
    public let goal, selectedMode, selectionSource, reason, provider,
               providerSource, doneCriteria, doneCriteriaSource,
               sourceMode: String?
    public let doneContract: ContractSummary?  // network word + check rows
    public let requiresConfirmation, willStart: Bool
    public let nextActions, tryLines: [String]
    public let launchPlanData: Data?
    public let planCeilingUSD: Double?
    public var isLaunchable: Bool
    public init?(data: Data)
}

/// finish --dry-run --json (G4), decoded EXACTLY as the design doc section
/// 7 G4 specifies: staged[{path,bytes,sha256}], diffstat, destination,
/// irreversible_steps. R-M2 lands the verb concurrently; against the
/// committed M1 binary the flag is a clap parse error and the promote sheet
/// degrades honestly ("promote preview requires the M2 binary") instead of
/// guessing. Report-only either way.
public struct FinishPlanEnvelope: Codable {
    public struct StagedFile { path: String; bytes: Int; sha256: String }
    public struct DiffStat { filesChanged, added, removed: Int? }
    public struct Destination { kind, path, target: String? }
    public let kind: String            // "finish_plan"
    public let id: String?
    public let staged: [StagedFile]    // absent decodes []
    public let diffstat: DiffStat?
    public let destination: Destination?
    public let irreversibleSteps: [String]  // rendered verbatim
}
```

### Write-flow state machines (Models/WriteFlow.swift)

```swift
/// The GUI-honest --i-know-its-a-lot. Bypass impossible by construction:
/// `armsFlag` (required AND typed-amount matches the cap within $0.005) is
/// the ONLY expression that authorizes the flag; nothing settable overrides.
public struct SpendAcknowledgement {
    public static let ceilingUSD: Double  // 50
    public var capUSD: Double?; public var typedAmount: String
    public var required, typedMatches, armsFlag, readyToStart: Bool
    public static func parseAmount(_ text: String) -> Double?
}

extension JobEventKind {
    /// cancelled, failed, blocked, needs_review, verified,
    /// budget_exhausted, deadline_reached.
    public var isTerminalClassification: Bool
}

/// Kill state machine. Amber cancel-requested ONLY from the envelope
/// acceptance; `.terminal` ONLY from fold(jobEventKind:) — there is no
/// exit-code fold on the type, and the tests pin that a clean exit (even an
/// envelope claiming terminal_phase_observed) never resolves. A refusal is
/// sticky. cascadeEnvelopes carries the campaign sub-plan kills. A corrupt
/// ledger folds into resolutionUnavailable (the sheet says file-backed
/// resolution is impossible) instead of waiting in amber forever.
public struct KillProgress {
    public enum Phase { idle, dispatching, cancelRequested(KillFacts),
                        refused(ErrorEnvelope),
                        envelopeFree(exitCode: Int32, words: String),
                        resolutionUnavailable(reason: String),
                        terminal(JobEventKind) }
    public mutating func dispatched()
    public mutating func fold(result: MutationResult)
    public mutating func fold(jobEventKind: JobEventKind)
    public mutating func fold(tailCorruption reason: String)
}

/// Steer chip machine: queued renders the envelope facts (inboxSeq); flips
/// to delivered ONLY on a typed steer_delivered event whose queued_at
/// matches (raw-string equality first, parsed-date within 1 ms fallback).
/// Deliveries observed while the envelope is still in flight are BUFFERED
/// and re-matched the moment .queued lands: arrival order cannot lose the
/// delivered flip (tested).
public struct SteerDeliveredFact { turn: Int?; queuedAtRaw, preview: String? }
public struct SteerDeliveryTracker {
    public enum Phase { idle, submitting, queued(SteerFacts),
                        delivered(SteerFacts, turn: Int?),
                        refused(ErrorEnvelope), envelopeFree(words: String) }
    public mutating func submitted()
    public mutating func fold(result: MutationResult)
    public mutating func fold(deliveries: [SteerDeliveredFact])
    public mutating func reset()
}

/// Binnacle gating from durable facts only: rollup receipt.verified ==
/// .valid (trust rule 6) AND report --json records the receipt block (key
/// 1) AND the semantic judgment (key 2). disabledReason names the FIRST
/// missing fact. The band's label states honestly that the keys are
/// report's RECORDED facts, not a fresh verdict --receipt re-validation —
/// the verdict-on-JOB-refs Rust gap already registered above stands.
public struct PromoteGate {
    public let proofValid, markerKeyPresent, judgmentKeyPresent: Bool
    public let judgmentDecision: String?
    public let promoteEnabled: Bool
    public let disabledReason: String?
    public static func evaluate(receipt: FleetRow.Receipt?,
                                report: JobReportEnvelope?) -> PromoteGate
}
```

### Verb dispatcher (Services/MutationRunner.swift)

```swift
public struct StartRequest { goal: String; provider, model: String?;
                             maxSpendUSD: Double?; projectDirectory: String? }
public enum FinishDestinationChoice {           // mapped 1:1 to documented flags
    case apply(autostash: Bool, cleanup: Bool)  // --autostash / --cleanup
    case export(path: String)                   // --dest DIR (tilde-expanded app-side:
                                                // the runner is shell-free and the Rust
                                                // side takes the path verbatim, so a
                                                // typed "~" must expand here — tested)
}

/// The complete verb surface. There is deliberately no dr-gate case, no
/// sign case, and no parameter that could express "force past a failed
/// digest": the enum is the proof that no code path retries a mutation with
/// different authority (tested: refused finish redispatches byte-identical
/// argv; no --force/--overwrite can appear).
///
/// Flag-injection rule (fix pass): flags come FIRST and a literal `--`
/// terminates the flag section before every OPERATOR-TYPED positional (a
/// goal or note). Pasted text like "--plan=/path/evil.json" or a bullet
/// "- fix login" reaches clap as literal text, never as a flag (tested).
/// The extend note rides a single `--note=<text>` token so dash-values
/// survive clap value parsing. IDs decoded from the binary's own envelopes
/// are not operator text and stay in place.
public enum PlannedVerb {
    case steer(id: String, note: String)              // steer --json -- <id> <note>
    case kill(id: String, escalate: Bool)             // kill <id> [--escalate] --json
    case finishDryRun(id: String, destination: ...)   // finish <id> --dry-run <dest> --json (NO --yes)
    case finish(id: String, destination: ...)         // finish <id> <dest> --yes --json
    case extendJob(parentID: String, goal: String, note: String?)
                                                      // extend [--note=N] --yes --json -- <p> <goal>
    case startPreview(StartRequest)                   // start [--provider..] [--from DIR] --json -- <goal> (NO --yes)
    case startExecute(planFilePath: String, spendAcknowledged: Bool, fromPath: String?)
                                                      // start --plan F --yes [--i-know-its-a-lot] [--from DIR] --json
    public var arguments: [String]                    // argv array, --json always present
    public var timeout: TimeInterval
}

/// The ONLY path that runs state-changing verbs: argv through the
/// FleetCLIRunning seam (never a shell string), literal CLI line exposed
/// for sheets (display-only), result classified into MutationResult.
public final class MutationRunner {
    public init(cli: FleetCLIRunning)
    public func literalCommandLine(for verb: PlannedVerb) -> String
    public func run(_ verb: PlannedVerb) async -> MutationResult
    /// Plan scratch file for the execute leg — app temp dir, NEVER under
    /// DEADRECKON_HOME (trust rule 8; tested).
    public static func writeLaunchPlanFile(_ data: Data) throws -> URL
}
```

### Write coordinators (Services/WriteCoordinators.swift)

`@MainActor ObservableObject` engines the sheets bind to; all injectable
via FleetCLIRunning, all tested with fakes; none renders optimism — new or
changed fleet rows arrive from FleetStore/FSEvents observation only.

- `LayCourseController` — the G2 protocol exactly: `runPreview()` (sets the
  acknowledgment cap from `planCeilingUSD ?? request.maxSpendUSD`),
  `execute()` writes `launchPlanData` verbatim to the scratch plan file and
  replays it; guarded so an unmatched required acknowledgment dispatches
  NOTHING (tested), and the flag rides `acknowledgement.armsFlag` only.
  `lastPlanFileURL` is test observability (reset to nil on every
  `runPreview()` so the displayed execute line never names a previous
  preview's plan file; stale scratch files are swept at app launch via
  `MutationRunner.sweepLaunchPlanFiles`). `request.projectDirectory` rides
  `--from` on BOTH legs (the launch plan does not embed the source; tested).
  Both verbs carry an in-flight guard: a double-click's second task
  dispatches nothing (tested). An envelope-free execute failure states that
  the Job may or may not have queued, points at the file-backed fleet, and
  DROPS the armed preview — a fresh preview is required before another
  Start (tested).
- `LayCourseCatalog` — `providers list --json` + `models --json`; failed
  probes stay visible with `message`/`tryLines` verbatim as fix hints
  (ProviderProbeRow grew those fields, additive); each surface degrades
  into its own failure words independently.
- `KillCoordinator` — dispatch + fold; owns ONE transient sheet-scoped
  jobEvents tail over `jobs/<id>/job-events.jsonl` polled while the sheet
  is open (stop() on dismissal). This is a documented exception to "only
  the selected job's JobDetailStore holds tails": bounded to the sheet's
  lifetime, read-only, strict-seq verified like every jobEvents tail.
  The tail is PRIMED to end-of-file at dispatch time: only rows appended
  after this dispatch can resolve the sheet, so a historical terminal
  classification already in the ledger (a prior attempt's budget_exhausted
  on a paused job, needs_review, an earlier failed) never resolves THIS
  kill — design 2.4.5, tested. A corrupt ledger (strict-sequence violation
  or the file vanishing beneath the tail) folds to resolutionUnavailable
  and the sheet says resolution is impossible (tested). Dispatch is
  guarded: at most one kill per sheet (tested).
- `SteerCoordinator` — submit + `observe(deliveries:)` fed from
  JobDetailStore.steerDeliveries (the workbench's events tail). The tracker
  BUFFERS deliveries observed while the envelope is in flight and
  re-matches the moment .queued lands, so arrival order cannot stick the
  chip at "queued" against a file-backed delivered fact (tested); the view
  additionally replays the store's current array right after submit. An
  in-flight guard drops a second submit while one is running (tested).
- `QuickSteerController` — popover quick-steer: lazy
  `checkEligibility()` via `status <id> --json` steerable{} (disabled
  states name the envelope's reason); submit shares the tracker. The
  popover holds no events tail, so its chip honestly stays "queued ·
  delivery shows in the workbench".
- `PromoteCoordinator` — `loadPreview()` (finish --dry-run; decodes the G4
  finish_plan spec-true INCLUDING `status` "deliverable"|"blocked" and
  `receipt`{validated,error}: a blocked plan renders receipt.error verbatim
  as a refusal-styled band, never as "0 files · +0 −0" — tested) and
  `promote(gate:)` (guarded: a disabled gate never reaches the binary —
  tested; refusal renders verbatim and STOPS; an in-flight guard drops a
  second promote while one finish runs — tested). The `.unsupported` M2-gap
  classification is reserved for the G1 carve-out signature (exit 2, clap
  prose); every OTHER envelope-free result — watchdog SIGTERM, crash,
  locator failure, task cancellation — is `.failed` with the words alone
  plus the exit code, never a fabricated version-gap diagnosis (tested).
  `previewDestination` names the destination the current plan was computed
  for, so the sheet flags a stale preview instead of silently pairing it
  with new flags.
- `SendBackCoordinator` — goal + note editors; `--note` omitted when the
  note is empty; decodes the extend acknowledgment (`noteRecorded`,
  `contract`); empty goal never dispatches; in-flight guard (tested). An
  envelope-free failure sets `mayHaveQueued`: the continuation Job may
  already have queued, so the dispatch stays DISARMED (canSubmit false)
  until the operator explicitly `rearmAfterPossibleQueue()`s after
  checking the file-backed fleet (tested).

### JobDetailStore additions (APP-4, additive)

- `steerDeliveries: [SteerDeliveredFact]` — typed steer_delivered events
  decoded off the events tail (RunEventRecord.Detail grew `queuedAt`
  decoding `queued_at` as the raw string). Per-run, reset on attempt
  rebuild and on close (tested).
- (Fix pass) A `terminalJobEventKind` surface was removed: kill resolution
  has exactly ONE source — the KillCoordinator's own dispatch-primed tail.
  A second store-scoped fold (which resets per attempt and replays full
  history) would invite a future caller to bind kill state to the wrong
  tail.

### APP-4 views (Sources, app target)

`WriteSurfaceRouter` (one `@Published pending: PendingSurface?` — layCourse
/ kill(FleetRow) / promote(FleetRow) / sendBack(FleetRow)); the queue, the
workbench (via environmentObject), and the menubar popover all open the
same sheets through it. `RefusalView` renders every typed refusal verbatim
(message + try lines, selectable) with NO override control anywhere;
`CommandLineView` shows the literal CLI line each sheet will run (2.4.3).

- `LayCourseSheet` (Command-N, replacing the placeholder): goal editor;
  provider rows from the catalog (failed probes visible-but-disabled with
  message + try lines); model picker per provider; spend-cap field;
  preview leg with the resolved plan facts and the done-contract band;
  >$50 swaps Start for the type-the-amount field (border flips only on the
  exact match; Start disabled until `readyToStart`); execute leg shows the
  literal replay line; success says "the row appears when job.json lands".
  **Done-contract deviation, documented:** the step is READ-ONLY detection
  through the preview envelope (`done_criteria`/`done_contract` — checks,
  must_pass, capabilities.network). The app authors no acceptance.yaml:
  `start` has no `--acceptance` flag to point at an app-written file,
  `def-done` has no `--json`, and writing into the operator's project
  would break the single-writer honesty the sheet claims. A missing
  contract refuses with try lines that teach `def-done`, rendered
  verbatim. **Rust-side gap, needs registering:** a `--acceptance <path>`
  (or def-done --json authoring surface) on `start`, so a GUI contract
  step can author without touching the project tree.
- `KillSheet`: the real semantics verbatim (CancelRequested sticky +
  cancel.marker, SIGTERM process groups, 2s grace, SIGKILL,
  supervisor-proven terminal Cancelled), a separate explicit --escalate
  toggle, amber only from the envelope, resolution only from the job-events
  terminal event (KillCoordinator).
- `PromoteSheet` (the converged Binnacle): TWO-KEY band labeled "recorded
  by report --json · fresh verdict on JOB refs is a registered Rust gap";
  CONTRACT table (frozen checkRows crossed with recorded
  deterministic_checks: status, duration, expandable clipped output —
  pairing is positional but VERIFIED on check kind, degrading to the
  unpaired "not recorded" glyph on any identity mismatch so a reordered or
  wrong-revision results list never shows a result against the wrong
  check); digest/receipt chips; CANDIDATE band (dry-run states: plan /
  BLOCKED plan rendered as a refusal band quoting receipt.error verbatim /
  refused / unsupported-M2 (exit-2 carve-out only) / failed, plus a
  stale-preview notice whenever the selected destination differs from the
  one the shown plan was computed for); destination radio mapped 1:1 to
  flags — export has NO invented default path: PROMOTE and the preview
  stay disabled with "no export destination entered" named until the
  operator types one (tilde-expanded app-side); the literal finish line;
  decision bar Promote (gated, disabled reason named) / Send back / Kill
  (each swapping the routed sheet directly — no dismiss()-then-set race)
  reading the gate from the LIVE FleetStore row, not the sheet-open
  snapshot; success shows the envelope's own `deadreckon undo` next-action
  and says the row updates from the files. Opens its own transient
  JobDetailStore for report evidence (sheet-scoped, closed on dismissal —
  same documented exception as the kill tail).
- `SendBackSheet`: follow-up goal + note editors, literal extend line,
  queued acknowledgment (contract inherited/replaced, note recorded). After
  an envelope-free failure the sheet shows the may-have-queued words and an
  explicit "I checked the fleet — re-arm" button (the coordinator keeps the
  dispatch disarmed until it is pressed).
- `MenuBarPopover` (decision 4, replacing the APP-2 menu; MenuBarExtra is
  `.window` style now): NEEDS DECISION rows carry Promote…/Send
  back…/Kill…/Inspect, UNDERWAY rows carry inline quick-steer (lazy
  eligibility) and Kill…. Every destructive item opens the main window
  directly onto the confirmation sheet; the popover NEVER fires a
  destructive verb itself.
- `SteerBarView` (workbench, now live): gated on
  `status.currentSteerable`, tracker refusal downgrades (trust rule 4),
  queued chip -> delivered flip on `detail.steerDeliveries`; decision
  verbs contextual on phase (terminal: Promote/Send back; else Kill).

### Still pending (so nothing squats on the names)

- Operator-attention notify events (operator decision 6): the binary-side
  rows landed in R-M1; real user notifications (stable IDs + launch-time
  catch-up scan) are APP-5 scope. The `notify.jsonl` tail stays
  observability-only.
- `finish --dry-run --json` (G4): the decode model above is spec-true and
  tested; the vendored binary gains the verb when R-M2 lands and the
  CANDIDATE band lights up with no app change beyond re-vendoring.
- `finish --json` on the real path also lands with R-M2; the promote
  success path already decodes the G1 finish envelope shape.
- `follow <id> --json` merged NDJSON `{"source","offset","record"}` (G5
  step 2, M3): replaces the per-job tail fleet; `JSONLTailer` stays for
  the pre-M3 world and as the fallback.
- Rewind has no machine envelope in the M1 binary (it is not one of the
  nine G1 verbs); the Flight tab's rewind affordance stays honestly
  disabled pointing at the CLI until one exists.
