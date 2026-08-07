import DeadreckonKit
import SwiftUI

/// The right EVIDENCE rail (P3/P4): Contract & Checks | Changes | Flight |
/// Docs. Everything here is file-backed or a CLI envelope; narrative prose
/// never appears on this rail (the 2.4.4 trust rule).
struct EvidenceRailView: View {
    enum Tab: String, CaseIterable {
        case contract = "Contract"
        case changes = "Changes"
        case flight = "Flight"
        case docs = "Docs"
    }

    let row: FleetRow
    @ObservedObject var detail: JobDetailStore
    @State private var tab: Tab = .contract

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 2) {
                ForEach(Tab.allCases, id: \.self) { candidate in
                    Button {
                        tab = candidate
                        if candidate == .changes, detail.changes == nil {
                            Task { await detail.refreshChanges() }
                        }
                    } label: {
                        Text(title(candidate))
                            .font(Theme.body(10.5, weight: tab == candidate ? .semibold : .regular))
                            .foregroundStyle(tab == candidate ? Theme.ink : Theme.inkSecondary)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 5)
                            .background(
                                tab == candidate ? Theme.card : .clear,
                                in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    }
                    .buttonStyle(.tactile)
                }
                Spacer()
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            Divider().overlay(Theme.hairline)

            switch tab {
            case .contract: ContractChecksView(row: row, detail: detail)
            case .changes: ChangesView(detail: detail)
            case .flight: FlightView(detail: detail)
            case .docs: DocsView(detail: detail)
            }
        }
        .background(Theme.paper)
    }

    private func title(_ tab: Tab) -> String {
        if tab == .changes, let changes = detail.changes {
            return "Chg \(changes.filesChanged)"
        }
        return tab.rawValue
    }
}

// MARK: - Contract & Checks

