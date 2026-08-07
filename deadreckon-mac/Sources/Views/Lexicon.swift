import DeadreckonKit
import Foundation

/// Every UI word in the app (REDESIGN-SPEC.md §A, normative). Two
/// vocabularies, never blended: this file owns the plain product language a
/// human reads; on-disk/CLI truth (commands, file names, envelope words,
/// raw status labels, exit codes) stays monospace in technical contexts and
/// is never translated there. `GlossaryText` (Kit) keeps mirroring
/// glossary.rs and is treated as CLI truth that this file translates for
/// display — views call Lexicon for labels and quote GlossaryText/raw
/// values only in mono contexts.
enum Lexicon {
    // MARK: Sections (QueueSection.title stays byte-identical in Kit)

    static func sectionTitle(_ section: QueueSection) -> String {
        switch section {
        case .atTheGate: return "READY TO APPROVE"
        case .needsReview: return "NEEDS YOUR REVIEW"
        case .approaching: return "CHECKING"
        case .underway: return "RUNNING"
        case .wrecked: return "FAILED / STOPPED"
        case .unknown: return "UNREADABLE"
        }
    }

    static func sectionSubtitle(_ section: QueueSection) -> String? {
        switch section {
        case .atTheGate: return "verified; waiting for you"
        case .needsReview: return "stopped for your judgment; nothing verified"
        case .unknown: return "entries the app could not read; the CLI remains authoritative"
        default: return nil
        }
    }

    // MARK: Phase words (GlossaryText.phaseWord stays; this overlays)

    static func phaseWord(_ phase: JobPhase) -> String {
        switch phase {
        case .queued: return "Queued"
        case .running: return "Working"
        case .verifyingChecks: return "Running checks"
        case .verifyingMeaning: return "Judging result"
        case .waiting: return "Waiting"
        case .terminal: return "Finished"
        case .unknown: return "Unknown"
        }
    }

    // MARK: Outcome words

    static func outcomeWord(_ outcome: JobOutcome) -> String {
        switch outcome {
        case .verified: return "Verified"
        case .needsReview: return "Needs your review"
        case .blocked: return "Blocked"
        case .budgetExhausted: return "Budget used up"
        case .deadlineReached: return "Deadline reached"
        case .retryExhausted: return "Out of retries"
        case .cancelled: return "Stopped"
        case .failed: return "Failed"
        case .unknown: return "Unknown"
        }
    }

    /// The display translation of a `job_status_label` word (FleetRow.status).
    /// Mirrors GlossaryText.statusWord's resolution order; anything
    /// unrecognized renders with underscores spaced — machine truth, never
    /// dropped, never re-worded.
    static func statusWord(_ raw: String) -> String {
        if raw == "verified_proof_invalid" { return "Proof invalid" }
        if let outcome = JobOutcome(rawValue: raw), outcome != .unknown {
            return outcomeWord(outcome)
        }
        if let phase = JobPhase(rawValue: raw), phase != .unknown {
            return phaseWord(phase)
        }
        return raw.replacingOccurrences(of: "_", with: " ")
    }

    // MARK: Stop-reason words (shown after "Waiting —")

    static func stopReasonWord(_ reason: StopReason) -> String {
        switch reason {
        case .verified: return "verified"
        case .semanticRevise: return "judge asked for changes"
        case .semanticUncertain: return "judge unsure \u{2014} your call"
        case .semanticUnavailable: return "judge unavailable"
        case .operatorInputRequired: return "needs your input"
        case .spendCap: return "paused at its budget"
        case .wallCap: return "paused at its time limit"
        case .deadline: return "deadline"
        case .attemptLimit: return "attempt limit"
        case .cancelRequested: return "stopping\u{2026}"
        case .deterministicRevise: return "checks asked for changes"
        case .transientProvider: return "agent error (retryable)"
        case .fatalProvider: return "agent error (fatal)"
        case .fatalGate: return "checks error (fatal)"
        case .lostContainment: return "sandbox breach \u{2014} stopped"
        case .supervisorFailure: return "service failure"
        case .corruptHistory: return "history unreadable"
        case .legacyUnknown: return "older run (reason unknown)"
        case .unknown: return "unknown"
        }
    }

    // MARK: Proof words

    /// The proof chip keeps the word "Verified" — strong success chip, only
    /// ever from `receipt.verified == .valid` (trust rule 6). "Proof
    /// invalid" is its own state, never softened (trust rule 5).
    static let proofVerified = "Verified"
    static let proofInvalid = "Proof invalid"

