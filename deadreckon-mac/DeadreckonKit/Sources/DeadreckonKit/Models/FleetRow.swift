import Foundation

/// One Job row from `deadreckon list --json` (the G3 fleet rollup,
/// commands/inspection.rs `job_list_row`). Strictly read-only display data:
/// nothing in this row is a fence, an authority, or a promotion input.
public struct FleetRow: Codable, Equatable, Sendable {
    /// The `projection{}` sub-object: the Job's rebuildable lifecycle
    /// checkpoint. Fleet display derives from this plus glossary words only.
    public struct Projection: Codable, Equatable, Sendable {
        public let phase: JobPhase
        public let outcome: JobOutcome?
        public let stopReason: StopReason?
        public let attemptCount: Int
        public let caveats: [String]

        enum CodingKeys: String, CodingKey {
            case phase
            case outcome
            case stopReason = "stop_reason"
            case attemptCount = "attempt_count"
            case caveats
        }

        public init(
            phase: JobPhase, outcome: JobOutcome?, stopReason: StopReason?,
            attemptCount: Int, caveats: [String]
        ) {
            self.phase = phase
            self.outcome = outcome
            self.stopReason = stopReason
            self.attemptCount = attemptCount
            self.caveats = caveats
        }
    }

    /// The `lease{}` rollup (null when no checkpoint is readable). Freshness
    /// is display data for a fleet board, never a reclamation decision;
    /// `claim_job_lease` in the binary remains the only judge of ownership.
    public struct Lease: Codable, Equatable, Sendable {
        public let ownerID: String
        public let epoch: Int
        public let heartbeatAgeSeconds: Int
        public let expiresAt: Date
        public let fresh: Bool

        enum CodingKeys: String, CodingKey {
            case ownerID = "owner_id"
            case epoch
            case heartbeatAgeSeconds = "heartbeat_age_seconds"
            case expiresAt = "expires_at"
            case fresh
        }

        public init(
            ownerID: String, epoch: Int, heartbeatAgeSeconds: Int,
            expiresAt: Date, fresh: Bool
        ) {
            self.ownerID = ownerID
            self.epoch = epoch
            self.heartbeatAgeSeconds = heartbeatAgeSeconds
            self.expiresAt = expiresAt
            self.fresh = fresh
        }
    }

    /// The `spend{}` rollup: the newest run-loop spend head (null when no
    /// ledger exists). `totalCostUSD` is already a running total; do not sum.
    public struct Spend: Codable, Equatable, Sendable {
        public let totalCostUSD: Double
        public let capUSD: Double?
        public let subscription: Bool
        public let wallTimeSeconds: Double?

        enum CodingKeys: String, CodingKey {
            case totalCostUSD = "total_cost_usd"
            case capUSD = "cap_usd"
            case subscription
            case wallTimeSeconds = "wall_time_seconds"
        }

        public init(
            totalCostUSD: Double, capUSD: Double?, subscription: Bool,
            wallTimeSeconds: Double?
        ) {
            self.totalCostUSD = totalCostUSD
            self.capUSD = capUSD
            self.subscription = subscription
            self.wallTimeSeconds = wallTimeSeconds
        }
    }

    /// The `receipt{}` rollup. `verified` is the shared three-valued proof
    /// classifier, NOT a boolean: `valid` only for a Verified outcome whose
    /// external signed receipt still validates, `invalid` for a Verified
    /// outcome whose proof is missing or tampered, `not-applicable` otherwise.
    public struct Receipt: Codable, Equatable, Sendable {
        public let present: Bool
        public let verified: ProofStatus
        public let error: String?

        public init(present: Bool, verified: ProofStatus, error: String?) {
            self.present = present
            self.verified = verified
            self.error = error
        }
    }

