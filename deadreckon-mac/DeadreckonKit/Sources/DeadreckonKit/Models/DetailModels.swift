import Foundation

// Read-models for the Chartroom drill-in (APP-3). Every shape here decodes a
// committed Rust surface; the source of truth for each is named on the type.
// All are display data: nothing here confers authority (TAILING.md), and no
// model invents status language (glossary discipline).

// MARK: - state.json (PipelineState subset)

/// The subset of `runstate/<scope>/runs/<id>/state.json` (PipelineState,
/// crates/deadreckon-core/src/state.rs) the Chartroom needs: spine inputs and
/// the phase list for the Timeline tab. Unknown keys are ignored; absent
/// optionals stay nil rather than guessed.
public struct RunStateDoc: Codable, Equatable, Sendable {
    public struct Phase: Codable, Equatable, Sendable {
        public struct Identifier: Codable, Equatable, Sendable {
            public let raw: Int
            public init(from decoder: Decoder) throws {
                // PhaseId serializes as a bare integer (newtype struct).
                let container = try decoder.singleValueContainer()
                raw = try container.decode(Int.self)
            }
            public func encode(to encoder: Encoder) throws {
                var container = encoder.singleValueContainer()
                try container.encode(raw)
            }
            public init(raw: Int) { self.raw = raw }
        }

        public let id: Identifier
        public let name: String
        /// kebab-case PhaseStatus word: pending, planned, executing,
        /// completed, failed. Rendered raw when unrecognized.
        public let status: String
        public let updatedAt: Date

        enum CodingKeys: String, CodingKey {
            case id, name, status
            case updatedAt = "updated_at"
        }

        public init(id: Identifier, name: String, status: String, updatedAt: Date) {
            self.id = id
            self.name = name
            self.status = status
            self.updatedAt = updatedAt
        }
    }

    public let goal: String
    public let runID: String
    public let scope: String
    /// kebab-case RunStatus word: pending, planned, executing, completed,
    /// failed, killed.
    public let status: String
    public let currentPhaseID: Int
    public let startedAt: Date
    public let updatedAt: Date
    public let workingDir: String
    public let provider: String?
    public let maxSpendUSD: Double?
    public let maxWallSeconds: Double?
    public let totalSpendUSD: Double
    public let totalWallSeconds: Double
    public let turn: Int
    public let pauseReason: String?
    public let failureReason: String?
    public let phases: [Phase]

    enum CodingKeys: String, CodingKey {
        case goal
        case runID = "run_id"
        case scope
        case status
        case currentPhaseID = "current_phase_id"
        case startedAt = "started_at"
        case updatedAt = "updated_at"
        case workingDir = "working_dir"
        case provider
        case maxSpendUSD = "max_spend_usd"
        case maxWallSeconds = "max_wall_seconds"
        case totalSpendUSD = "total_spend_usd"
        case totalWallSeconds = "total_wall_seconds"
        case turn
        case pauseReason = "pause_reason"
        case failureReason = "failure_reason"
        case phases
    }

    public var activePhaseName: String? {
        phases.first(where: { $0.id.raw == currentPhaseID })?.name
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        goal = try container.decode(String.self, forKey: .goal)
        runID = try container.decode(String.self, forKey: .runID)
        scope = try container.decode(String.self, forKey: .scope)
        status = try container.decode(String.self, forKey: .status)
        currentPhaseID = try container.decode(Int.self, forKey: .currentPhaseID)
        startedAt = try container.decode(Date.self, forKey: .startedAt)
        updatedAt = try container.decode(Date.self, forKey: .updatedAt)
        workingDir = try container.decode(String.self, forKey: .workingDir)
        provider = try container.decodeIfPresent(String.self, forKey: .provider)
        maxSpendUSD = try container.decodeIfPresent(Double.self, forKey: .maxSpendUSD)
        maxWallSeconds = try container.decodeIfPresent(Double.self, forKey: .maxWallSeconds)
        totalSpendUSD = try container.decode(Double.self, forKey: .totalSpendUSD)
        // serde(default) on the Rust side: legacy states may omit it.
        totalWallSeconds = try container.decodeIfPresent(Double.self, forKey: .totalWallSeconds) ?? 0
        turn = try container.decode(Int.self, forKey: .turn)
        pauseReason = try container.decodeIfPresent(String.self, forKey: .pauseReason)
        failureReason = try container.decodeIfPresent(String.self, forKey: .failureReason)
        phases = try container.decodeIfPresent([Phase].self, forKey: .phases) ?? []
    }

