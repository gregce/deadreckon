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
                // start.rs emits the COMPILED check rows (acceptance.rs
                // CompiledCheck): no top-level must_pass — the raw
                // AcceptanceCheck nests under "raw" and the compiled row
                // carries the inverse as can_fail. Read whichever the
                // emitter provided; only a shapeless row defaults to the
                // strict reading (must pass).
                let raw = check["raw"] as? [String: Any]
                let mustPass = (check["must_pass"] as? Bool)
                    ?? (raw?["must_pass"] as? Bool)
                    ?? (check["can_fail"] as? Bool).map { !$0 }
                    ?? true
                return ContractSummary.CheckRow(kind: kind, mustPass: mustPass)
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

    /// The typed missing-contract signal: start.rs sets
    /// `done_criteria_source` to StartDoneCriteriaSource::Missing ("missing")
    /// exactly when no done contract exists for the project, and the preview
    /// comes back blocked with try lines teaching `def-done`. The Lay Course
    /// sheet swaps the bare try-line for the inline contract editor on this.
    public var missingDoneContract: Bool { doneCriteriaSource == "missing" }
}

// MARK: - def-done --json (the done-contract authoring surface)

/// The `def_done_result` success envelope (`def-done --json`, acceptance.rs
/// `def_done_contract_envelope`): `status` is "written" after declare/add/
/// edit, "declared" or "default_gate" from show (a missing contract is a
/// normal exit-0 read, never a refusal), "passed" from check. `checks` is
/// the EXACT serialized `AcceptanceSpec` shape `report --json` already
/// emits (serde-tagged `kind` + `must_pass` + the kind-specific fields), so
/// the app never parses YAML — every row here comes from the binary's own
/// envelope, with the kind-specific target facts surfaced raw, never
/// interpreted. `drafted_by` names the drafting route ("<provider> /
/// <model>", or "<name> pack" for provider-free pack adds); nil on reads.
/// Every refusal — missing --yes, provider/critic failures, corrupt YAML —
/// arrives as the shared G1 error envelope with verb "def-done" instead.
public struct DefDoneResultEnvelope: Equatable, Sendable {
    public struct CheckRow: Equatable, Sendable {
        public let kind: String
        public let mustPass: Bool
        /// Kind-specific target facts, raw (gate.rs AcceptanceCheck):
        /// file_exists/content_match carry `path`, shell carries `command`
        /// (+ optional `cwd`), build_success carries `cwd`, content_match
        /// adds `pattern`, cargo_test carries `args`. Fields a kind does
        /// not declare stay nil/empty, never guessed.
        public let path: String?
        public let command: String?
        public let cwd: String?
        public let pattern: String?
        public let args: [String]

        /// The one-line target for a display row: the kind's own primary
        /// fact, verbatim. Nothing is derived or interpreted.
        public var target: String? {
            if let command { return command }
            if let path { return path }
            if let cwd { return cwd }
            if !args.isEmpty { return args.joined(separator: " ") }
            return nil
        }
    }

    public let kind: String
    public let status: String
    /// Where the binary declared the contract: .deadreckon/acceptance.yaml
    /// in the project. nil on a default_gate read (nothing declared).
    public let contractPath: String?
    public let notesPath: String?
    public let name: String?
    public let checkCount: Int
    public let checks: [CheckRow]
    /// capabilities.network: deny | loopback | full (deny is the default).
    public let network: String?
    public let draftedBy: String?
    public let nextActions: [String]

