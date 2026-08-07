import DeadreckonKit
import SwiftUI

/// The converged Binnacle promote sheet (design A4/B3/C2/C4 per 6.2 item 4):
/// TWO-KEY band, CONTRACT table, receipt chips, CANDIDATE preview
/// (`finish --dry-run --json`, degrading honestly until the M2 binary
/// lands), destination radio mapped 1:1 to flags, the literal finish line,
/// and Quarterdeck's decision bar (Promote / Send back / Kill). Every
/// fail-closed refusal renders verbatim with NO override control — the only
/// recovery affordances are the envelope's try lines and next actions.
struct PromoteSheet: View {
    let row: FleetRow
    /// The live fleet store: the gate must ride the FRESHEST receipt facts,
    /// not the row snapshot captured at sheet-open (a long-lived sheet's
    /// enablement would otherwise derive from facts no longer on disk).
    @ObservedObject var fleet: FleetStore
    /// Routes Send back / Kill to their own confirmation sheets.
    let onSendBack: () -> Void
    let onKill: () -> Void

    @Environment(\.dismiss) private var dismiss
    @StateObject private var coordinator: PromoteCoordinator
    @StateObject private var evidence: JobDetailStore
    @State private var exportPath = ""
    @State private var expandedCheck: Int?

    init(row: FleetRow, fleet: FleetStore, onSendBack: @escaping () -> Void,
         onKill: @escaping () -> Void) {
        self.row = row
        self.fleet = fleet
        self.onSendBack = onSendBack
        self.onKill = onKill
        _coordinator = StateObject(
            wrappedValue: PromoteCoordinator(jobID: row.jobID, cli: WriteCLI.client))
        // The evidence engine: report --json for the two keys and the frozen
        // contract (the APP-3 receipt fallback — see the gate band's label).
        _evidence = StateObject(wrappedValue: JobDetailStore(
            jobID: row.jobID, scope: row.scope, goal: row.goal,
            cli: WriteCLI.client))
    }

    /// The freshest rollup row for this job (falling back to the open-time
    /// snapshot only while the fleet has no loaded row for it).
    private var liveRow: FleetRow {
        fleet.queue.allItems.compactMap(\.row).first(where: { $0.jobID == row.jobID }) ?? row
    }