    public init(goal: String, runID: String, scope: String, status: String,
                currentPhaseID: Int, startedAt: Date, updatedAt: Date,
                workingDir: String, provider: String?, maxSpendUSD: Double?,
                maxWallSeconds: Double?, totalSpendUSD: Double,
                totalWallSeconds: Double, turn: Int, pauseReason: String?,
                failureReason: String?, phases: [Phase]) {
        self.goal = goal
        self.runID = runID
        self.scope = scope
        self.status = status
        self.currentPhaseID = currentPhaseID
        self.startedAt = startedAt
        self.updatedAt = updatedAt
        self.workingDir = workingDir
        self.provider = provider
        self.maxSpendUSD = maxSpendUSD
        self.maxWallSeconds = maxWallSeconds
        self.totalSpendUSD = totalSpendUSD
        self.totalWallSeconds = totalWallSeconds
        self.turn = turn
        self.pauseReason = pauseReason
        self.failureReason = failureReason
        self.phases = phases
    }
}

// MARK: - projection.json (JobProjection)

/// Full `jobs/<id>/projection.json` (JobProjection, deadreckon-core job.rs):
/// the rebuildable lifecycle checkpoint. `child_run_ids.last` is the current
/// attempt's run, which resolves the run root for every tailed surface.
public struct JobProjectionDoc: Codable, Equatable, Sendable {
    public struct LastGateAttempt: Codable, Equatable, Sendable {
        public let attempt: Int
        public let nPassed: Int
        public let nTotal: Int
        public let finishedAt: Date

        enum CodingKeys: String, CodingKey {
            case attempt
            case nPassed = "n_passed"
            case nTotal = "n_total"
            case finishedAt = "finished_at"
        }
    }

    public let jobID: String
    public let phase: JobPhase
    public let outcome: JobOutcome?
    public let stopReason: StopReason?
    public let lastSequence: Int
    public let currentLeaseEpoch: Int
    public let attemptCount: Int
    public let childRunIDs: [String]
    public let lastGateAttempt: LastGateAttempt?
    public let updatedAt: Date?
    public let caveats: [String]

    enum CodingKeys: String, CodingKey {
        case jobID = "job_id"
        case phase, outcome
        case stopReason = "stop_reason"
        case lastSequence = "last_sequence"
        case currentLeaseEpoch = "current_lease_epoch"
        case attemptCount = "attempt_count"
        case childRunIDs = "child_run_ids"
        case lastGateAttempt = "last_gate_attempt"
        case updatedAt = "updated_at"
        case caveats
    }
}

// MARK: - status <id> --json (job_status envelope subset)

/// Subset of the `status <job> --json` payload
/// (print_job_status_with_facts, crates/deadreckon/src/commands/job.rs).
/// `steerable{}` for a Job lives on the LAST attempt's embedded RunView
/// (G6); there is no top-level steerable on the job envelope.
public struct JobStatusEnvelope: Codable, Equatable, Sendable {
    public struct VerifiedProof: Codable, Equatable, Sendable {
        public let status: ProofStatus
        public let error: String?
    }

    public struct WorkClock: Codable, Equatable, Sendable {
        public let activeElapsedSeconds: Double
        public let remainingSeconds: Double
        public let cutoff: Date
        public let limitingBoundary: String

        enum CodingKeys: String, CodingKey {
            case activeElapsedSeconds = "active_elapsed_seconds"
            case remainingSeconds = "remaining_seconds"
            case cutoff
            case limitingBoundary = "limiting_boundary"
        }
    }

    /// Subset of one embedded attempt RunView: identity plus the live
    /// steer eligibility.
    public struct AttemptRef: Codable, Equatable, Sendable {
        public struct Identity: Codable, Equatable, Sendable {
            public let scope: String
            public let runID: String

            enum CodingKeys: String, CodingKey {
                case scope
                case runID = "run_id"
            }
        }

        public let id: Identity
        /// kebab-case RunStatus word.
        public let status: String
        public let steerable: SteerEligibility?
        public let provider: String?
    }