    public init?(data: Data) {
        guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let kind = object["kind"] as? String, kind == "def_done_result",
              let status = object["status"] as? String else { return nil }
        self.kind = kind
        self.status = status
        contractPath = object["contract_path"] as? String
        notesPath = object["notes_path"] as? String
        name = object["name"] as? String
        checkCount = (object["check_count"] as? NSNumber)?.intValue ?? 0
        let checkObjects = object["checks"] as? [[String: Any]] ?? []
        checks = checkObjects.compactMap { check -> CheckRow? in
            guard let kind = check["kind"] as? String else { return nil }
            return CheckRow(
                kind: kind,
                mustPass: check["must_pass"] as? Bool ?? true,
                path: check["path"] as? String,
                command: check["command"] as? String,
                cwd: check["cwd"] as? String,
                pattern: check["pattern"] as? String,
                args: check["args"] as? [String] ?? [])
        }
        let capabilities = object["capabilities"] as? [String: Any]
        network = capabilities?["network"] as? String
        draftedBy = object["drafted_by"] as? String
        nextActions = object["next_actions"] as? [String] ?? []
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

// MARK: - Shared verdict block (verdict_surface.rs verdict_json, as shipped)

/// The `verdict` object every armed G1 success envelope carries
/// (`VerdictSurface::add_to_json`): the one-surface-object rule means prose
/// and JSON come from the same VerdictSurface, so `label`/`explanation`/
/// `evidence` are the binary's own words and render verbatim, never
/// paraphrased. `evidence` rows are `[key, value]` string pairs.
public struct VerdictBlock: Codable, Equatable, Sendable {
    public let kind: String
    public let label: String
    public let subject: String?
    public let recommendedCommand: String?
    public let explanation: String?
    public let evidence: [[String]]

    enum CodingKeys: String, CodingKey {
        case kind, label, subject, explanation, evidence
        case recommendedCommand = "recommended_command"
    }

    public init(kind: String, label: String, subject: String? = nil,
                recommendedCommand: String? = nil, explanation: String? = nil,
                evidence: [[String]] = []) {
        self.kind = kind
        self.label = label
        self.subject = subject
        self.recommendedCommand = recommendedCommand
        self.explanation = explanation
        self.evidence = evidence
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        kind = try container.decode(String.self, forKey: .kind)
        label = try container.decode(String.self, forKey: .label)
        subject = try container.decodeIfPresent(String.self, forKey: .subject)
        recommendedCommand = try container.decodeIfPresent(String.self, forKey: .recommendedCommand)
        explanation = try container.decodeIfPresent(String.self, forKey: .explanation)
        evidence = try container.decodeIfPresent([[String]].self, forKey: .evidence) ?? []
    }

    /// Evidence as (key, value) tuples; rows that are not 2-element pairs
    /// are dropped rather than guessed at.
    public var evidencePairs: [(String, String)] {
        evidence.compactMap { row in
            guard row.count == 2 else { return nil }
            return (row[0], row[1])
        }
    }
}

// MARK: - config show --json (spec §P1, reconciled with the SHIPPED shape)

/// The complete effective configuration (`config_show_command`, main.rs).
/// SHIPPED shape differs from the spec's §P1 guess and this decoder follows
/// the binary: the map is `settings` (not `values`) with per-key
/// `{value, source: "set"|"default"}` provenance; there is no separate
/// `keys` map — key state lives structurally redacted inside `providers`
/// (`api_key` slots read the literal marker "configured", never bytes);
/// `file` is the complete REDACTED document (the app never reads
/// config.toml itself — raw bytes could carry secrets).
public struct ConfigShowEnvelope: Equatable, Sendable {
    public struct Setting: Codable, Equatable, Sendable {
        public let value: JSONValue
        public let source: String

        public var isSet: Bool { source == "set" }

        public init(value: JSONValue, source: String) {
            self.value = value
            self.source = source
        }
    }

    public let kind: String
    public let status: String?
    public let configPath: String
    public let configExists: Bool
    public let settings: [String: Setting]
    /// Redacted provider entries keyed by route id. Every `api_key` slot
    /// inside is the literal marker string, by the binary's structural
    /// redaction — no code path can surface stored key bytes.
    public let providers: [String: JSONValue]
    public let fallback: JSONValue?
    public let providerOverrideFiles: [String]
    /// The complete redacted config document, pretty-printable for the
    /// ADVANCED disclosure. Secrets are already the marker string.
    public let file: JSONValue?
    public let nextActions: [String]
    public let tryLines: [String]
    public let verdict: VerdictBlock?

    /// Fail-closed: anything that is not a `kind:"config"` show envelope
    /// returns nil and the surface degrades with the raw words.
    public init?(data: Data) {
        struct Raw: Codable {
            let kind: String
            let id: String?
            let status: String?
            let action: String?
            let configPath: String?
            let configExists: Bool?
            let settings: [String: Setting]?
            let providers: [String: JSONValue]?
            let fallback: JSONValue?
            let providerOverrideFiles: [String]?
            let file: JSONValue?
            let nextActions: [String]?
            let tryLines: [String]?
            let verdict: VerdictBlock?

            enum CodingKeys: String, CodingKey {
                case kind, id, status, action, settings, providers, fallback, file, verdict
                case configPath = "config_path"
                case configExists = "config_exists"
                case providerOverrideFiles = "provider_override_files"
                case nextActions = "next_actions"
                case tryLines = "try_lines"
            }
        }
        guard let raw = try? DeadreckonJSON.decoder().decode(Raw.self, from: data),
              raw.kind == "config", raw.action == "show",
              let configPath = raw.configPath else { return nil }
        kind = raw.kind
        status = raw.status
        self.configPath = configPath
        configExists = raw.configExists ?? false
        settings = raw.settings ?? [:]
        providers = raw.providers ?? [:]
        fallback = raw.fallback
        providerOverrideFiles = raw.providerOverrideFiles ?? []
        file = raw.file
        nextActions = raw.nextActions ?? []
        tryLines = raw.tryLines ?? []
        verdict = raw.verdict
    }

    /// Key state for one route, from the redacted provider entry: true when
    /// an `api_key` slot exists (serialized as the redaction marker). The
    /// UI renders only configured / not configured — never material.
    public func keyConfigured(route: String) -> Bool {
        guard case .object(let entry)? = providers[route] else { return false }
        return entry["api_key"] != nil
    }

    /// Whether the route authenticates through an environment variable
    /// (`api_key_env`) rather than a stored key.
    public func keyFromEnvironment(route: String) -> Bool {
        guard case .object(let entry)? = providers[route] else { return false }
        if case .string? = entry["api_key_env"] { return true }
        return false
    }

    /// The plain display string for a setting's effective value; nil when
    /// the key is absent AND has no pinned built-in default (the binary
    /// serializes those as null: "unset — decided contextually at use time").
    public func displayValue(_ key: String) -> String? {
        guard let setting = settings[key] else { return nil }
        return Self.display(setting.value)
    }

    public static func display(_ value: JSONValue) -> String? {
        switch value {
        case .null: return nil
        case .bool(let flag): return flag ? "true" : "false"
        case .string(let text): return text
        case .number(let number):
            if number == number.rounded(), abs(number) < 1_000_000_000 {
                return String(Int(number))
            }
            return String(number)
        case .array(let items):
            return items.compactMap(display).joined(separator: ", ")
        case .object:
            return nil
        }
    }
}

// MARK: - config set / unset / set-key / unset-key --json (spec §P2/§P3, shipped)

/// One config write acknowledgment. SHIPPED: `kind` is always "config"
/// (not the spec's guessed "config_set" family); the discriminator is
/// `action` ("set" | "unset" | "set-key" | "unset-key"); `id` is the dotted
/// key or the provider route. set-key facts are exactly
/// `{provider, stored: true, keychain_or_file: "file"}` — the envelope
/// NEVER echoes key material, and this decoder has no field that could
/// carry it.
public struct ConfigWriteEnvelope: Equatable, Sendable {
    public let kind: String
    public let id: String?
    public let status: String?
    public let action: String
    public let key: String?
    public let value: JSONValue?
    public let previous: JSONValue?
    public let removed: Bool?
    public let provider: String?
    public let stored: Bool?
    public let keychainOrFile: String?
    public let configPath: String?
    public let nextActions: [String]
    public let tryLines: [String]
    public let verdict: VerdictBlock?

    public init?(data: Data) {
        struct Raw: Codable {
            let kind: String
            let id: String?
            let status: String?
            let action: String?
            let key: String?
            let value: JSONValue?
            let previous: JSONValue?
            let removed: Bool?
            let provider: String?
            let stored: Bool?
            let keychainOrFile: String?
            let configPath: String?
            let nextActions: [String]?
            let tryLines: [String]?
            let verdict: VerdictBlock?

            enum CodingKeys: String, CodingKey {
                case kind, id, status, action, key, value, previous, removed
                case provider, stored, verdict
                case keychainOrFile = "keychain_or_file"
                case configPath = "config_path"
                case nextActions = "next_actions"
                case tryLines = "try_lines"
            }
        }
        guard let raw = try? DeadreckonJSON.decoder().decode(Raw.self, from: data),
              raw.kind == "config",
              let action = raw.action,
              ["set", "unset", "set-key", "unset-key"].contains(action) else { return nil }
        kind = raw.kind
        id = raw.id
        status = raw.status
        self.action = action
        key = raw.key
        value = raw.value
        previous = raw.previous
        removed = raw.removed
        provider = raw.provider
        stored = raw.stored
        keychainOrFile = raw.keychainOrFile
        configPath = raw.configPath
        nextActions = raw.nextActions ?? []
        tryLines = raw.tryLines ?? []
        verdict = raw.verdict
    }
}

// MARK: - supervisor status --json (spec §P4, reconciled with the SHIPPED v4 report)

/// `ServiceRunState` (supervisor_service.rs, snake_case): source one of the
/// two-source health truth — the service manager's account.
public enum ServiceRunWord: String, ForgivingStringEnum, CaseIterable {
    case running
    case stopped
    case notInstalled = "not_installed"
    case unknown
}

/// `HomeCheckpointState`: source two — this home's live instance checkpoint.
public enum HomeCheckpointWord: String, ForgivingStringEnum, CaseIterable {
    case present
    case absent
    case stale
    case unknown
}

/// `SupervisorHealthVerdict`: one honest word over both sources. SHIPPED
/// vocabulary is healthy | degraded | foreign_home | down — NOT the spec's
/// guessed running/stopped/... list; the app types on the shipped words and
/// derives display language in the Lexicon.
public enum ServiceHealthWord: String, ForgivingStringEnum, CaseIterable {
    case healthy
    case degraded
    case foreignHome = "foreign_home"
    case down
    case unknown
}

/// The v4 `SupervisorServiceStatusReport`, decoded fail-closed. SHIPPED:
/// this is a BARE typed document (schema_version discriminated), not a
/// kind-scaffold envelope. v3 reports (older binaries) decode too — the
/// typed two-source fields are simply absent and the verdict derivation
/// degrades honestly. The checkpoint-absent refusal on pre-v4 binaries is
/// prose on stderr (exit 1) and never reaches this decoder.
public struct ServiceStatusReport: Codable, Equatable, Sendable {
    public struct Checkpoint: Codable, Equatable, Sendable {
        public let generation: Int?
        public let instanceID: String?
        public let bootID: String?
        public let pid: Int?
        public let processStartIdentity: String?
        public let startedAt: Date?
        public let binary: String?
        public let deadreckonHome: String?
        public let bundleBuildID: String?
        public let binarySha256: String?

        enum CodingKeys: String, CodingKey {
            case generation, pid, binary
            case instanceID = "instance_id"
            case bootID = "boot_id"
            case processStartIdentity = "process_start_identity"
            case startedAt = "started_at"
            case deadreckonHome = "deadreckon_home"
            case bundleBuildID = "bundle_build_id"
            case binarySha256 = "binary_sha256"
        }
    }

    public let schemaVersion: Int
    public let manager: String
    public let installed: SupervisorInstallState
    public let loaded: Bool?
    public let enabled: String?
    public let active: String?
    /// v4 typed two-source truth; nil on a v3 report.
    public let service: ServiceRunWord?
    public let homeCheckpoint: HomeCheckpointWord?
    public let verdict: ServiceHealthWord?
    /// The most specific verbatim fact behind any verdict other than
    /// healthy. Rendered verbatim, never paraphrased.
    public let verdictReason: String?
    public let checkpoint: Checkpoint?
    public let currentBootID: String?
    public let bootIdentitySource: String?
    public let testOverride: Bool?

    enum CodingKeys: String, CodingKey {
        case manager, installed, loaded, enabled, active, service, verdict, checkpoint
        case schemaVersion = "schema_version"
        case homeCheckpoint = "home_checkpoint"
        case verdictReason = "verdict_reason"
        case currentBootID = "current_boot_id"
        case bootIdentitySource = "boot_identity_source"
        case testOverride = "test_override"
    }

    public init?(data: Data) {
        guard let decoded = try? DeadreckonJSON.decoder().decode(
            ServiceStatusReport.self, from: data) else { return nil }
        self = decoded
    }
}

// MARK: - supervisor install|start|stop --json (spec §P6, shipped)

/// One supervisor lifecycle acknowledgment
/// (`emit_supervisor_lifecycle_success`). SHIPPED: `kind` is "supervisor"
/// with `id`/`action` naming the verb (not the spec's guessed
/// "supervisor_install" family); the plist/unit path field is `unit_path`;
/// `result` is the outcome word (installed | already-installed | updated |
/// started | stopped | already-stopped); `service_state` is a post-action
/// observation and may be "unknown" when the manager was unreadable — the
/// section re-polls `status` before repainting either way.
public struct SupervisorLifecycleEnvelope: Equatable, Sendable {
    public let kind: String
    public let action: String
    public let result: String?
    public let status: String?
    public let serviceState: String?
    public let unitPath: String?
    public let deadreckonHome: String?
    public let binary: String?
    public let nextActions: [String]
    public let tryLines: [String]
    public let verdict: VerdictBlock?

    public init?(data: Data) {
        struct Raw: Codable {
            let kind: String
            let id: String?
            let status: String?
            let action: String?
            let result: String?
            let serviceState: String?
            let unitPath: String?
            let deadreckonHome: String?
            let binary: String?
            let nextActions: [String]?
            let tryLines: [String]?
            let verdict: VerdictBlock?

            enum CodingKeys: String, CodingKey {
                case kind, id, status, action, result, binary, verdict
                case serviceState = "service_state"
                case unitPath = "unit_path"
                case deadreckonHome = "deadreckon_home"
                case nextActions = "next_actions"
                case tryLines = "try_lines"
            }
        }
        guard let raw = try? DeadreckonJSON.decoder().decode(Raw.self, from: data),
              raw.kind == "supervisor",
              let action = raw.action ?? raw.id else { return nil }
        kind = raw.kind
        self.action = action
        result = raw.result
        status = raw.status
        serviceState = raw.serviceState
        unitPath = raw.unitPath
        deadreckonHome = raw.deadreckonHome
        binary = raw.binary
        nextActions = raw.nextActions ?? []
        tryLines = raw.tryLines ?? []
        verdict = raw.verdict
    }
}

// MARK: - doctor --json / doctor --repair --json (spec §P5, shipped)

/// The full doctor document (`doctor_json_payload` + the `repairs` rows
/// `--repair` adds). SHIPPED: there is NO per-finding `repairable` flag —
/// the section-level repair capability derives from
/// `binary_health.repairable_receipt` / `repairable_active_installation`
/// and a failed "supervisor service" finding (§S6's documented fallback).
/// `rawJSON` retains the exact bytes for the raw-report disclosure.
public struct DoctorReportEnvelope: Equatable, Sendable {
    public struct Finding: Codable, Equatable, Sendable {
        public let status: String
        public let subject: String
        public let detail: String
        public let action: String?

        public init(status: String, subject: String, detail: String, action: String? = nil) {
            self.status = status
            self.subject = subject
            self.detail = detail
            self.action = action
        }
    }

    public struct Sandbox: Codable, Equatable, Sendable {
        public let backend: String
        public let available: Bool
        public let path: String?
        public let note: String?
    }

    public struct Installation: Codable, Equatable, Sendable {
        public let canonicalPath: String
        public let locations: [String]
        public let roles: [String]
        public let channel: String
        public let version: String?
        public let sha256: String?
        public let probeError: String?
        public let updateCommand: String?

        enum CodingKeys: String, CodingKey {
            case locations, roles, channel, version, sha256
            case canonicalPath = "canonical_path"
            case probeError = "probe_error"
            case updateCommand = "update_command"
        }
    }

    public struct BinaryHealth: Codable, Equatable, Sendable {
        public let currentPath: String?
        public let currentVersion: String?
        public let pathSelected: String?
        public let installations: [Installation]
        public let conflicts: [String]
        public let advisories: [String]
        public let gateHelperCompatible: Bool?
        public let gateHelperPath: String?
        public let gateProtocolVersion: Int?
        public let bundleBuildID: String?
        public let repairableReceipt: Bool?
        public let repairableActiveInstallation: Bool?

        enum CodingKeys: String, CodingKey {
            case installations, conflicts, advisories
            case currentPath = "current_path"
            case currentVersion = "current_version"
            case pathSelected = "path_selected"
            case gateHelperCompatible = "gate_helper_compatible"
            case gateHelperPath = "gate_helper_path"
            case gateProtocolVersion = "gate_protocol_version"
            case bundleBuildID = "bundle_build_id"
            case repairableReceipt = "repairable_receipt"
            case repairableActiveInstallation = "repairable_active_installation"
        }
    }

    /// One `--repair` outcome row: `{attempted, result, detail}`, in run
    /// order. Repairs gain no authority from the envelope — these are
    /// serialized outcomes only.
    public struct Repair: Codable, Equatable, Sendable {
        public let attempted: String
        public let result: String
        public let detail: String
    }

    public let kind: String
    public let status: String?
    public let configPresent: Bool?
    public let configPath: String?
    public let home: String?
    public let sandboxes: [Sandbox]
    public let binaryHealth: BinaryHealth?
    public let findings: [Finding]
    public let repairs: [Repair]
    public let nextActions: [String]
    public let verdict: VerdictBlock?
    /// The exact document bytes, for the raw-report disclosure (the
    /// evidence floor under every derived row).
    public let rawJSON: String

    public init?(data: Data) {
        struct Raw: Codable {
            let kind: String
            let status: String?
            let configPresent: Bool?
            let configPath: String?
            let home: String?
            let sandboxes: [Sandbox]?
            let binaryHealth: BinaryHealth?
            let findings: [Finding]?
            let repairs: [Repair]?
            let nextActions: [String]?
            let verdict: VerdictBlock?

            enum CodingKeys: String, CodingKey {
                case kind, status, home, sandboxes, findings, repairs, verdict
                case configPresent = "config_present"
                case configPath = "config_path"
                case binaryHealth = "binary_health"
                case nextActions = "next_actions"
            }
        }
        guard let raw = try? DeadreckonJSON.decoder().decode(Raw.self, from: data),
              raw.kind == "doctor" else { return nil }
        kind = raw.kind
        status = raw.status
        configPresent = raw.configPresent
        configPath = raw.configPath
        home = raw.home
        sandboxes = raw.sandboxes ?? []
        binaryHealth = raw.binaryHealth
        findings = raw.findings ?? []
        repairs = raw.repairs ?? []
        nextActions = raw.nextActions ?? []
        verdict = raw.verdict
        rawJSON = String(decoding: data, as: UTF8.self)
    }

    /// §S6 capability probe for the section-level [Repair…]: SHIPPED
    /// signals only — repairable binary-health facts, or a failed
    /// supervisor service finding (doctor's own repair covers exactly
    /// these three).
    public var repairAvailable: Bool {
        if binaryHealth?.repairableReceipt == true { return true }
        if binaryHealth?.repairableActiveInstallation == true { return true }
        return findings.contains { $0.subject == "supervisor service" && $0.status == "failed" }
    }

    public var failedCount: Int { findings.filter { $0.status == "failed" }.count }
    public var warningCount: Int { findings.filter { $0.status == "warning" }.count }
}