/// The frozen acceptance contract rendered check-by-check, crossed with the
/// live acceptance-progress band; when terminal, the receipt-audit facts.
/// VERIFIED language only from the shared proof classifier (trust rule 6);
/// live rows are display data, never evidence (TAILING.md).
struct ContractChecksView: View {
    let row: FleetRow
    @ObservedObject var detail: JobDetailStore

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                contractBand
                liveBand
                if row.projection.phase == .terminal {
                    twoKeysBand
                    receiptAuditBand
                }
                if let issue = detail.reportIssue {
                    Text("report unavailable: \(issue)")
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.warn)
                        .textSelection(.enabled)
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    @ViewBuilder private var contractBand: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Text("acceptance.yaml")
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.ink)
                Text("(frozen)")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.inkTertiary)
            }
            digestChip
            networkLine

            if let contract = detail.report?.contract {
                let rows = contract.checkRows
                if rows.isEmpty {
                    Text("no checks decoded from the frozen spec")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkTertiary)
                }
                ForEach(Array(rows.enumerated()), id: \.offset) { _, check in
                    HStack(alignment: .top, spacing: 6) {
                        Text(check.kind)
                            .font(Theme.mono(10))
                            .foregroundStyle(Theme.ink)
                        if check.mustPass {
                            Text("must_pass")
                                .font(Theme.body(8.5, weight: .semibold))
                                .foregroundStyle(Theme.inkTertiary)
                        }
                        Text(check.subject)
                            .font(Theme.mono(9.5))
                            .foregroundStyle(Theme.inkSecondary)
                            .lineLimit(2)
                            .textSelection(.enabled)
                    }
                }
            } else {
                Text("waiting on report --json for the frozen spec")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.inkTertiary)
            }
        }
    }

    @ViewBuilder private var digestChip: some View {
        if let contract = detail.report?.contract {
            switch contract.matchesApprovedDigest {
            case true?:
                StatusChip(text: "sha matches authority.json", color: Theme.verified)
                    .help("approved \(contract.approvedSHA256 ?? "")")
            case false?:
                StatusChip(text: "DIGEST MISMATCH vs authority.json", color: Theme.danger, filled: true)
                    .help("approved \(contract.approvedSHA256 ?? "-") \u{00B7} current \(contract.currentSHA256 ?? "-")")
            case nil:
                StatusChip(text: "digest \(GlossaryText.unknownState)", color: Theme.inkTertiary)
            }
        }
    }

    @ViewBuilder private var networkLine: some View {
        if let network = detail.status?.job?.job.policy?.execution?.gate?.network {
            Text("network authority: \(network)")
                .font(Theme.mono(10))
                .foregroundStyle(network == "deny" ? Theme.inkSecondary : Theme.warn)
                .help("Contract capability compiled into the immutable gate policy")
        }
    }

    /// The live acceptance-progress band with the restart rule: rows are
    /// scoped to the current gate attempt (the store already discards on
    /// restart). Display data only, never evidence.
    @ViewBuilder private var liveBand: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                Theme.sectionTitle("LIVE CHECKS", size: 9, kerning: 0.5)
                Text("advisory rows \u{00B7} not evidence")
                    .font(Theme.body(9))
                    .foregroundStyle(Theme.inkTertiary)
            }
            if detail.liveChecks.isEmpty {
                Text("no live gate rows (strict gates stream nothing; the file appears whole at sign time)")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.inkTertiary)
            }
            ForEach(Array(detail.liveChecks.enumerated()), id: \.offset) { _, progressRow in
                liveRow(progressRow)
            }
        }
    }

    @ViewBuilder private func liveRow(_ progressRow: AcceptanceProgressRow) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Text(glyph(progressRow))
                .font(Theme.body(10, weight: .bold))
                .foregroundStyle(color(progressRow))
                .frame(width: 12)
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 6) {
                    Text("\(progressRow.index)/\(progressRow.total)")
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.inkTertiary)
                    Text(progressRow.result?.kind ?? progressRow.status)
                        .font(Theme.mono(10))
                        .foregroundStyle(Theme.ink)
                    if let duration = progressRow.result?.durationMS {
                        Text(String(format: "%.1fs", Double(duration) / 1000))
                            .font(Theme.mono(9.5))
                            .foregroundStyle(Theme.inkTertiary)
                    }
                }
                if let result = progressRow.result {
                    CheckResultDetail(result: result)
                }
            }
        }
    }

    private func glyph(_ progressRow: AcceptanceProgressRow) -> String {
        guard let result = progressRow.result else { return "\u{25CC}" }
        return result.passed ? "\u{2713}" : "\u{2717}"
    }

    private func color(_ progressRow: AcceptanceProgressRow) -> Color {
        guard let result = progressRow.result else { return Theme.inkTertiary }
        return result.passed ? Theme.verified : Theme.danger
    }

    /// Two keys (design B1): the deterministic marker and the semantic
    /// judgment. Words come from the report; the VERIFIED chip only from
    /// the rollup's proof classifier.
    @ViewBuilder private var twoKeysBand: some View {
        VStack(alignment: .leading, spacing: 5) {
            Theme.sectionTitle("TWO KEYS", size: 9, kerning: 0.5)

            HStack(spacing: 6) {
                Text("\u{26BF}")
                if let receipt = detail.report?.receipt {
                    Text("marker: \(receipt.status)")
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.ink)
                    if receipt.contained == true {
                        StatusChip(text: "contained", color: Theme.inkSecondary)
                            .help(receipt.sandboxBackend ?? "")
                    }
                } else {
                    Text("marker: \(GlossaryText.unknownState)")
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.inkTertiary)
                }
            }
            if let error = detail.report?.receipt?.signatureValidationError {
                Text(error)
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.danger)
                    .textSelection(.enabled)
            }

            HStack(spacing: 6) {
                Text("\u{2696}")
                if let judgment = detail.report?.semantic?.judgment {
                    Text("judge: \(judgment.decision)")
                        .font(Theme.mono(10.5))
                        .foregroundStyle(judgment.decision == "achieved" ? Theme.ink : Theme.warn)
                } else {
                    Text("judge: pending")
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.inkTertiary)
                }
            }
            if let summary = detail.report?.semantic?.judgment?.summary, !summary.isEmpty {
                // The judge's reason is quoted verbatim, never paraphrased.
                Text("\u{201C}\(summary)\u{201D}")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.inkSecondary)
                    .textSelection(.enabled)
            }

            if row.receipt?.verified == .valid {
                StatusChip(text: GlossaryText.verdictVerified, color: Theme.verifiedFill, filled: true)
                    .help(GlossaryText.phraseVerifiedByDrGate)
            }
            if row.receipt?.verified == .invalid {
                StatusChip(text: GlossaryText.proofWord(.invalid), color: Theme.danger, filled: true)
                    .help(row.receipt?.error ?? "the signed receipt did not validate")
            }
        }
    }

    /// Receipt evidence for terminal jobs, sourced from `report --json`: the
    /// deterministic checks the signed marker recorded, check by check.
    /// Inspection only; the strict fail-closed path in the binary stays the
    /// sole promotion authority.
    ///
    /// Deliberately NOT `verdict --receipt --json`: the committed binary's
    /// `verdict` accepts run references only, a Single-shape job's id
    /// resolves to the Job kind, and the child run is driver-fenced, so a
    /// fresh checks re-run cannot exist for job-owned attempts today. The
    /// Rust-side follow-up (verdict accepting JOB refs) is registered in
    /// CONTRACTS.md; the G7 per-digest audit facts land here with it.
    @ViewBuilder private var receiptAuditBand: some View {
        VStack(alignment: .leading, spacing: 5) {
            Theme.sectionTitle("RECEIPT EVIDENCE", size: 9, kerning: 0.5)

            if let report = detail.report {
                if report.deterministicChecks.isEmpty {
                    Text("no recorded deterministic checks in the report")
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.inkTertiary)
                }
                ForEach(Array(report.deterministicChecks.enumerated()), id: \.offset) { _, check in
                    VStack(alignment: .leading, spacing: 1) {
                        HStack(spacing: 6) {
                            Text(check.passed ? "\u{2713}" : "\u{2717}")
                                .font(Theme.body(10, weight: .bold))
                                .foregroundStyle(check.passed ? Theme.verified : Theme.danger)
                                .frame(width: 12)
                            Text(check.kind)
                                .font(Theme.mono(10))
                                .foregroundStyle(Theme.ink)
                            if let duration = check.durationMS {
                                Text(String(format: "%.1fs", Double(duration) / 1000))
                                    .font(Theme.mono(9.5))
                                    .foregroundStyle(Theme.inkTertiary)
                            }
                        }
                        CheckResultDetail(result: check)
                            .padding(.leading, 18)
                    }
                }
                Text("recorded at gate time, from report --json. A fresh verdict re-run is not available for job-owned attempts in the committed binary (the verdict verb accepts run references only); the Rust-side follow-up is registered in CONTRACTS.md.")
                    .font(Theme.body(9))
                    .foregroundStyle(Theme.inkTertiary)
            } else if let issue = detail.reportIssue {
                Text("report unavailable: \(issue)")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            } else {
                Text("waiting on report --json \u{2026}")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.inkTertiary)
            }
        }
    }
}