    public struct JobFacts: Codable, Equatable, Sendable {
        public struct Identity: Codable, Equatable, Sendable {
            public struct Policy: Codable, Equatable, Sendable {
                public struct Execution: Codable, Equatable, Sendable {
                    public struct Gate: Codable, Equatable, Sendable {
                        /// snake_case JobGateNetworkAccess word:
                        /// deny | loopback | full.
                        public let network: String?
                    }
                    public let gate: Gate?
                }
                public let maxSpendUSD: Double?
                public let maxWallSeconds: Double?
                public let maxAttempts: Int?
                public let execution: Execution?

                enum CodingKeys: String, CodingKey {
                    case maxSpendUSD = "max_spend_usd"
                    case maxWallSeconds = "max_wall_seconds"
                    case maxAttempts = "max_attempts"
                    case execution
                }
            }

            public let jobID: String
            public let scope: String
            public let goal: String
            public let sourceCwd: String?
            public let policy: Policy?

            enum CodingKeys: String, CodingKey {
                case jobID = "job_id"
                case scope, goal
                case sourceCwd = "source_cwd"
                case policy
            }
        }

        public let job: Identity
        public let attempts: [AttemptRef]
        public let verifiedReceiptError: String?

        enum CodingKeys: String, CodingKey {
            case job, attempts
            case verifiedReceiptError = "verified_receipt_error"
        }
    }

    public let kind: String
    public let id: String
    /// Rust job_status_label word, rendered not interpreted.
    public let status: String
    /// The binary's own suggested next commands (print_job_status_with_facts
    /// emits exactly one, from job_primary_action): the friendliness
    /// contract, and the right altitude for the spine's NEXT cell — the
    /// run-surface fallback can suggest verbs the ownership fence refuses
    /// for job-owned runs. Absent on legacy fixtures decodes as empty.
    public let nextActions: [String]
    public let verifiedProof: VerifiedProof?
    public let workClock: WorkClock?
    public let job: JobFacts?

    enum CodingKeys: String, CodingKey {
        case kind, id, status
        case nextActions = "next_actions"
        case verifiedProof = "verified_proof"
        case workClock = "work_clock"
        case job
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        kind = try container.decode(String.self, forKey: .kind)
        id = try container.decode(String.self, forKey: .id)
        status = try container.decode(String.self, forKey: .status)
        nextActions = try container.decodeIfPresent([String].self, forKey: .nextActions) ?? []
        verifiedProof = try container.decodeIfPresent(VerifiedProof.self, forKey: .verifiedProof)
        workClock = try container.decodeIfPresent(WorkClock.self, forKey: .workClock)
        job = try container.decodeIfPresent(JobFacts.self, forKey: .job)
    }

    /// The live attempt's steer eligibility: the LAST embedded RunView's
    /// `steerable{}`. Absent (no attempts yet) means the steer bar renders
    /// "no run yet", never a guessed eligibility. Trust rule 4 still holds:
    /// a verb refusal after `steerable: true` is authoritative.
    public var currentSteerable: SteerEligibility? {
        job?.attempts.last?.steerable
    }

    public var currentAttempt: AttemptRef? {
        job?.attempts.last
    }
}

// MARK: - report <job> --json (JobReport subset)

/// Subset of `report <job> --json` (JobReport, commands/report.rs). This is
/// the evidence rail's contract surface: the frozen AcceptanceSpec rendered
/// check-by-check (no YAML parsing app-side; the binary already serialized
/// it), the contract digest cross-check, the two keys, and the receipt block.
public struct JobReportEnvelope: Codable, Equatable, Sendable {
    /// One frozen contract check (AcceptanceCheck, deadreckon-core gate.rs;
    /// serde tag = "kind", snake_case). Unknown kinds decode as `.unknown`
    /// with the raw kind preserved: rendered raw, never guessed.
    public struct ContractCheck: Equatable, Sendable {
        public let kind: String
        public let mustPass: Bool
        /// The check's one identifying detail: args/path/pattern/cwd/command.
        public let subject: String

        public init(kind: String, mustPass: Bool, subject: String) {
            self.kind = kind
            self.mustPass = mustPass
            self.subject = subject
        }
    }

    public struct Contract: Codable, Equatable, Sendable {
        public struct Spec: Codable, Equatable, Sendable {
            public let name: String?
            public let checks: [JSONValue]
        }

        public let path: String?
        public let approvedSHA256: String?
        public let currentSHA256: String?
        public let matchesApprovedDigest: Bool?
        public let spec: Spec?

        enum CodingKeys: String, CodingKey {
            case path
            case approvedSHA256 = "approved_sha256"
            case currentSHA256 = "current_sha256"
            case matchesApprovedDigest = "matches_approved_digest"
            case spec
        }

