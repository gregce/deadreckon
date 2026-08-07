import Foundation

// APP-4 write-parity read-models: the G1/G2 machine envelopes the committed
// M1 binary emits for the nine state-changing verbs, plus the G4 finish_plan
// preview shape (spec-true for the R-M2 binary; the app degrades honestly
// while the vendored binary predates the verb). Ground truth:
// crates/deadreckon/src/machine_json.rs (emitter), the per-verb fact builders
// (kill_outcome_facts, steer.rs, print_materialized, apply_outcome_facts,
// extend_queue_facts), and design doc section 7 G1/G2/G4/G9 "As built".

// MARK: - Envelope stream splitting

/// `kill --json` of a campaign cascades into sub-plan kills, so stdout
/// carries one pretty-printed envelope per killed sub-plan followed by the
/// campaign envelope: concatenated JSON objects, not a single document.
/// This splitter walks braces outside strings and returns each top-level
/// object's exact bytes. Non-object bytes between objects are ignored.
public enum EnvelopeStreamParser {
    public static func objects(in text: String) -> [Data] {
        var results: [Data] = []
        let bytes = Array(text.utf8)
        var depth = 0
        var inString = false
        var escaped = false
        var start: Int?
        for (index, byte) in bytes.enumerated() {
            if inString {
                if escaped {
                    escaped = false
                } else if byte == UInt8(ascii: "\\") {
                    escaped = true
                } else if byte == UInt8(ascii: "\"") {
                    inString = false
                }
                continue
            }
            switch byte {
            case UInt8(ascii: "\""):
                inString = true
            case UInt8(ascii: "{"):
                if depth == 0 { start = index }
                depth += 1
            case UInt8(ascii: "}"):
                guard depth > 0 else { continue }
                depth -= 1
                if depth == 0, let opening = start {
                    results.append(Data(bytes[opening...index]))
                    start = nil
                }
            default:
                break
            }
        }
        return results
    }
}

// MARK: - The G1 refusal envelope

/// `{"kind":"error","code",<exit code>,"verb","message","try_lines"}` on
/// stdout, exit code unchanged. Rendering rule (trust rule 2): `message` and
/// `tryLines` verbatim; a refusal envelope is authoritative (trust rule 4) —
/// there is no override affordance anywhere, and the only recovery
/// affordances a sheet may offer are these try lines and the envelope's own
/// next actions.
public struct ErrorEnvelope: Codable, Equatable, Sendable {
    public let kind: String
    public let code: Int
    public let verb: String
    public let message: String
    public let tryLines: [String]

    enum CodingKeys: String, CodingKey {
        case kind, code, verb, message
        case tryLines = "try_lines"
    }

    public init(kind: String, code: Int, verb: String, message: String, tryLines: [String]) {
        self.kind = kind
        self.code = code
        self.verb = verb
        self.message = message
        self.tryLines = tryLines
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        kind = try container.decode(String.self, forKey: .kind)
        code = try container.decode(Int.self, forKey: .code)
        verb = try container.decode(String.self, forKey: .verb)
        message = try container.decode(String.self, forKey: .message)
        tryLines = try container.decodeIfPresent([String].self, forKey: .tryLines) ?? []
    }
}

// MARK: - Verb-specific outcome facts

/// `kill --json` facts (kill_outcome_facts / kill_job_facts): `signal` is
/// "SIGTERM" | "SIGKILL" | "none" (a Job kill that had nothing to signal),
/// `terminal_phase_observed` is the binary's own observation — the app's
/// kill state machine still resolves ONLY on the job-events terminal event,
/// never on this flag and never on the exit code.
public struct KillFacts: Equatable, Sendable {
    public let signal: String
    public let escalated: Bool
    public let terminalPhaseObserved: Bool
    /// Plan kills additionally report how many processes were signalled.
    public let processesSignalled: Int?

    public init(signal: String, escalated: Bool, terminalPhaseObserved: Bool,
                processesSignalled: Int? = nil) {
        self.signal = signal
        self.escalated = escalated
        self.terminalPhaseObserved = terminalPhaseObserved
        self.processesSignalled = processesSignalled
    }
}