/// One check result's detail with expandable clipped output when present.
struct CheckResultDetail: View {
    let result: AcceptanceProgressRow.CheckResult
    @State private var outputShown = false

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            if !result.detail.isEmpty {
                Text(result.detail)
                    .font(Theme.body(9.5))
                    .foregroundStyle(result.passed ? Theme.inkTertiary : Theme.danger)
                    .lineLimit(3)
                    .textSelection(.enabled)
            }
            if hasOutput {
                Button(outputShown ? "hide output" : "show output") {
                    outputShown.toggle()
                }
                .buttonStyle(.plain)
                .font(Theme.body(9, weight: .medium))
                .foregroundStyle(Theme.accent)
                if outputShown {
                    if let stdout = result.stdout, !stdout.isEmpty {
                        clipped(stdout, label: "stdout")
                    }
                    if let stderr = result.stderr, !stderr.isEmpty {
                        clipped(stderr, label: "stderr")
                    }
                }
            }
        }
    }

    private var hasOutput: Bool {
        !(result.stdout ?? "").isEmpty || !(result.stderr ?? "").isEmpty
    }

    private func clipped(_ text: String, label: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text("\(label) (clipped by the gate)")
                .font(Theme.body(8.5))
                .foregroundStyle(Theme.inkTertiary)
            ScrollView(.horizontal) {
                Text(text)
                    .font(Theme.mono(9))
                    .foregroundStyle(Theme.inkSecondary)
                    .textSelection(.enabled)
            }
            .frame(maxHeight: 120)
        }
        .padding(6)
        .background(Theme.card, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
    }
}