        /// The frozen checks projected to display rows. Decoded leniently
        /// from the open JSONValue array so one malformed check costs that
        /// row, not the whole report.
        public var checkRows: [ContractCheck] {
            guard let checks = spec?.checks else { return [] }
            return checks.compactMap { value in
                guard case .object(let fields) = value,
                      case .string(let kind)? = fields["kind"] else { return nil }
                var mustPass = true
                if case .bool(let flag)? = fields["must_pass"] { mustPass = flag }
                return ContractCheck(kind: kind, mustPass: mustPass, subject: Self.subject(kind: kind, fields: fields))
            }
        }

        private static func subject(kind: String, fields: [String: JSONValue]) -> String {
            func string(_ key: String) -> String? {
                if case .string(let value)? = fields[key] { return value }
                return nil
            }
            switch kind {
            case "cargo_test":
                if case .array(let args)? = fields["args"] {
                    let words = args.compactMap { if case .string(let s) = $0 { return s } else { return nil } }
                    if !words.isEmpty { return "cargo test \(words.joined(separator: " "))" }
                }
                return "cargo test"
            case "file_exists": return string("path") ?? ""
            case "content_match":
                let path = string("path") ?? ""
                let pattern = string("pattern") ?? ""
                return "\(path) =~ \(pattern)"
            case "build_success": return string("cwd").map { "build in \($0)" } ?? "build"
            case "shell": return string("command") ?? ""
            default: return ""
            }
        }
    }

    public struct SemanticJudgmentDoc: Codable, Equatable, Sendable {
        /// snake_case SemanticDecision word: achieved | revise | uncertain.
        public let decision: String
        public let summary: String?
        public let judgedAt: Date?

        enum CodingKeys: String, CodingKey {
            case decision, summary
            case judgedAt = "judged_at"
        }
    }

    public struct Semantic: Codable, Equatable, Sendable {
        public let judgment: SemanticJudgmentDoc?
    }

    public struct ReceiptBlock: Codable, Equatable, Sendable {
        /// The report's own status word for the receipt, rendered verbatim.
        public let status: String
        public let contained: Bool?
        public let sandboxBackend: String?
        public let signatureValidationError: String?

        enum CodingKeys: String, CodingKey {
            case status, contained
            case sandboxBackend = "sandbox_backend"
            case signatureValidationError = "signature_validation_error"
        }
    }

    public struct Attempt: Codable, Equatable, Sendable {
        public let runID: String
        public let status: String
        public let provider: String
        public let spendUSD: Double
        public let checks: [AcceptanceProgressRow.CheckResult]

        enum CodingKeys: String, CodingKey {
            case runID = "run_id"
            case status, provider
            case spendUSD = "spend_usd"
            case checks
        }
    }

    public let id: String
    public let goal: String
    public let phase: JobPhase
    public let outcome: JobOutcome?
    public let stopReason: StopReason?
    public let contract: Contract?
    public let deterministicChecks: [AcceptanceProgressRow.CheckResult]
    public let semantic: Semantic?
    public let attempts: [Attempt]
    public let receipt: ReceiptBlock?

    enum CodingKeys: String, CodingKey {
        case id, goal, phase, outcome
        case stopReason = "stop_reason"
        case contract
        case deterministicChecks = "deterministic_checks"
        case semantic, attempts, receipt
    }
}

// MARK: - verdict <id> --receipt --json (G7)

/// Subset of `verdict --receipt --json` (verdict_json, commands/verdict.rs).
/// `receiptAudit.facts` are the per-digest audit rows (ReceiptFact,
/// deadreckon-core completion.rs). Inspection only: the strict fail-closed
/// path in the binary remains the sole promotion authority.
public struct VerdictEnvelope: Codable, Equatable, Sendable {
    public struct ReceiptAudit: Codable, Equatable, Sendable {
        public struct Fact: Codable, Equatable, Sendable {
            public let name: String
            public let pass: Bool
            public let detail: String
        }
        public let facts: [Fact]
    }

    public let kind: String
    public let id: String
    /// VerdictState word: verified | regressed | unverified (rendered via
    /// the glossary discipline: the word itself, never re-derived).
    public let status: String
    public let hadSignedMarker: Bool?
    public let markerValid: Bool?
    public let checks: [AcceptanceProgressRow.CheckResult]
    public let receiptAudit: ReceiptAudit?