    /// The `gate{}` rollup from `projection.last_gate_attempt`. Present ONLY
    /// when a signature-verified acceptance marker exists for the attempt:
    /// failed gate attempts carry no counts (raw progress rows are untrusted
    /// display data), so rank failed attempts on phase/stopReason, not counts.
    public struct Gate: Codable, Equatable, Sendable {
        public let attempt: Int
        public let nPassed: Int
        public let nTotal: Int

        enum CodingKeys: String, CodingKey {
            case attempt
            case nPassed = "n_passed"
            case nTotal = "n_total"
        }

        public init(attempt: Int, nPassed: Int, nTotal: Int) {
            self.attempt = attempt
            self.nPassed = nPassed
            self.nTotal = nTotal
        }
    }

    public let jobID: String
    public let scope: String
    public let goal: String
    /// The Rust-side status label (`job_status_label`): a glossary word such
    /// as "running", "verified", or "verified_proof_invalid". Render it or a
    /// glossary translation of it; never synthesize new status language.
    public let status: String
    public let updatedAt: Date
    public let attempts: Int
    public let outcome: JobOutcome?
    public let stopReason: StopReason?
    public let projection: Projection
    public let lease: Lease?
    public let spend: Spend?
    public let provider: String?
    public let sandbox: String?
    public let receipt: Receipt?
    public let gate: Gate?

    enum CodingKeys: String, CodingKey {
        case jobID = "job_id"
        case scope
        case goal
        case status
        case updatedAt = "updated_at"
        case attempts
        case outcome
        case stopReason = "stop_reason"
        case projection
        case lease
        case spend
        case provider
        case sandbox
        case receipt
        case gate
    }

    public init(
        jobID: String, scope: String, goal: String, status: String,
        updatedAt: Date, attempts: Int, outcome: JobOutcome?,
        stopReason: StopReason?, projection: Projection, lease: Lease?,
        spend: Spend?, provider: String?, sandbox: String?, receipt: Receipt?,
        gate: Gate?
    ) {
        self.jobID = jobID
        self.scope = scope
        self.goal = goal
        self.status = status
        self.updatedAt = updatedAt
        self.attempts = attempts
        self.outcome = outcome
        self.stopReason = stopReason
        self.projection = projection
        self.lease = lease
        self.spend = spend
        self.provider = provider
        self.sandbox = sandbox
        self.receipt = receipt
        self.gate = gate
    }
}

/// One legacy run row from `deadreckon list --json` (`run_list_row`).
/// Legacy `state.json` artifacts answer honestly only for provider and the
/// spend head; leases, receipts, and gate stamps are durable-Job facts and
/// stay absent rather than fabricated.
public struct RunRow: Codable, Equatable, Sendable {
    public let runID: String
    public let scope: String
    public let goal: String
    public let status: String
    public let updatedAt: Date
    public let statePath: String
    public let provider: String?
    public let spend: FleetRow.Spend?

    enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case scope
        case goal
        case status
        case updatedAt = "updated_at"
        case statePath = "state_path"
        case provider
        case spend
    }

    public init(
        runID: String, scope: String, goal: String, status: String,
        updatedAt: Date, statePath: String, provider: String?,
        spend: FleetRow.Spend?
    ) {
        self.runID = runID
        self.scope = scope
        self.goal = goal
        self.status = status
        self.updatedAt = updatedAt
        self.statePath = statePath
        self.provider = provider
        self.spend = spend
    }
}

/// The `list --json` top-level envelope (`{kind:"list", ...}`). Plans and
/// chains ride the same envelope; their read-models land with APP-2's
/// screens, and unknown keys are ignored by JSONDecoder until then.
public struct ListEnvelope: Codable, Equatable, Sendable {
    public let kind: String
    public let id: String
    public let status: String
    public let nextActions: [String]
    public let tryLines: [String]
    public let jobs: [FleetRow]
    public let runs: [RunRow]

    enum CodingKeys: String, CodingKey {
        case kind
        case id
        case status
        case nextActions = "next_actions"
        case tryLines = "try_lines"
        case jobs
        case runs
    }
}