    private var gate: PromoteGate {
        PromoteGate.evaluate(receipt: liveRow.receipt, report: evidence.report)
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(Theme.hairline)
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    twoKeyBand
                    contractBand
                    receiptBand
                    candidateBand
                    destinationBand
                }
                .padding(18)
            }
            Divider().overlay(Theme.hairline)
            decisionBar
        }
        .frame(width: 680, height: 700)
        .background(Theme.paper)
        .onAppear { evidence.open() }
        .onDisappear { evidence.close() }
        .task { await coordinator.loadPreview() }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Promote \(row.jobID)")
                    .font(Theme.display(18))
                    .foregroundStyle(Theme.ink)
                Text(row.goal)
                    .font(Theme.body(11.5))
                    .foregroundStyle(Theme.inkSecondary)
                    .lineLimit(2)
            }
            Spacer()
            // Trust rule 6: VERIFIED only from the shared proof classifier,
            // read from the LIVE row (never the sheet-open snapshot).
            if liveRow.receipt?.verified == .valid {
                StatusChip(text: GlossaryText.verdictVerified, color: Theme.verified, filled: true)
                    .help(GlossaryText.phraseVerifiedByDrGate)
            } else if liveRow.receipt?.verified == .invalid {
                StatusChip(text: GlossaryText.proofWord(.invalid), color: Theme.danger, filled: true)
                    .help(liveRow.receipt?.error ?? "the signed receipt did not validate")
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
    }

    // MARK: Two keys

    @ViewBuilder private var twoKeyBand: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                sectionTitle("TWO-KEY COMPLETION (Binnacle)")
                Spacer()
                // The honesty label the design requires: these are report's
                // RECORDED facts; a fresh verdict --receipt on a JOB ref is
                // a registered Rust-side gap (CONTRACTS.md).
                Text("recorded by report --json \u{00B7} fresh verdict on JOB refs is a registered Rust gap")
                    .font(Theme.body(9))
                    .foregroundStyle(Theme.inkTertiary)
            }
            keyLine(
                glyph: "\u{26BF}",
                title: "Key 1 \u{00B7} deterministic marker",
                present: gate.markerKeyPresent,
                detail: markerDetail)
            keyLine(
                glyph: "\u{2696}",
                title: "Key 2 \u{00B7} semantic judgment",
                present: gate.judgmentKeyPresent,
                detail: judgmentDetail)
            if let judgment = evidence.report?.semantic?.judgment, let summary = judgment.summary {
                Text("\u{201C}\(summary)\u{201D}")
                    .font(Theme.body(11))
                    .italic()
                    .foregroundStyle(Theme.inkSecondary)
                    .textSelection(.enabled)
                    .padding(.leading, 22)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    private var markerDetail: String {
        guard let receipt = evidence.report?.receipt else {
            return "no receipt block recorded"
        }
        var parts = ["status \(receipt.status)"]
        if receipt.contained == true { parts.append("contained") }
        if let backend = receipt.sandboxBackend { parts.append(backend) }
        if let error = receipt.signatureValidationError { parts.append("signature: \(error)") }
        return parts.joined(separator: " \u{00B7} ")
    }

    private var judgmentDetail: String {
        guard let judgment = evidence.report?.semantic?.judgment else {
            return "no judgment recorded"
        }
        return judgment.decision
    }

    private func keyLine(glyph: String, title: String, present: Bool, detail: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Text(glyph)
                .font(Theme.body(12))
                .foregroundStyle(present ? Theme.verified : Theme.warn)
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(Theme.body(11, weight: .semibold))
                    .foregroundStyle(Theme.ink)
                Text(detail)
                    .font(Theme.body(10.5))
                    .foregroundStyle(present ? Theme.inkSecondary : Theme.warn)
                    .textSelection(.enabled)
            }
        }
    }

    // MARK: Contract table

    @ViewBuilder private var contractBand: some View {
        VStack(alignment: .leading, spacing: 6) {
            sectionTitle("CONTRACT \u{00B7} frozen acceptance.yaml")
            if let contract = evidence.report?.contract {
                HStack(spacing: 6) {
                    if let approved = contract.approvedSHA256 {
                        Text("sha256 \(String(approved.prefix(12)))\u{2026}")
                            .font(Theme.mono(10))
                            .foregroundStyle(Theme.inkSecondary)
                    }
                    if let matches = contract.matchesApprovedDigest {
                        StatusChip(
                            text: matches ? "matches authority.json" : "DIGEST MISMATCH",
                            color: matches ? Theme.verified : Theme.danger,
                            filled: !matches)
                    }
                    if let network = evidence.status?.job?.job.policy?.execution?.gate?.network {
                        StatusChip(text: "net: \(network)", color: Theme.inkSecondary)
                    }
                }
                checkTable(contract)
            } else if let issue = evidence.reportIssue {
                Text("report unavailable: \(issue)")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            } else {
                ProgressView().controlSize(.small)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    /// Frozen checks crossed with the recorded per-check results (report's
    /// deterministic_checks): status, duration, expandable clipped output
    /// where the ledger recorded any. Pairing is positional BUT verified on
    /// check identity (kind): a recorded list that is not a same-order
    /// mirror of the frozen contract (reordered, or from another contract
    /// revision) degrades that row to the unpaired "not recorded" glyph
    /// rather than showing a result against the wrong check.
    @ViewBuilder private func checkTable(_ contract: JobReportEnvelope.Contract) -> some View {
        let rows = contract.checkRows
        let results = evidence.report?.deterministicChecks ?? []
        ForEach(Array(rows.enumerated()), id: \.offset) { index, check in
            let result: AcceptanceProgressRow.CheckResult? =
                (index < results.count && results[index].kind == check.kind)
                ? results[index] : nil
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(result == nil ? "\u{25CB}" : (result!.passed ? "\u{2713}" : "\u{2717}"))
                        .font(Theme.body(11, weight: .bold))
                        .foregroundStyle(result == nil ? Theme.inkTertiary
                            : (result!.passed ? Theme.verified : Theme.danger))
                    Text(check.kind)
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.ink)
                    Text(check.subject)
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkSecondary)
                        .lineLimit(1)
                    if check.mustPass {
                        StatusChip(text: "must pass", color: Theme.inkSecondary)
                    }
                    Spacer()
                    if let duration = result?.durationMS {
                        Text(String(format: "%.1fs", Double(duration) / 1000))
                            .font(Theme.mono(10))
                            .foregroundStyle(Theme.inkTertiary)
                    }
                    if result?.stdout != nil || result?.stderr != nil {
                        Button(expandedCheck == index ? "hide output" : "output \u{25B8}") {
                            expandedCheck = expandedCheck == index ? nil : index
                        }
                        .buttonStyle(.tactile)
                        .font(Theme.body(9.5))
                        .foregroundStyle(Theme.accent)
                    }
                }
                if expandedCheck == index, let result {
                    VStack(alignment: .leading, spacing: 2) {
                        if let stdout = result.stdout, !stdout.isEmpty {
                            Text(stdout)
                                .font(Theme.mono(9.5))
                                .foregroundStyle(Theme.inkSecondary)
                                .textSelection(.enabled)
                                .lineLimit(14)
                        }
                        if let stderr = result.stderr, !stderr.isEmpty {
                            Text(stderr)
                                .font(Theme.mono(9.5))
                                .foregroundStyle(Theme.warn)
                                .textSelection(.enabled)
                                .lineLimit(8)
                        }
                        Text("clipped \u{2014} recorded by the gate, not re-run here")
                            .font(Theme.body(9))
                            .foregroundStyle(Theme.inkTertiary)
                    }
                    .padding(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.paper, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                }
            }
        }
    }

    // MARK: Receipt chips

    @ViewBuilder private var receiptBand: some View {
        VStack(alignment: .leading, spacing: 6) {
            sectionTitle("RECEIPT")
            HStack(spacing: 6) {
                if let receipt = liveRow.receipt {
                    StatusChip(
                        text: "proof \(GlossaryText.proofWord(receipt.verified))",
                        color: receipt.verified == .valid ? Theme.verified
                            : receipt.verified == .invalid ? Theme.danger : Theme.inkTertiary)
                    if let error = receipt.error {
                        Text(error)
                            .font(Theme.body(10))
                            .foregroundStyle(Theme.danger)
                            .textSelection(.enabled)
                    }
                } else {
                    Text("no receipt on the rollup row")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.warn)
                }
                if let gateCounts = liveRow.gate {
                    StatusChip(text: GlossaryText.gateCounts(gateCounts), color: Theme.inkSecondary)
                        .help("From the signed acceptance marker, attempt \(gateCounts.attempt)")
                }
            }
            Text("real finish re-validates the receipt fail-closed before AND after the atomic rename; any drift refuses with no operator override, by design")
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.inkTertiary)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    // MARK: Candidate (dry-run preview)

    @ViewBuilder private var candidateBand: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                sectionTitle("CANDIDATE \u{00B7} preview before mutate")
                Spacer()
                Button("Refresh preview") {
                    Task { await coordinator.loadPreview() }
                }
                .buttonStyle(.tactile)
                .font(Theme.body(10))
                .foregroundStyle(Theme.accent)
                .disabled(exportPathMissing)
                .help(exportPathMissing
                    ? "Enter an export destination first \u{2014} there is no default path."
                    : "Re-runs finish --dry-run --json for the selected destination.")
            }
            CommandLineView(command: coordinator.dryRunCommandLine)
            if let previewedFor = coordinator.previewDestination,
               previewedFor != coordinator.destination {
                // The shown plan was computed for a DIFFERENT destination:
                // say so instead of silently pairing it with the new flags.
                Text("this preview was computed for a different destination \u{2014} Refresh preview before trusting it")
                    .font(Theme.body(10, weight: .medium))
                    .foregroundStyle(Theme.warn)
            }
            switch coordinator.preview {
            case .idle, .loading:
                ProgressView().controlSize(.small)
            case .unsupported(let words):
                VStack(alignment: .leading, spacing: 3) {
                    Text("promote preview requires the M2 binary")
                        .font(Theme.body(11, weight: .semibold))
                        .foregroundStyle(Theme.warn)
                    Text(words)
                        .font(Theme.mono(10))
                        .foregroundStyle(Theme.inkSecondary)
                        .textSelection(.enabled)
                    Text("PROMOTE below still runs the real fail-closed finish; only the staged-file preview is missing.")
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.inkTertiary)
                }
            case .refused(let refusal):
                RefusalView(refusal: refusal)
            case .failed(let words):
                Text(words)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            case .plan(let plan):
                if plan.isBlocked {
                    blockedPlanView(plan)
                } else {
                    planView(plan)
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    /// A blocked finish_plan (receipt tamper / digest mismatch / staging
    /// refusal — exit 0, plan on stdout, status "blocked"): rendered as the
    /// fail-closed refusal it is, quoting receipt.error verbatim. Never
    /// rendered as a normal plan — "0 files · +0 −0" would be a lie.
    @ViewBuilder private func blockedPlanView(_ plan: FinishPlanEnvelope) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "xmark.octagon")
                    .foregroundStyle(Theme.danger)
                Text("finish plan blocked \u{2014} nothing will promote")
                    .font(Theme.body(11.5, weight: .semibold))
                    .foregroundStyle(Theme.danger)
            }
            if let error = plan.receipt?.error {
                Text(error)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.danger)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                Text("the binary reported status \u{201C}blocked\u{201D} without a receipt error message")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.danger)
            }
            ForEach(plan.nextActions, id: \.self) { action in
                Text("next: \(action)")
                    .font(Theme.mono(10))
                    .foregroundStyle(Theme.accent)
                    .textSelection(.enabled)
            }
            Text("real finish would refuse the same way; there is no override")
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.inkTertiary)
        }
    }

    @ViewBuilder private func planView(_ plan: FinishPlanEnvelope) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            if let diffstat = plan.diffstat {
                Text("\(diffstat.filesChanged ?? plan.staged.count) files \u{00B7} +\(diffstat.added ?? 0) \u{2212}\(diffstat.removed ?? 0)")
                    .font(Theme.body(11, weight: .medium))
                    .foregroundStyle(Theme.ink)
            }
            ForEach(Array(plan.staged.prefix(40).enumerated()), id: \.offset) { _, file in
                HStack(spacing: 8) {
                    Text(file.path)
                        .font(Theme.mono(10))
                        .foregroundStyle(Theme.ink)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    Text("\(file.bytes) B")
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.inkTertiary)
                    Text(String(file.sha256.prefix(8)))
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.inkTertiary)
                }
            }
            if plan.staged.count > 40 {
                Text("\u{2026} \(plan.staged.count - 40) more staged files")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.inkTertiary)
            }
            if !plan.irreversibleSteps.isEmpty {
                HStack(spacing: 6) {
                    Text("IRREVERSIBLE:")
                        .font(Theme.body(10, weight: .bold))
                        .foregroundStyle(Theme.danger)
                    Text(plan.irreversibleSteps.joined(separator: ", "))
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.danger)
                }
            }
            Text("report-only: real finish re-validates and re-stages from scratch")
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.inkTertiary)
        }
    }

    // MARK: Destination

    @ViewBuilder private var destinationBand: some View {
        VStack(alignment: .leading, spacing: 6) {
            sectionTitle("DESTINATION")
            destinationRadio(
                selected: isApply,
                title: "Apply to the working tree",
                subtitle: "undoable afterwards: deadreckon undo"
            ) {
                coordinator.destination = .apply(autostash: applyAutostash, cleanup: applyCleanup)
            }
            if isApply {
                HStack(spacing: 14) {
                    Toggle("--autostash", isOn: autostashBinding).toggleStyle(.checkbox)
                    Toggle("--cleanup", isOn: cleanupBinding).toggleStyle(.checkbox)
                }
                .font(Theme.mono(10.5))
                .padding(.leading, 22)
            }
            destinationRadio(
                selected: !isApply,
                title: "Export to a directory (--dest)",
                subtitle: nil
            ) {
                // No invented default: the operator names the destination or
                // PROMOTE stays disabled with the missing fact named below.
                coordinator.destination = .export(path: exportPath)
            }
            if !isApply {
                TextField("/path/to/export", text: $exportPath)
                    .textFieldStyle(.plain)
                    .font(Theme.mono(11))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .frame(maxWidth: 320)
                    .background(Theme.card, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .strokeBorder(Theme.hairline, lineWidth: 1))
                    .padding(.leading, 22)
                    .onChange(of: exportPath) { _, path in
                        coordinator.destination = .export(path: path)
                    }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    private var isApply: Bool {
        if case .apply = coordinator.destination { return true }
        return false
    }

    /// Export selected but no destination typed yet: PROMOTE and the preview
    /// stay disabled with this fact named (no invented default path).
    private var exportPathMissing: Bool {
        !isApply && exportPath.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var applyAutostash: Bool {
        if case .apply(let autostash, _) = coordinator.destination { return autostash }
        return true
    }

    private var applyCleanup: Bool {
        if case .apply(_, let cleanup) = coordinator.destination { return cleanup }
        return false
    }

    private var autostashBinding: Binding<Bool> {
        Binding(get: { applyAutostash },
                set: { coordinator.destination = .apply(autostash: $0, cleanup: applyCleanup) })
    }

    private var cleanupBinding: Binding<Bool> {
        Binding(get: { applyCleanup },
                set: { coordinator.destination = .apply(autostash: applyAutostash, cleanup: $0) })
    }

    private func destinationRadio(selected: Bool, title: String, subtitle: String?,
                                  action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: selected ? "largecircle.fill.circle" : "circle")
                    .foregroundStyle(selected ? Theme.accent : Theme.inkTertiary)
                    .font(.system(size: 12))
                Text(title)
                    .font(Theme.body(11.5, weight: .medium))
                    .foregroundStyle(Theme.ink)
                if let subtitle {
                    Text(subtitle)
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.inkTertiary)
                }
                Spacer()
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.tactile)
    }

    // MARK: Decision bar

    @ViewBuilder private var decisionBar: some View {
        VStack(alignment: .leading, spacing: 8) {
            switch coordinator.promotion {
            case .idle:
                EmptyView()
            case .running:
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("running finish \u{2014} validate, stage, revalidate, rename, revalidate\u{2026}")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkSecondary)
                }
            case .succeeded(let envelope):
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Image(systemName: "checkmark.seal")
                            .foregroundStyle(Theme.verified)
                        Text("promoted \u{00B7} \(envelope.status ?? "completed")"
                            + (envelope.delivery?.stagedFileCount.map { " \u{00B7} \($0) files" } ?? ""))
                            .font(Theme.body(11.5, weight: .medium))
                            .foregroundStyle(Theme.ink)
                    }
                    Text("one-command rollback: deadreckon undo \u{00B7} the row updates from the files, not from this sheet")
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.inkTertiary)
                }
            case .refused(let refusal):
                RefusalView(refusal: refusal)
            case .failed(let words):
                Text(words)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            }

            CommandLineView(command: coordinator.finishCommandLine)

            HStack(spacing: 8) {
                Button("Close") { dismiss() }
                    .buttonStyle(.tactile)
                    .keyboardShortcut(.cancelAction)
                Spacer()
                // No dismiss() first: the router's .sheet(item:) swaps the
                // presented content on identity change. dismiss()-then-set
                // in the same tick can race the dismissal animation and
                // swallow the follow-up sheet on some macOS versions.
                Button("Send back + note\u{2026}") {
                    onSendBack()
                }
                .buttonStyle(.tactile)
                .font(Theme.body(11, weight: .medium))
                Button("Kill\u{2026}") {
                    onKill()
                }
                .buttonStyle(.tactile)
                .font(Theme.body(11, weight: .medium))
                .foregroundStyle(Theme.danger)
                promoteButton
            }
            if let reason = promoteDisabledReason {
                Text("promote disabled: \(reason)")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.warn)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
    }

    private var promoteButton: some View {
        Button {
            Task { await coordinator.promote(gate: gate) }
        } label: {
            Text("PROMOTE \u{2014} finish \(row.jobID)")
                .font(Theme.body(12, weight: .semibold))
                .foregroundStyle(Theme.onFill)
                .padding(.horizontal, 14)
                .padding(.vertical, 7)
                .background(promoteEnabled ? Theme.verified : Theme.inkTertiary, in: Capsule())
        }
        .buttonStyle(.tactile)
        .disabled(!promoteEnabled)
        .keyboardShortcut(.defaultAction)
    }

    private var promoteEnabled: Bool {
        if case .running = coordinator.promotion { return false }
        if case .succeeded = coordinator.promotion { return false }
        if exportPathMissing { return false }
        return gate.promoteEnabled
    }

    /// The first missing fact, named: the gate's own reason, or the export
    /// destination the operator has not typed yet.
    private var promoteDisabledReason: String? {
        if let reason = gate.disabledReason { return reason }
        if exportPathMissing { return "no export destination entered (--dest has no default)" }
        return nil
    }

    private func sectionTitle(_ text: String) -> some View {
        Text(text)
            .font(Theme.body(10, weight: .bold))
            .kerning(0.6)
            .foregroundStyle(Theme.inkTertiary)
    }
}