// MARK: - Changes

/// Diffstat list from `show --diff --json` (G10); per-file unified patch
/// loaded on demand via `--patch --file` with truncation honesty.
struct ChangesView: View {
    @ObservedObject var detail: JobDetailStore
    @State private var expandedPath: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    if let changes = detail.changes {
                        Text("\u{0394} \(changes.filesChanged) files \u{00B7} +\(changes.added) \u{2212}\(changes.removed)")
                            .font(Theme.body(11, weight: .medium))
                            .foregroundStyle(Theme.ink)
                            .monospacedDigit()
                    }
                    Spacer()
                    Button {
                        Task { await detail.refreshChanges() }
                    } label: {
                        Image(systemName: "arrow.clockwise").font(.system(size: 10))
                    }
                    .buttonStyle(.tactile)
                    .help("Re-run show --diff --json")
                }

                if let issue = detail.changesIssue {
                    Text(issue)
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.warn)
                        .textSelection(.enabled)
                } else if detail.changes == nil {
                    Text("Reading the run diff \u{2026}")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkTertiary)
                } else if detail.changes?.files.isEmpty == true {
                    Text("No source changes recorded.")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkTertiary)
                }

                ForEach(detail.changes?.files ?? [], id: \.path) { file in
                    fileRow(file)
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    @ViewBuilder private func fileRow(_ file: DiffSummaryModel.FileDelta) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Button {
                if expandedPath == file.path {
                    expandedPath = nil
                } else {
                    expandedPath = file.path
                    if detail.patches[file.path] == nil {
                        Task { await detail.loadPatch(path: file.path) }
                    }
                }
            } label: {
                HStack(spacing: 6) {
                    Text(statusGlyph(file.status))
                        .font(Theme.mono(10))
                        .foregroundStyle(statusColor(file.status))
                        .frame(width: 12)
                    Text(file.path)
                        .font(Theme.mono(10))
                        .foregroundStyle(Theme.ink)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    Text("+\(file.added)")
                        .font(Theme.mono(9.5)).foregroundStyle(Theme.verified)
                    Text("\u{2212}\(file.removed)")
                        .font(Theme.mono(9.5)).foregroundStyle(Theme.danger)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if expandedPath == file.path {
                patchBody(path: file.path)
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .cardChrome()
    }

    @ViewBuilder private func patchBody(path: String) -> some View {
        if let patch = detail.patches[path] {
            VStack(alignment: .leading, spacing: 3) {
                if let note = patch.note {
                    Text(note).font(Theme.body(9.5)).foregroundStyle(Theme.inkTertiary)
                }
                if patch.truncated {
                    Text("patch truncated by the binary's byte budget")
                        .font(Theme.body(9.5))
                        .foregroundStyle(Theme.warn)
                }
                ScrollView(.horizontal) {
                    Text(patch.unified.isEmpty ? "(empty patch)" : patch.unified)
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.inkSecondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(6)
            .background(Theme.paper, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        } else if let issue = detail.patchIssues[path] {
            Text(issue).font(Theme.body(9.5)).foregroundStyle(Theme.warn)
        } else {
            Text("loading patch \u{2026}").font(Theme.body(9.5)).foregroundStyle(Theme.inkTertiary)
        }
    }

    private func statusGlyph(_ status: String) -> String {
        switch status {
        case "added": return "A"
        case "removed": return "D"
        case "modified": return "M"
        default: return "?"
        }
    }

    private func statusColor(_ status: String) -> Color {
        switch status {
        case "added": return Theme.verified
        case "removed": return Theme.danger
        case "modified": return Theme.accent
        default: return Theme.warn
        }
    }
}

// MARK: - Flight

/// Flight recorder: checkpoint cards from the manifest tree. PREVIEW facts
/// only — the rewind-apply verb belongs to APP-4 and its affordance says so.
struct FlightView: View {
    @ObservedObject var detail: JobDetailStore

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                if let issue = detail.flightIssue {
                    Text("flight-events tail stopped: \(issue)")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.danger)
                        .textSelection(.enabled)
                }
                if let manifest = detail.flight.manifest {
                    VStack(alignment: .leading, spacing: 3) {
                        Theme.sectionTitle("FLIGHT RECORDER", size: 9, kerning: 0.5)
                        Text("\(detail.flight.eventCount) events this session \u{00B7} \(manifest.sessions.count) sessions")
                            .font(Theme.body(10.5))
                            .foregroundStyle(Theme.inkSecondary)
                            .monospacedDigit()
                        if let last = detail.flight.lastEventSummary {
                            Text(last)
                                .font(Theme.body(10))
                                .foregroundStyle(Theme.inkTertiary)
                                .lineLimit(2)
                        }
                    }
                } else {
                    Text("No flight manifest for this attempt yet.")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkTertiary)
                }

                if detail.flight.checkpoints.isEmpty {
                    Text("No checkpoints captured yet.")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkTertiary)
                }
                ForEach(detail.flight.checkpoints.reversed(), id: \.checkpointID) { checkpoint in
                    checkpointCard(checkpoint)
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func checkpointCard(_ checkpoint: CheckpointManifestDoc) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(checkpoint.checkpointID)
                    .font(Theme.mono(10))
                    .foregroundStyle(Theme.ink)
                if checkpoint.fullAnchor {
                    StatusChip(text: "anchor", color: Theme.accent)
                }
                Spacer()
                Text(ActivityPaneView.time(checkpoint.createdAt))
                    .font(Theme.mono(9.5))
                    .foregroundStyle(Theme.inkTertiary)
            }
            Text("turn \(checkpoint.deadreckonTurn) \u{00B7} \(checkpoint.trigger) \u{00B7} \(checkpoint.fileCount) files")
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.inkSecondary)
            Button("Preview rewind") {}
                .buttonStyle(.plain)
                .font(Theme.body(9.5, weight: .medium))
                .foregroundStyle(Theme.inkTertiary)
                .disabled(true)
                .help("Rewind is not among the M1 machine verbs (no --json envelope yet); it stays CLI-only until the binary grows one. Open in Terminal to rewind.")
        }
        .padding(8)
        .cardChrome()
    }
}