/// `steer --json` facts (steer.rs): the queued chip's evidence. `queuedAtRaw`
/// is kept verbatim because it is the correlator against the typed
/// `steer_delivered` event in events.jsonl (the run loop echoes the same
/// `queued_at` back on delivery).
public struct SteerFacts: Equatable, Sendable {
    public let queuedAtRaw: String
    public let queuedAt: Date?
    public let inboxSeq: Int
    public let source: String?
    /// "active or next provider turn" (codex-server mid-turn path) or
    /// "next turn boundary" (every other provider). Rendered verbatim.
    public let delivery: String?

    public init(queuedAtRaw: String, inboxSeq: Int, source: String?, delivery: String?) {
        self.queuedAtRaw = queuedAtRaw
        self.queuedAt = DeadreckonJSON.date(from: queuedAtRaw)
        self.inboxSeq = inboxSeq
        self.source = source
        self.delivery = delivery
    }
}

/// `finish` / `materialize` / `apply --json` destination facts. `kind` is
/// "export" | "in-place" | "git-branch". `receiptValidated` honesty (G1 as
/// built): it means the outcome rode the verified-delivery authority path,
/// DERIVED, not a fresh receipt re-validation at print time.
public struct DeliveryFacts: Equatable, Sendable {
    public let destinationKind: String?
    /// export/in-place: the path; git-branch: the target branch.
    public let destination: String?
    public let stagedFileCount: Int?
    public let receiptValidated: Bool?
    public let strategy: String?
    public let cleaned: Bool?
    public let alreadyApplied: Bool?
    public let source: String?

    public init(destinationKind: String?, destination: String?, stagedFileCount: Int?,
                receiptValidated: Bool?, strategy: String? = nil, cleaned: Bool? = nil,
                alreadyApplied: Bool? = nil, source: String? = nil) {
        self.destinationKind = destinationKind
        self.destination = destination
        self.stagedFileCount = stagedFileCount
        self.receiptValidated = receiptValidated
        self.strategy = strategy
        self.cleaned = cleaned
        self.alreadyApplied = alreadyApplied
        self.source = source
    }
}

/// `extend --json` facts (G9 as built): the envelope kind stays "extend",
/// the facts ride as top-level fields, and the note text is NOT echoed back
/// (`note_recorded` is the acknowledgment; the app knows what it sent).
public struct ExtendFacts: Equatable, Sendable {
    public let parentID: String?
    public let parentRunID: String?
    /// "inherited" (parent's frozen contract carries over) or "replaced"
    /// (--acceptance was explicit). Rendered verbatim.
    public let contract: String?
    public let noteRecorded: Bool?
    public let queued: Bool?

    public init(parentID: String?, parentRunID: String?, contract: String?,
                noteRecorded: Bool?, queued: Bool?) {
        self.parentID = parentID
        self.parentRunID = parentRunID
        self.contract = contract
        self.noteRecorded = noteRecorded
        self.queued = queued
    }
}

// MARK: - The shared success envelope

/// One G1 success envelope: the shared `{kind,id,status,next_actions,
/// try_lines}` scaffold plus the armed verb's facts at the top level.
/// Decoded via JSONSerialization (integers never laundered into doubles);
/// facts the verb did not emit stay nil, never guessed.
public struct MutationEnvelope: Equatable, Sendable {
    public let kind: String
    public let id: String?
    public let status: String?
    public let nextActions: [String]
    public let tryLines: [String]
    public let primaryAction: String?
    public let kill: KillFacts?
    public let steer: SteerFacts?
    public let delivery: DeliveryFacts?
    public let extend: ExtendFacts?
    public let queued: Bool?

