import Foundation

/// The full `lease.json` checkpoint (docs/schemas/job-lease.schema.json).
/// Read-only observation of fenced ownership. The app never judges ownership
/// from this file; `claim_job_lease` in the binary is the only authority.
/// The `list --json` fleet rollup carries a derived subset (`FleetRow.Lease`);
/// this model is for direct checkpoint reads (Chartroom evidence rail).
public struct JobLease: Codable, Equatable, Sendable {
    public let schemaVersion: Int
    public let jobID: String
    public let ownerID: String
    public let epoch: Int
    public let pid: Int
    public let processGroup: Int
    public let childPID: Int?
    public let bootID: String
    /// Same-boot process identity captured at acquisition. Old checkpoints
    /// omit it and retain expiry-only recovery semantics.
    public let processStartIdentity: String?
    public let acquiredAt: Date
    public let heartbeatAt: Date
    public let expiresAt: Date

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case jobID = "job_id"
        case ownerID = "owner_id"
        case epoch
        case pid
        case processGroup = "process_group"
        case childPID = "child_pid"
        case bootID = "boot_id"
        case processStartIdentity = "process_start_identity"
        case acquiredAt = "acquired_at"
        case heartbeatAt = "heartbeat_at"
        case expiresAt = "expires_at"
    }
}

/// One line of `job-events.jsonl` (docs/schemas/job-event.schema.json).
/// Append-only, strictly sequenced 1..N with no gaps: readers must verify
/// continuity (JSONLTailer's `.jobEvents` mode does) and render "unknown"
/// on any gap rather than a guessed state.
public struct JobEvent: Codable, Equatable, Sendable {
    public let schemaVersion: Int
    public let jobID: String
    public let eventID: String
    public let causationID: String
    public let sequence: Int
    public let kind: JobEventKind
    /// Epoch zero is reserved for trusted controller events before a lease is
    /// acquired. Worker control events require the current non-zero epoch.
    public let leaseEpoch: Int
    public let timestamp: Date
    /// Schema leaves this open (`"detail": true`).
    public let detail: JSONValue

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case jobID = "job_id"
        case eventID = "event_id"
        case causationID = "causation_id"
        case sequence
        case kind
        case leaseEpoch = "lease_epoch"
        case timestamp
        case detail
    }
}

/// One line of `spend.jsonl` (docs/schemas/spend-record.schema.json).
/// The ledger is shared: the run loop writes `kind == "loop"` rows and the
/// live narrator writes `kind == "narrator"` rows to the same file. Only
/// `loop` rows are the run's provider spend; never sum across kinds. The
/// last `loop` row's `totalCostUSD` is the spend head (a running total).
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
    /// Defaulted to "loop" so legacy spend.jsonl rows still parse.
    public let kind: String
    /// Defaulted to false (schema default).
    public let subscription: Bool
    /// Defaulted to false (schema default).
    public let estimated: Bool
    public let wallTimeSeconds: Double?
    public let wallTimeCapSeconds: Double?

    enum CodingKeys: String, CodingKey {
        case timestamp
        case turn
        case provider
        case model
        case inputTokens = "input_tokens"
        case outputTokens = "output_tokens"
        case costUSD = "cost_usd"
        case totalCostUSD = "total_cost_usd"
        case capUSD = "cap_usd"
        case kind
        case subscription
        case estimated
        case wallTimeSeconds = "wall_time_seconds"
        case wallTimeCapSeconds = "wall_time_cap_seconds"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        timestamp = try container.decode(Date.self, forKey: .timestamp)
        turn = try container.decode(Int.self, forKey: .turn)
        provider = try container.decode(String.self, forKey: .provider)
        model = try container.decode(String.self, forKey: .model)
        inputTokens = try container.decode(Int.self, forKey: .inputTokens)
        outputTokens = try container.decode(Int.self, forKey: .outputTokens)
        costUSD = try container.decode(Double.self, forKey: .costUSD)
        totalCostUSD = try container.decode(Double.self, forKey: .totalCostUSD)
        capUSD = try container.decodeIfPresent(Double.self, forKey: .capUSD)
        kind = try container.decodeIfPresent(String.self, forKey: .kind) ?? "loop"
        subscription = try container.decodeIfPresent(Bool.self, forKey: .subscription) ?? false
        estimated = try container.decodeIfPresent(Bool.self, forKey: .estimated) ?? false
        wallTimeSeconds = try container.decodeIfPresent(Double.self, forKey: .wallTimeSeconds)
        wallTimeCapSeconds = try container.decodeIfPresent(Double.self, forKey: .wallTimeCapSeconds)
    }
}