    enum CodingKeys: String, CodingKey {
        case kind, id, status
        case hadSignedMarker = "had_signed_marker"
        case markerValid = "marker_valid"
        case checks
        case receiptAudit = "receipt_audit"
    }
}

// MARK: - show <run> --diff --json (G10)

/// `show <run> --diff --json` payload: the DiffSummary
/// (deadreckon-core artifacts.rs). With `--patch` the same object carries a
/// `patches` array (PatchEntry) with truncation honesty.
public struct DiffSummaryModel: Codable, Equatable, Sendable {
    public struct FileDelta: Codable, Equatable, Sendable {
        public let path: String
        public let added: Int
        public let removed: Int
        /// snake_case FileDeltaStatus: added | removed | modified.
        public let status: String
    }

    public let filesChanged: Int
    public let added: Int
    public let removed: Int
    public let files: [FileDelta]
    public let patches: [PatchModel]?

    enum CodingKeys: String, CodingKey {
        case filesChanged = "files_changed"
        case added, removed, files, patches
    }
}

/// One exportable per-file unified patch (PatchEntry, artifacts.rs).
/// `truncated` is the binary's own honesty flag (64 KiB budget per file in
/// the whole-run export; a single `--file` selection is exported in full).
public struct PatchModel: Codable, Equatable, Sendable {
    public let path: String
    public let status: String
    public let unified: String
    public let truncated: Bool
    public let note: String?
}

// MARK: - narrative/state.json + snapshots.jsonl

/// `narrative/state.json` (NarrativeState, crates/deadreckon/src/narrative.rs).
public struct NarrativeStateDoc: Codable, Equatable, Sendable {
    public let scope: String
    public let targetID: String
    public let latestSnapshotID: String
    /// snake_case NarrativeStatus word: fresh | stale | failed | disabled |
    /// redacted | deterministic.
    public let latestStatus: String
    public let latestCreatedAt: Date?
    public let lastError: String?

    enum CodingKeys: String, CodingKey {
        case scope
        case targetID = "target_id"
        case latestSnapshotID = "latest_snapshot_id"
        case latestStatus = "latest_status"
        case latestCreatedAt = "latest_created_at"
        case lastError = "last_error"
    }
}

/// One `narrative/snapshots.jsonl` beat (NarrativeSnapshot, narrative.rs).
/// The trust split (design 2.4.4): `status == "deterministic"` is the
/// projection built from ledgers; every other status is provider-refreshed
/// content and MUST render behind the "overlay — unverified" label, and
/// never on a decision surface or in the evidence rail.
public struct NarrativeSnapshotDoc: Codable, Equatable, Sendable {
    public struct Claim: Codable, Equatable, Sendable {
        public let text: String
        public let evidence: [String]
        public let confidence: String
    }

    public struct Live: Codable, Equatable, Sendable {
        public let beatSeq: Int
        public let coversTurn: Int
        /// snake_case NarrativeSource: live | attach.
        public let source: String
        public let rollingSummary: String?

        enum CodingKeys: String, CodingKey {
            case beatSeq = "beat_seq"
            case coversTurn = "covers_turn"
            case source
            case rollingSummary = "rolling_summary"
        }
    }

    public let snapshotID: String
    public let createdAt: Date
    public let status: String
    public let headline: String
    public let currentWork: [Claim]
    public let risks: [Claim]
    public let nextLikely: [Claim]
    public let live: Live?

    enum CodingKeys: String, CodingKey {
        case snapshotID = "snapshot_id"
        case createdAt = "created_at"
        case status, headline
        case currentWork = "current_work"
        case risks
        case nextLikely = "next_likely"
        case live
    }

    /// Provider-refreshed content ("overlay") vs the deterministic
    /// projection. Anything not positively deterministic is treated as
    /// overlay: fail toward the unverified label, never away from it.
    public var isUnverifiedOverlay: Bool {
        status != "deterministic"
    }
}

/// The narrative pane's staleness chip, derived from the snapshot age.
public enum NarrativeStaleness: Equatable, Sendable {
    case fresh(ageSeconds: Int)
    case stale(ageSeconds: Int)
    case unknown

    /// Mirrors the attach TUI's quiet cadence default (45 s between provider
    /// calls, NarrativeCadence in narrative.rs): a beat older than twice the
    /// cadence window is presented as stale.
    public static let staleAfterSeconds = 90