    public init?(data: Data) {
        guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let kind = object["kind"] as? String else { return nil }
        self.kind = kind
        id = object["id"] as? String
        status = object["status"] as? String
        nextActions = object["next_actions"] as? [String] ?? []
        tryLines = object["try_lines"] as? [String] ?? []
        primaryAction = object["primary_action"] as? String
        queued = object["queued"] as? Bool

        if let signal = object["signal"] as? String,
           let escalated = object["escalated"] as? Bool {
            kill = KillFacts(
                signal: signal,
                escalated: escalated,
                terminalPhaseObserved: object["terminal_phase_observed"] as? Bool ?? false,
                processesSignalled: (object["processes_signalled"] as? NSNumber)?.intValue)
        } else {
            kill = nil
        }

        if let queuedAt = object["queued_at"] as? String,
           let inboxSeq = (object["inbox_seq"] as? NSNumber)?.intValue {
            steer = SteerFacts(
                queuedAtRaw: queuedAt,
                inboxSeq: inboxSeq,
                source: object["source"] as? String,
                delivery: object["delivery"] as? String)
        } else {
            steer = nil
        }

        if let destinationObject = object["destination"] as? [String: Any] {
            delivery = DeliveryFacts(
                destinationKind: destinationObject["kind"] as? String,
                destination: (destinationObject["path"] as? String)
                    ?? (destinationObject["target"] as? String),
                stagedFileCount: (object["staged_file_count"] as? NSNumber)?.intValue,
                receiptValidated: object["receipt_validated"] as? Bool,
                strategy: object["strategy"] as? String,
                cleaned: object["cleaned"] as? Bool,
                alreadyApplied: object["already_applied"] as? Bool,
                source: object["source"] as? String)
        } else {
            delivery = nil
        }

        if object["parent_run_id"] != nil || object["note_recorded"] != nil {
            extend = ExtendFacts(
                parentID: object["parent_id"] as? String,
                parentRunID: object["parent_run_id"] as? String,
                contract: object["contract"] as? String,
                noteRecorded: object["note_recorded"] as? Bool,
                queued: object["queued"] as? Bool)
        } else {
            extend = nil
        }
    }
}

// MARK: - One mutation's complete machine result

/// Everything one state-changing invocation produced: the success envelopes
/// in stream order (campaign kill: one per killed sub-plan, then the
/// campaign), the typed refusal if one landed, and the raw process facts.
/// The G1 carve-out is modeled honestly: argument-parse failures (clap usage
/// errors — including flags the committed binary does not know yet, like
/// `--dry-run` before R-M2) exit 2 with prose only and NO envelope, so
/// `envelopes` and `refusal` are both empty and the caller must degrade with
/// the prose, never invent an envelope.
public struct MutationResult: Equatable, Sendable {
    public let envelopes: [MutationEnvelope]
    /// The typed refusal (kind "error"), authoritative when present.
    public let refusal: ErrorEnvelope?
    /// Every top-level JSON object on stdout, verbatim bytes, for surfaces
    /// that decode richer shapes (start preview, finish_plan).
    public let rawObjects: [Data]
    public let exitCode: Int32
    public let stdout: String
    public let stderr: String

    public init(envelopes: [MutationEnvelope], refusal: ErrorEnvelope?, rawObjects: [Data],
                exitCode: Int32, stdout: String, stderr: String) {
        self.envelopes = envelopes
        self.refusal = refusal
        self.rawObjects = rawObjects
        self.exitCode = exitCode
        self.stdout = stdout
        self.stderr = stderr
    }

    /// The verb's own outcome envelope: the LAST success envelope in the
    /// stream (campaign kill emits sub-plan envelopes first).
    public var primary: MutationEnvelope? { envelopes.last }

    public var isSuccess: Bool { refusal == nil && !envelopes.isEmpty }

    /// No envelope of either family landed: the G1 carve-out (parse errors)
    /// or a launch failure. The words in stdout/stderr are all there is.
    public var isEnvelopeFree: Bool { refusal == nil && envelopes.isEmpty }

    /// The prose a degraded surface renders when no envelope landed. A
    /// crashed binary can leave BOTH streams empty (e.g. SIGKILL before any
    /// output); the only honest fact then is the exit code, so it is said
    /// rather than rendering a blank failure.
    public var envelopeFreeWords: String {
        let words = stderr.isEmpty ? stdout : stderr
        if words.isEmpty {
            return "exit \(exitCode) with no output"
        }
        return String(words.prefix(600))
    }