/// `steerable{}` as published on `status --json`, `show --json`, and the
/// RunView projection (run-view.schema.json SteerEligibility).
///
/// Legacy caveat (G6, as built in M0): the JSON surfaces derive
/// `driver_fenced` from the ownership stamp, while the steer verb also runs
/// the full plan-lineage fence. A pre-stamping job-owned run can read
/// `steerable: true` yet still receive the typed fence refusal from the verb.
/// The verb's refusal is authoritative: on refusal after `steerable: true`,
/// downgrade the control. Never retry around a refusal.
public struct SteerEligibility: Codable, Equatable, Sendable {
    public let steerable: Bool
    public let reason: SteerIneligibleReason?

    public init(steerable: Bool, reason: SteerIneligibleReason?) {
        self.steerable = steerable
        self.reason = reason
    }
}

/// One line of `notify.jsonl` (docs/TAILING.md,
/// docs/schemas/notify-event.schema.json). Since M1 the file carries TWO row
/// shapes, distinguished by the presence of the `kind` field: typed
/// operator-attention signals (`kind: "operator_attention"`) and the
/// historical delivery-attempt rows (no `kind`). Every append is best-effort
/// and the binary never reads the rows back, so treat this file as
/// display-only observability — never as an authoritative delivery log, and
/// never as evidence of the state it describes (`status --json` and the
/// signed markers stay the source of truth). Dedupe app-side with stable
/// notification IDs.
public enum NotifyRecord: Equatable, Sendable {
    case attention(OperatorAttentionRow)
    case deliveryAttempt(NotifyDeliveryAttempt)
}

extension NotifyRecord: Codable {
    private enum DiscriminatorKeys: String, CodingKey {
        case kind
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: DiscriminatorKeys.self)
        if container.contains(.kind) {
            self = .attention(try OperatorAttentionRow(from: decoder))
        } else {
            self = .deliveryAttempt(try NotifyDeliveryAttempt(from: decoder))
        }
    }

    public func encode(to encoder: Encoder) throws {
        switch self {
        case .attention(let row): try row.encode(to: encoder)
        case .deliveryAttempt(let row): try row.encode(to: encoder)
        }
    }
}

/// A typed operator-attention signal (notify-event.schema.json
/// OperatorAttentionEvent): one row per operator-relevant transition,
/// appended once by the process that owns the fact. `summary` is one
/// glossary-worded line and `nextActions` are real runnable CLI commands —
/// render both verbatim.
public struct OperatorAttentionRow: Codable, Equatable, Sendable {
    public let schemaVersion: Int
    /// Always "operator_attention" (the discriminator).
    public let kind: String
    public let reason: OperatorAttentionReason
    public let jobID: String?
    public let runID: String?
    public let scope: String?
    public let at: Date
    public let summary: String
    public let nextActions: [String]

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case kind
        case reason
        case jobID = "job_id"
        case runID = "run_id"
        case scope
        case at
        case summary
        case nextActions = "next_actions"
    }
}

/// One notification delivery attempt — the historical `notify.jsonl` row
/// shape. Rows record *attempts*, including failures (`ok: false` with a
/// `detail`).
public struct NotifyDeliveryAttempt: Codable, Equatable, Sendable {
    public let ts: Date
    public let transition: NotifyTransition
    public let channel: String
    public let ok: Bool
    public let detail: String?
}

/// One line of `proofs/acceptance-progress.jsonl` (docs/TAILING.md,
/// deadreckon-core gate.rs AcceptanceProgressEntry). Display data only,
/// never evidence: the signed acceptance marker is the only trustworthy
/// record of what the gate concluded. `status` stays a raw string on the
/// Rust side, so it stays a raw string here.
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

        enum CodingKeys: String, CodingKey {
            case kind
            case passed
            case mustPass = "must_pass"
            case detail
            case command
            case cwd
            case durationMS = "duration_ms"
            case stdout
            case stderr
        }
    }

    public let checkedAt: Date
    public let status: String
    public let index: Int
    public let total: Int
    public let result: CheckResult?

    enum CodingKeys: String, CodingKey {
        case checkedAt = "checked_at"
        case status
        case index
        case total
        case result
    }
}