/// The send-back sheet (Quarterdeck's middle button, G9): a follow-up goal
/// plus the operator's note, recorded as typed provenance on the parent run.
/// Not promoted, not killed — sent back with a receipt of why.
struct SendBackSheet: View {
    let row: FleetRow
    @Environment(\.dismiss) private var dismiss
    @StateObject private var coordinator: SendBackCoordinator

    init(row: FleetRow) {
        self.row = row
        _coordinator = StateObject(
            wrappedValue: SendBackCoordinator(parentID: row.jobID, cli: WriteCLI.client))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Send back \(row.jobID)")
                    .font(Theme.display(18))
                    .foregroundStyle(Theme.ink)
                Text("queues a continuation Job under the parent's frozen contract; your note lands as typed provenance the next agentic turn can read")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.inkTertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            VStack(alignment: .leading, spacing: 4) {
                Text("FOLLOW-UP GOAL")
                    .font(Theme.body(10, weight: .bold))
                    .foregroundStyle(Theme.inkTertiary)
                TextEditor(text: $coordinator.goal)
                    .font(Theme.body(12))
                    .scrollContentBackground(.hidden)
                    .padding(6)
                    .frame(height: 56)
                    .background(Theme.card, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .strokeBorder(Theme.hairline, lineWidth: 1))
            }

            VStack(alignment: .leading, spacing: 4) {
                Text("OPERATOR NOTE (--note, recorded on the parent)")
                    .font(Theme.body(10, weight: .bold))
                    .foregroundStyle(Theme.inkTertiary)
                TextEditor(text: $coordinator.note)
                    .font(Theme.body(12))
                    .scrollContentBackground(.hidden)
                    .padding(6)
                    .frame(height: 72)
                    .background(Theme.card, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .strokeBorder(Theme.hairline, lineWidth: 1))
            }

            CommandLineView(command: coordinator.commandLine)

            switch coordinator.state {
            case .idle:
                EmptyView()
            case .running:
                ProgressView().controlSize(.small)
            case .queued(let envelope):
                HStack(spacing: 6) {
                    Image(systemName: "checkmark.circle")
                        .foregroundStyle(Theme.verified)
                    Text("queued \(envelope.id ?? "")"
                        + " \u{00B7} contract \(envelope.extend?.contract ?? "?")"
                        + " \u{00B7} note \(envelope.extend?.noteRecorded == true ? "recorded" : "not recorded")")
                        .font(Theme.body(11))
                        .foregroundStyle(Theme.inkSecondary)
                        .textSelection(.enabled)
                }
            case .refused(let refusal):
                RefusalView(refusal: refusal)
            case .failed(let words):
                VStack(alignment: .leading, spacing: 6) {
                    Text(words)
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.warn)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    if coordinator.mayHaveQueued {
                        // The explicit fresh confirmation after an ambiguous
                        // failure: the operator re-arms only after checking
                        // the fleet (which updates from the files).
                        Button("I checked the fleet \u{2014} re-arm Send back") {
                            coordinator.rearmAfterPossibleQueue()
                        }
                        .buttonStyle(.tactile)
                        .font(Theme.body(10.5, weight: .medium))
                        .foregroundStyle(Theme.accent)
                    }
                }
            }

            HStack {
                Spacer()
                Button("Close") { dismiss() }
                    .buttonStyle(.tactile)
                    .keyboardShortcut(.cancelAction)
                Button {
                    Task { await coordinator.submit() }
                } label: {
                    Text("Send back")
                        .font(Theme.body(12, weight: .semibold))
                        .foregroundStyle(Theme.onFill)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 6)
                        .background(coordinator.canSubmit ? Theme.accent : Theme.inkTertiary,
                                    in: Capsule())
                }
                .buttonStyle(.tactile)
                .disabled(!coordinator.canSubmit)
            }
        }
        .padding(20)
        .frame(width: 560)
        .background(Theme.paper)
    }
}