    public static func from(createdAt: Date?, now: Date) -> NarrativeStaleness {
        guard let createdAt else { return .unknown }
        let age = max(0, Int(now.timeIntervalSince(createdAt)))
        return age > staleAfterSeconds ? .stale(ageSeconds: age) : .fresh(ageSeconds: age)
    }
}

// MARK: - events.jsonl (RunEvent) and traces.jsonl (TraceRecord)

/// One `events.jsonl` line (run-event.schema.json). The `event` object is
/// internally tagged by `kind`; unknown kinds decode with their raw kind and
/// no fields, so vocabulary growth costs rendering detail, never the tail.
public struct RunEventRecord: Codable, Equatable, Sendable {
    public struct Detail: Codable, Equatable, Sendable {
        public let kind: String
        public let turn: Int?
        public let toolName: String?
        public let toolCallID: String?
        public let status: String?
        public let preview: String?
        public let inputTokens: Int?
        public let outputTokens: Int?
        public let costUSD: Double?
        public let totalCostUSD: Double?
        public let message: String?
        public let path: String?
        /// steer_delivered events echo the inbox entry's queued_at back;
        /// kept as the raw string because it is the correlator against the
        /// steer envelope's queued_at (SteerDeliveryTracker).
        public let queuedAt: String?
        /// spend_delta rows carry the turn's wall clock (additive decode;
        /// legacy ledgers without the field decode unchanged).
        public let wallTimeSeconds: Double?

        enum CodingKeys: String, CodingKey {
            case kind, turn
            case toolName = "tool_name"
            case toolCallID = "tool_call_id"
            case status, preview
            case inputTokens = "input_tokens"
            case outputTokens = "output_tokens"
            case costUSD = "cost_usd"
            case totalCostUSD = "total_cost_usd"
            case message, path
            case queuedAt = "queued_at"
            case wallTimeSeconds = "wall_time_seconds"
        }

        public init(kind: String, turn: Int? = nil, toolName: String? = nil,
                    toolCallID: String? = nil, status: String? = nil,
                    preview: String? = nil, inputTokens: Int? = nil,
                    outputTokens: Int? = nil, costUSD: Double? = nil,
                    totalCostUSD: Double? = nil, message: String? = nil,
                    path: String? = nil, queuedAt: String? = nil,
                    wallTimeSeconds: Double? = nil) {
            self.kind = kind
            self.turn = turn
            self.toolName = toolName
            self.toolCallID = toolCallID
            self.status = status
            self.preview = preview
            self.inputTokens = inputTokens
            self.outputTokens = outputTokens
            self.costUSD = costUSD
            self.totalCostUSD = totalCostUSD
            self.message = message
            self.path = path
            self.queuedAt = queuedAt
            self.wallTimeSeconds = wallTimeSeconds
        }
    }

    public let timestamp: Date
    public let event: Detail

    public init(timestamp: Date, event: Detail) {
        self.timestamp = timestamp
        self.event = event
    }