    /// Classify a finished CLI invocation. Any object whose kind is "error"
    /// is the refusal (the binary emits at most one, last); everything else
    /// with a kind is a success envelope.
    public static func classify(stdout: String, stderr: String, exitCode: Int32) -> MutationResult {
        let objects = EnvelopeStreamParser.objects(in: stdout)
        var envelopes: [MutationEnvelope] = []
        var refusal: ErrorEnvelope?
        let decoder = DeadreckonJSON.decoder()
        for object in objects {
            if let error = try? decoder.decode(ErrorEnvelope.self, from: object), error.kind == "error" {
                refusal = error
                continue
            }
            if let envelope = MutationEnvelope(data: object), envelope.kind != "error" {
                envelopes.append(envelope)
            }
        }
        return MutationResult(
            envelopes: envelopes, refusal: refusal, rawObjects: objects,
            exitCode: exitCode, stdout: stdout, stderr: stderr)
    }
}

// MARK: - start --json preview (G2 launch protocol, read leg)

/// The read-only launch preview (`start --json` without `--yes`,
/// emit_start_read_only_result in start.rs): `will_start` is always false, a
/// launchable preview embeds `launch_plan` — the exact replayable payload
/// `--plan` accepts. `launchPlanData` is that payload re-serialized through
/// JSONSerialization (NSNumber integer-ness preserved; never through Codable
/// where integers would launder into doubles); write it to disk unchanged
/// and replay it verbatim. Blocked previews omit the field.
public struct StartPreviewEnvelope: Equatable, Sendable {
    public struct ContractSummary: Equatable, Sendable {
        public struct CheckRow: Equatable, Sendable {
            public let kind: String
            public let mustPass: Bool
        }
        /// The declared network capability word (deny | loopback | full).
        public let network: String?
        public let checks: [CheckRow]
    }

    public let kind: String
    public let goal: String?
    public let selectedMode: String?
    public let selectionSource: String?
    public let reason: String?
    public let provider: String?
    public let providerSource: String?
    public let doneCriteria: String?
    public let doneCriteriaSource: String?
    public let doneContract: ContractSummary?
    public let sourceMode: String?
    public let requiresConfirmation: Bool
    public let willStart: Bool
    public let nextActions: [String]
    public let tryLines: [String]
    /// The replayable launch-plan payload bytes; nil on a blocked preview.
    public let launchPlanData: Data?
    /// budget.ceiling_usd from the embedded plan: the resolved spend cap the
    /// >$50 acknowledgment keys off (the plan is the decision, not the form).
    public let planCeilingUSD: Double?

    public var isLaunchable: Bool { launchPlanData != nil }

    public init?(data: Data) {
        guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let kind = object["kind"] as? String, kind == "start" else { return nil }
        self.kind = kind
        goal = object["goal"] as? String
        selectedMode = object["selected_mode"] as? String
        selectionSource = object["selection_source"] as? String
        reason = object["reason"] as? String
        provider = object["provider"] as? String
        providerSource = object["provider_source"] as? String
        doneCriteria = object["done_criteria"] as? String
        doneCriteriaSource = object["done_criteria_source"] as? String
        sourceMode = object["source_mode"] as? String
        requiresConfirmation = object["requires_confirmation"] as? Bool ?? false
        willStart = object["will_start"] as? Bool ?? false
        nextActions = object["next_actions"] as? [String] ?? []
        tryLines = object["try_lines"] as? [String] ?? []

        if let contract = object["done_contract"] as? [String: Any] {
            let capabilities = contract["capabilities"] as? [String: Any]
            let checkObjects = contract["checks"] as? [[String: Any]] ?? []
            let rows = checkObjects.compactMap { check -> ContractSummary.CheckRow? in
                guard let kind = check["kind"] as? String else { return nil }
                return ContractSummary.CheckRow(
                    kind: kind, mustPass: check["must_pass"] as? Bool ?? true)
            }
            if capabilities != nil || !rows.isEmpty {
                doneContract = ContractSummary(
                    network: capabilities?["network"] as? String, checks: rows)
            } else {
                doneContract = nil
            }
        } else {
            doneContract = nil
        }

        if let plan = object["launch_plan"] as? [String: Any] {
            launchPlanData = try? JSONSerialization.data(
                withJSONObject: plan, options: [.sortedKeys])
            let budget = plan["budget"] as? [String: Any]
            planCeilingUSD = (budget?["ceiling_usd"] as? NSNumber)?.doubleValue
        } else {
            launchPlanData = nil
            planCeilingUSD = nil
        }
    }
}