    static func proofWord(_ proof: ProofStatus) -> String {
        switch proof {
        case .valid: return proofVerified
        case .invalid: return proofInvalid
        case .notApplicable: return "no proof expected"
        case .unknown: return "unknown"
        }
    }

    /// Tooltip for the Verified chip; the mono provenance rides along
    /// because tooltips cannot switch face.
    static let verifiedHelp =
        "Verified by the harness gate \u{2014} the signed receipt validated \u{00B7} verified by dr-gate"

    // MARK: Summary line (built from GateQueue counts; views stop using
    // GateQueue.summaryLine — the Kit string stays for tests)

    static func summaryLine(_ queue: GateQueue) -> String {
        var parts: [String] = []
        let gate = queue.items(in: .atTheGate).count
        let review = queue.items(in: .needsReview).count
        let checking = queue.items(in: .approaching).count
        let running = queue.items(in: .underway).count
        let failed = queue.items(in: .wrecked).count
        let unreadable = queue.items(in: .unknown).count
        if gate > 0 { parts.append("\(gate) ready to approve") }
        if review > 0 { parts.append("\(review) \(review == 1 ? "needs" : "need") review") }
        if checking > 0 { parts.append("\(checking) checking") }
        if running > 0 { parts.append("\(running) running") }
        if failed > 0 { parts.append("\(failed) failed") }
        if unreadable > 0 { parts.append("\(unreadable) unreadable") }
        return parts.isEmpty ? "no runs yet" : parts.joined(separator: " \u{00B7} ")
    }

    // MARK: Spine facts (re-formatted from SpineSnapshot's structured
    // fields, never string-parsed — Kit parity with spine.rs is untouched)

    static func lastActivity(_ alive: SpineSnapshot.Aliveness) -> String {
        switch alive {
        // The value sits under the LAST ACTIVITY cell label (§D3 / E2:
        // "5s ago") — repeating the label's words in the value is noise.
        case .live(let age): return "\(age)s ago"
        case .stale(let age): return "Quiet for \(age)s"
        case .dead(let reason): return "Ended \u{2014} \(reason)"
        case .done: return "Done"
        }
    }

    static func onTrackLine(_ onTrack: SpineSnapshot.OnTrack) -> String {
        var parts: [String] = []
        if let ceiling = onTrack.spendCeilingUSD {
            parts.append(String(format: "$%.2f of $%.2f", onTrack.spendUsedUSD, ceiling))
        } else if onTrack.spendUsedUSD > 0 {
            parts.append(String(format: "$%.2f \u{00B7} no budget cap", onTrack.spendUsedUSD))
        } else {
            parts.append("no budget cap")
        }
        if onTrack.wallSecondsUsed > 0 {
            parts.append(hours(onTrack.wallSecondsUsed))
        }
        if let turns = onTrack.turnsUsed {
            parts.append("turn \(turns)")
        }
        return parts.joined(separator: " \u{00B7} ")
    }

    static func attentionLine(_ wrong: [SpineSnapshot.Attention]) -> String {
        guard let first = wrong.first else { return "Nothing flagged" }
        return "\(wrong.count) flagged \u{2014} \(first.summary)"
    }

    /// "$9.12 of $25.00" when a cap exists, "$9.12" otherwise (§9 voice:
    /// numbers are facts). Mirrors GlossaryText.spendLine's resolution; the
    /// Kit string stays for tests.
    static func spendLine(_ spend: FleetRow.Spend) -> String {
        if let cap = spend.capUSD {
            return String(format: "$%.2f of $%.2f", spend.totalCostUSD, cap)
        }
        return String(format: "$%.2f", spend.totalCostUSD)
    }

    static func hours(_ seconds: Double) -> String {
        if seconds >= 3600 { return String(format: "%.1fh", seconds / 3600) }
        if seconds >= 60 { return String(format: "%.0fm", seconds / 60) }
        return String(format: "%.0fs", seconds)
    }

    // MARK: Agents (Provider → Agent). Binary-supplied display_name wins;
    // unknown route ids display verbatim in mono — this returns nil so the
    // caller keeps the id monospace.

    static func agentName(_ id: String, displayName: String? = nil) -> String? {
        if let displayName, !displayName.isEmpty { return displayName }
        switch id.lowercased() {
        case "claude", "claude-code", "anthropic": return "Claude Code"
        case "codex", "openai": return "Codex CLI"
        case "gemini", "google": return "Gemini CLI"
        case "opencode": return "opencode"
        default: return nil
        }
    }