    /// One activity-feed line for this event: the fact, in the ledger's own
    /// words. No interpretation beyond naming the kind.
    public var activityLine: String {
        let detail = event
        switch detail.kind {
        case "turn_started":
            return "turn \(detail.turn.map(String.init) ?? "?") started"
        case "tool_call_started":
            return "tool \(detail.toolName ?? "?")"
        case "tool_call_result":
            let status = detail.status ?? "?"
            let preview = (detail.preview ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            return preview.isEmpty ? "result \(status)" : "result \(status) \u{00B7} \(preview)"
        case "token_usage_delta":
            return "tokens in \(detail.inputTokens ?? 0) out \(detail.outputTokens ?? 0)"
        case "spend_delta":
            let cost = detail.costUSD ?? 0
            return String(format: "spend +$%.4f", cost)
        case "docs_checkpoint":
            return "docs \(detail.status ?? "") \(detail.path ?? "")"
        case "error":
            return "error \(detail.message ?? "")"
        case "steer_delivered":
            let preview = (detail.preview ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            return preview.isEmpty
                ? "steer delivered"
                : "steer delivered \u{00B7} \(preview)"
        default:
            return detail.kind
        }
    }
}

/// One `traces.jsonl` line (trace-record.schema.json). `detail` is an open
/// schema field and is deliberately not decoded here.
public struct TraceRow: Codable, Equatable, Sendable {
    public let timestamp: Date
    public let turn: Int
    public let event: String
    public let latencyMS: Int?

    enum CodingKeys: String, CodingKey {
        case timestamp, turn, event
        case latencyMS = "latency_ms"
    }

    public init(timestamp: Date, turn: Int, event: String, latencyMS: Int?) {
        self.timestamp = timestamp
        self.turn = turn
        self.event = event
        self.latencyMS = latencyMS
    }
}

// MARK: - flight-manifest.json + checkpoints/<id>/manifest.json

/// `flight-manifest.json` subset (FlightManifest, deadreckon-core flight.rs).
public struct FlightManifestDoc: Codable, Equatable, Sendable {
    public struct Session: Codable, Equatable, Sendable {
        public let flightSessionID: String
        public let provider: String
        public let deadreckonTurn: Int
        /// snake_case FlightSessionStatus: running | completed | failed |
        /// killed | superseded.
        public let status: String
        public let startedAt: Date

        enum CodingKeys: String, CodingKey {
            case flightSessionID = "flight_session_id"
            case provider
            case deadreckonTurn = "deadreckon_turn"
            case status
            case startedAt = "started_at"
        }
    }

    public let runID: String
    public let sessions: [Session]

    enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case sessions
    }
}

/// `checkpoints/<id>/manifest.json` subset (CheckpointManifest, flight.rs).
/// Cards render PREVIEW information only; the rewind-apply verb is APP-4's
/// (an honest disabled affordance names that).
public struct CheckpointManifestDoc: Codable, Equatable, Sendable {
    public let checkpointID: String
    public let deadreckonTurn: Int
    public let createdAt: Date
    /// snake_case CheckpointTrigger: provider_tool | file_quiet |
    /// provider_exit | manual.
    public let trigger: String
    public let fullAnchor: Bool
    public let fileCount: Int

    enum CodingKeys: String, CodingKey {
        case checkpointID = "checkpoint_id"
        case deadreckonTurn = "deadreckon_turn"
        case createdAt = "created_at"
        case trigger
        case fullAnchor = "full_anchor"
        case files
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        checkpointID = try container.decode(String.self, forKey: .checkpointID)
        deadreckonTurn = try container.decode(Int.self, forKey: .deadreckonTurn)
        createdAt = try container.decode(Date.self, forKey: .createdAt)
        trigger = try container.decode(String.self, forKey: .trigger)
        fullAnchor = try container.decode(Bool.self, forKey: .fullAnchor)
        fileCount = (try? container.decode([JSONValue].self, forKey: .files))?.count ?? 0
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(checkpointID, forKey: .checkpointID)
        try container.encode(deadreckonTurn, forKey: .deadreckonTurn)
        try container.encode(createdAt, forKey: .createdAt)
        try container.encode(trigger, forKey: .trigger)
        try container.encode(fullAnchor, forKey: .fullAnchor)
        try container.encode([JSONValue](), forKey: .files)
    }
}

// MARK: - Trace drill-down (VIZ-DRILLDOWN-SPEC §K11)

/// The full tool exchange behind one retained `traces.jsonl` line, decoded
/// ON EXPANSION (pure; held only while the drill is open). Lenient by
/// design: any missing branch yields nils, never a throw — `decode` returns
/// nil only when the line is not a JSON object at all (the view then shows
/// the raw line verbatim, never a guess).
public struct TraceDetailDoc: Equatable, Sendable {
    public struct FlightRow: Equatable, Sendable, Identifiable {
        public let id: String
        public let toolName: String?
        /// shell | edit | …, verbatim.
        public let toolCategory: String?
        /// completed | failed, verbatim.
        public let status: String?
        public let summary: String?
        // Decoded from the flight row's embedded `raw` item JSON when present:
        public let command: String?
        public let aggregatedOutput: String?
        /// The built-in loop's tool traces record stderr as its own stream;
        /// it renders as its own well — streams are never merged silently.
        public let stderrOutput: String?
        public let exitCode: Int?
        public let changedPaths: [String]
    }

    public let provider: String?
    public let model: String?
    public let binary: String?
    public let durationMS: Int?
    public let exitCode: Int?
    public let sandboxBackend: String?
    /// Verbatim when present — a degradation fact, never dropped.
    public let sandboxWarning: String?
    public let workspaceAccess: String?
    public let stdoutPath: String?
    /// The last element of `detail.trace.args` (the prompt on CLI subagents).
    public let promptArg: String?
    public let flightRows: [FlightRow]