// MARK: - Docs

/// Run docs listing from `<working_dir>/.deadreckon/docs`; an honest empty
/// state otherwise.
struct DocsView: View {
    @ObservedObject var detail: JobDetailStore

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 6) {
                if detail.docs.isEmpty {
                    Text("No run docs found under .deadreckon/docs for this attempt.")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkTertiary)
                }
                ForEach(detail.docs) { doc in
                    Button {
                        NSWorkspace.shared.open(URL(fileURLWithPath: doc.path))
                    } label: {
                        HStack(spacing: 6) {
                            Image(systemName: "doc.text")
                                .font(.system(size: 10))
                                .foregroundStyle(Theme.inkTertiary)
                            Text(doc.name)
                                .font(Theme.mono(10))
                                .foregroundStyle(Theme.ink)
                            Spacer()
                            Text(Self.bytes(doc.bytes))
                                .font(Theme.mono(9))
                                .foregroundStyle(Theme.inkTertiary)
                        }
                        .padding(.horizontal, 8)
                        .padding(.vertical, 6)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .cardChrome()
                    .help("Open in the default editor")
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    static func bytes(_ count: Int) -> String {
        count >= 1024 * 1024 ? String(format: "%.1f MB", Double(count) / 1048576)
            : count >= 1024 ? String(format: "%.0f KB", Double(count) / 1024)
            : "\(count) B"
    }
}