// MARK: - finish --dry-run --json (G4 finish_plan, R-M2 "As built" shape)

/// The promote preview envelope, EXACTLY as the R-M2 emitter builds it
/// (lifecycle.rs `build_finish_plan`, design doc section 7 G4 "As built"):
/// `{"kind":"finish_plan", id, status:"deliverable"|"blocked",
/// receipt:{validated,error}, mode, destination, staged:[{path,bytes,
/// sha256}], diffstat, result_tree_sha256, irreversible_steps,
/// next_actions}`. A BLOCKED plan (receipt tamper / digest mismatch /
/// staging refusal) still exits 0 with the plan on stdout: `status` is
/// "blocked" and `receipt.error` carries the exact fail-closed message,
/// which MUST render verbatim as a refusal — never as a normal plan.
/// Against a binary that predates the verb, `finish --dry-run` is a clap
/// parse error (exit 2, prose, no envelope — the G1 carve-out) and the
/// promote sheet degrades honestly instead of guessing. Report-only either
/// way: real finish re-validates and re-stages from scratch.
public struct FinishPlanEnvelope: Codable, Equatable, Sendable {
    public struct StagedFile: Codable, Equatable, Sendable {
        public let path: String
        public let bytes: Int
        public let sha256: String
    }

    public struct DiffStat: Codable, Equatable, Sendable {
        public let filesChanged: Int?
        public let added: Int?
        public let removed: Int?

        enum CodingKeys: String, CodingKey {
            case filesChanged = "files_changed"
            case added, removed
        }
    }

    public struct Destination: Codable, Equatable, Sendable {
        public let kind: String?
        public let path: String?
        public let target: String?
    }

    public struct Receipt: Codable, Equatable, Sendable {
        public let validated: Bool?
        public let error: String?
    }

    public let kind: String
    public let id: String?
    /// "deliverable" | "blocked", rendered not interpreted beyond the
    /// blocked/deliverable split below.
    public let status: String?
    public let receipt: Receipt?
    public let staged: [StagedFile]
    public let diffstat: DiffStat?
    public let destination: Destination?
    public let resultTreeSHA256: String?
    public let irreversibleSteps: [String]
    public let nextActions: [String]

    /// True exactly when the emitter said "blocked": the CANDIDATE band
    /// renders `receipt.error` verbatim as a refusal, never file counts.
    public var isBlocked: Bool { status == "blocked" }

    enum CodingKeys: String, CodingKey {
        case kind, id, status, receipt, staged, diffstat, destination
        case resultTreeSHA256 = "result_tree_sha256"
        case irreversibleSteps = "irreversible_steps"
        case nextActions = "next_actions"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        kind = try container.decode(String.self, forKey: .kind)
        id = try container.decodeIfPresent(String.self, forKey: .id)
        status = try container.decodeIfPresent(String.self, forKey: .status)
        receipt = try container.decodeIfPresent(Receipt.self, forKey: .receipt)
        staged = try container.decodeIfPresent([StagedFile].self, forKey: .staged) ?? []
        diffstat = try container.decodeIfPresent(DiffStat.self, forKey: .diffstat)
        destination = try container.decodeIfPresent(Destination.self, forKey: .destination)
        resultTreeSHA256 = try container.decodeIfPresent(String.self, forKey: .resultTreeSHA256)
        irreversibleSteps =
            try container.decodeIfPresent([String].self, forKey: .irreversibleSteps) ?? []
        nextActions = try container.decodeIfPresent([String].self, forKey: .nextActions) ?? []
    }
}