    public static func decode(rawTraceLine: String) -> TraceDetailDoc? {
        guard let data = rawTraceLine.data(using: .utf8),
              let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
            return nil
        }
        let detail = object["detail"] as? [String: Any]
        let trace = detail?["trace"] as? [String: Any]
        let args = trace?["args"] as? [Any]
        var rows: [FlightRow] = []
        for (index, rawRow) in ((trace?["flight_rows"] as? [[String: Any]]) ?? []).enumerated() {
            var command: String?
            var output: String?
            var exit: Int?
            var changed: [String] = []
            if let rawItem = rawRow["raw"] as? String,
               let itemData = rawItem.data(using: .utf8),
               let embedded = (try? JSONSerialization.jsonObject(with: itemData)) as? [String: Any],
               let item = embedded["item"] as? [String: Any] {
                command = item["command"] as? String
                output = item["aggregated_output"] as? String
                exit = (item["exit_code"] as? NSNumber)?.intValue
                for change in (item["changes"] as? [[String: Any]]) ?? [] {
                    if let path = change["path"] as? String { changed.append(path) }
                }
            }
            rows.append(FlightRow(
                id: (rawRow["id"] as? String) ?? "row-\(index)",
                toolName: rawRow["tool_name"] as? String,
                toolCategory: rawRow["tool_category"] as? String,
                status: rawRow["status"] as? String,
                summary: rawRow["summary"] as? String,
                command: command,
                aggregatedOutput: output,
                stderrOutput: nil,
                exitCode: exit,
                changedPaths: changed))
        }
        // The built-in loop's tool traces (`event: tool.*`) carry the
        // exchange directly on `detail` — command / stdout / stderr /
        // status_code — with no `trace` envelope. Map that recorded shape
        // into one flight row so the drill renders the exchange instead of
        // an empty block. Every field verbatim; absent -> nil.
        if rows.isEmpty, trace == nil, let detail,
           detail["command"] != nil || detail["stdout"] != nil || detail["stderr"] != nil {
            let stdout = detail["stdout"] as? String
            let stderr = detail["stderr"] as? String
            rows.append(FlightRow(
                id: (detail["tool_call_id"] as? String) ?? "tool",
                toolName: object["event"] as? String,
                toolCategory: nil,
                status: nil,
                summary: nil,
                command: detail["command"] as? String,
                aggregatedOutput: (stdout?.isEmpty ?? true) ? nil : stdout,
                stderrOutput: (stderr?.isEmpty ?? true) ? nil : stderr,
                exitCode: (detail["status_code"] as? NSNumber)?.intValue,
                changedPaths: []))
        }
        return TraceDetailDoc(
            provider: detail?["provider"] as? String,
            model: detail?["model"] as? String,
            binary: trace?["binary"] as? String,
            durationMS: (trace?["duration_ms"] as? NSNumber)?.intValue
                ?? (object["latency_ms"] as? NSNumber)?.intValue,
            exitCode: (trace?["exit_code"] as? NSNumber)?.intValue,
            sandboxBackend: trace?["sandbox_backend"] as? String,
            sandboxWarning: trace?["sandbox_warning"] as? String,
            workspaceAccess: trace?["workspace_access"] as? String,
            stdoutPath: trace?["stdout_path"] as? String,
            promptArg: args?.last as? String,
            flightRows: rows)
    }

    /// True when the decode found nothing renderable — the view then shows
    /// the raw line verbatim instead of an empty expansion (never a blank
    /// block where evidence exists).
    public var isEmpty: Bool {
        provider == nil && model == nil && binary == nil && durationMS == nil
            && exitCode == nil && sandboxBackend == nil && sandboxWarning == nil
            && workspaceAccess == nil && promptArg == nil && flightRows.isEmpty
    }
}

// MARK: - Docs listing

/// One run doc file found under `<working_dir>/.deadreckon/docs`
/// (deadreckon-core docs.rs DOCS_DIR). A listing fact, not content authority.
public struct DocEntry: Equatable, Sendable, Identifiable {
    public let name: String
    public let path: String
    public let bytes: Int
    public let modifiedAt: Date?

    public var id: String { path }

    public init(name: String, path: String, bytes: Int, modifiedAt: Date?) {
        self.name = name
        self.path = path
        self.bytes = bytes
        self.modifiedAt = modifiedAt
    }
}