    static func probeWord(_ status: ProviderProbeStatus) -> String {
        switch status {
        case .ok: return "ready"
        case .failed: return "not available"
        case .skipped: return "skipped"
        case .unknown: return "unknown"
        }
    }

    // MARK: Health words (Harbor → System health)

    static func healthWord(_ doctor: FleetStore.HarborState.Doctor) -> String {
        switch doctor {
        case .ok(let warnings) where warnings == 0: return "health OK"
        case .ok(let warnings): return "health: \(warnings) warning\(warnings == 1 ? "" : "s")"
        case .failed(let count): return "health: \(count) failure\(count == 1 ? "" : "s")"
        case .unknown: return "health unknown"
        }
    }

    static func agentsWord(_ providers: FleetStore.HarborState.Providers) -> String {
        switch providers {
        case .counted(let ok, let total): return "agents \(ok)/\(total) ready"
        case .unknown: return "agents unknown"
        }
    }

    static func serviceWord(_ supervisor: FleetStore.HarborState.Supervisor) -> String {
        switch supervisor {
        case .running: return "service running"
        case .stopped(let installState):
            switch installState {
            case .notInstalled: return "service not installed"
            case .stale: return "service outdated"
            case .unsupported: return "service unsupported"
            case .current, .unknown: return "service stopped"
            }
        case .unknown: return "service unknown"
        }
    }

    // MARK: Ledger integrity (drawer chip; Kit labels stay for tests)

    static func integrityWord(_ integrity: JobEventsIntegrity) -> String {
        switch integrity {
        case .none: return "no ledger yet"
        case .contiguous: return "ledger intact"
        case .corrupt: return "LEDGER BROKEN"
        }
    }

    // MARK: Guide (Steer → Guide)

    /// Plain overlays for QuickSteerController.reasonWords' fixed set (the
    /// Kit strings stay for tests/CLI parity); anything unrecognized passes
    /// through verbatim.
    static func guideReason(_ words: String) -> String {
        switch words {
        case "driver fenced": return "this run doesn\u{2019}t take guidance from the app"
        case "not executing": return "the run isn\u{2019}t working right now"
        case "provider not steerable": return "this agent can\u{2019}t take guidance"
        default: return words
        }
    }

    // MARK: Shared chip and meta words

    static let needsYou = "needs you"
    static let unreadableEntry = "unreadable entry"
    static let unknownID = "unknown id"

    static func heartbeatAge(_ seconds: Int) -> String { "\(seconds)s" }

    static func noSignal(_ seconds: Int) -> String { "no signal \(seconds)s" }

    static func notificationsBroken(_ count: Int) -> String {
        "notifications broken for \(count) run\(count == 1 ? "" : "s")"
    }

    static func olderCLIRuns(_ count: Int) -> String {
        "\(count) older CLI run\(count == 1 ? "" : "s")"
    }

    // MARK: Projects (scope → display name)

    /// The plain display name for a project scope. Scopes are source paths
    /// on disk; the folder name is the name a human uses. Non-path scopes
    /// display verbatim. The full scope stays available in tooltips (mono).
    static func projectName(_ scope: String) -> String {
        let trimmed = scope.hasSuffix("/") ? String(scope.dropLast()) : scope
        guard trimmed.contains("/") else { return trimmed.isEmpty ? scope : trimmed }
        let name = (trimmed as NSString).lastPathComponent
        return name.isEmpty ? trimmed : name
    }

    // MARK: Row state words (sidebar / Overview / popover row anatomy)

    /// The one-line plain state under a run's goal (§B1): decision rows say
    /// why they wait; live rows say the phase; finished rows say the
    /// outcome. Built from durable facts only.
    static func rowStateWord(_ item: QueueItem) -> String {
        guard let row = item.row else { return unreadableEntry }
        switch item.section {
        case .atTheGate:
            return "Verified \u{2014} ready to approve"
        case .needsReview:
            return "Stopped for your review"
        case .underway, .approaching:
            if row.projection.phase == .waiting, let reason = row.projection.stopReason {
                return "Waiting \u{2014} \(stopReasonWord(reason))"
            }
            return phaseWord(row.projection.phase)
        case .wrecked:
            return statusWord(row.status)
        case .unknown:
            return unreadableEntry
        }
    }

    // MARK: Relative time (one formatter, one word for "now")

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()

    static func relativeTime(_ date: Date) -> String {
        if Date().timeIntervalSince(date) < 90 { return "now" }
        return relativeFormatter.localizedString(for: date, relativeTo: Date())
    }
}
