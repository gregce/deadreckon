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

/// One notify.jsonl line (TAILING.md). Best-effort observability.
public struct NotifyRecord: Codable, Equatable, Sendable {
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

Fleet cadence (exemplar WatchSupervisor precedent): the queue home polls `list --all --json` at ~2 s while the window is visible and ~10 s menubar-only (FleetStore.Cadence, injectable). The bounded per-job tail fleet (max ~8 active tails, LRU eviction, focused job at heartbeat cadence) is APP-3 scope: the Gate Queue needs no tailing, only the rollup.

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

APP-2 views (Sources/Views, views + shell only, all facts from the Kit): `Theme.swift` (paper/card/hairline/ink tiers, `dynamicColor(light:dark:)`, display/body/mono, card chrome, tactile styles, `StatusChip`, and the overlay tokens `scrim`/`overlayShadow`/`onFill` — no view invents its own colors, including filled-chip text), `GateQueueView.swift` (queue home: section list per the Quarterdeck taxonomy including the amber NEEDS REVIEW section, Bridge column discipline per row, `LeaseDotView` amber only on FleetStore's confirmed-stale verdict, header counts + doctor/providers/supervisor chips, P7 empty state, typed-unavailable banner, `JobDetailStubView` APP-3 placeholder behind NavigationStack, `LayCoursePlaceholderSheet` for Command-N), `CommandPalette.swift` (Command-K text filter over goal/id/provider; Enter opens the first match). Row actions are navigation only: no mutation verbs exist until APP-4's MutationRunner.

Startup handshake (partial, APP-2): `deadreckon --version` + `doctor --json` land as Harbor facts (version chip, doctor ok/warn/failed chip from the report's own finding counts). The schema-version refusal (refuse to operate on a `DEADRECKON_HOME` written by a newer binary than the vendored one) still needs a committed binary surface that reports the home's schema version; it lands with that surface, and until then the doctor chip is the honest health signal. **Rust-side gap, needs registering:** no gap-register entry (G1-G10) covers a home-schema-version surface; it fits `doctor --json` (a finding whose detail carries the home's schema version). Register it Rust-side before RELEASE so the section 9 refusal has a landing slot; tracked here until then.

## PENDING-M1 (do not build against until R-M1 lands)

R-M1 (G1 + G2 + G9 + steer widening + notify events) is in progress in the Rust workspace. Expected shapes from the design doc section 7; they slot into the Kit as one `Envelopes.swift` + one `MutationRunner.swift` without touching anything above.

### PENDING-M1: global error envelope (G1)

Every state-changing verb invoked with `--json` that refuses or fails emits one envelope on stdout before exiting with the unchanged exit code:

```json
{"kind": "error", "code": "<machine code>", "verb": "<verb>", "message": "<prose>", "try_lines": ["deadreckon ..."]}
```

```swift
public struct ErrorEnvelope: Codable, Equatable, Sendable {   // PENDING-M1
    public let kind: String        // "error"
    public let code: String
    public let verb: String
    public let message: String
    public let tryLines: [String]
}
```

Rendering rule (trust rule 2): `message` and `tryLines` verbatim. A refusal envelope is authoritative (trust rule 4).

### PENDING-M1: verb outcome envelopes (G1)

Verb-specific outcome objects, routed through the same `VerdictSurface` the inspection surfaces already use. Known shapes so far:

- `kill --json`: `{"signal": ..., "escalated": Bool, "terminal_phase": ...}`
- `steer --json`: `{"queued_at": RFC3339, "inbox_seq": Int}`

```swift
public enum VerbOutcome: Equatable, Sendable {                // PENDING-M1
    case kill(signal: String, escalated: Bool, terminalPhase: JobPhase)
    case steer(queuedAt: Date, inboxSeq: Int)
    case extend(ExtendOutcome)
    // one case per landed verb; decoded by verb, not by guessing kinds
}
```

### PENDING-M1: launch protocol (G2)

The supported GUI launch protocol: `start --json` (read-only preview, `will_start: false`, embeds the exact replayable launch-plan payload) then `start --plan <file> --yes --json` (execute). `--i-know-its-a-lot` unlocks >$50 launches. Confirmation contract: `--yes` approves the launch preview; `--no-confirm` skips destructive follow-up confirmations.

### PENDING-M1: send-back (G9)

`extend <parent> "goal" --note "..." --json` appends a typed provenance record and reports the queued continuation:

```json
{"kind": "operator_sendback", "note": "...", "parent_job_id": "...", "new_job_id": "..."}
```

```swift
public struct ExtendOutcome: Codable, Equatable, Sendable {   // PENDING-M1
    public let parentJobID: String
    public let newJobID: String
    public let note: String?
}
```

### PENDING-M1: verb dispatcher

One choke point for every mutation, so trust rules 1, 2, 8, and 9 are enforced in exactly one place:

```swift
/// The ONLY path that runs state-changing verbs. Builds argv, always appends
/// --json, exposes the literal CLI line for the sheet to display, decodes
/// success into VerbOutcome and refusal into ErrorEnvelope. It refuses to
/// construct dr-gate invocations by design (no such verb constructor).
public final class MutationRunner {                           // PENDING-M1
    public init(binary: URL, home: URL?)
    public func literalCommandLine(for verb: PlannedVerb) -> String
    public func run(_ verb: PlannedVerb) async throws -> Swift.Result<VerbOutcome, ErrorEnvelope>
}
```

### PENDING-M1: operator-attention notify events

Operator decision 6: real user notifications ride typed operator-attention events emitted by the binary (R-M1 scope), not app-side inference. The `notify.jsonl` tail (`NotifyRecord`) stays observability-only; the typed event stream's shape lands here when the Rust side commits it. Stable notification IDs plus a launch-time catch-up scan (exemplar pattern) are APP-5 scope.

### PENDING-M2/M3 (noted so nothing squats on the names)

- `finish --dry-run --json` preview envelope (G4): report-only; real finish re-validates from scratch; `irreversible_steps` rendered verbatim.
- `follow <id> --json` merged NDJSON `{"source", "offset", "record"}` with replay offsets (G5 step 2): replaces the per-job tail fleet; `JSONLTailer` stays for the pre-M3 world and as the fallback.
