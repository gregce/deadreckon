import DeadreckonKit
import SwiftUI

/// The Review & Approve sheet: TWO SIGN-OFFS band, WHAT DONE MEANS table,
/// PROOF chips, RESULT PREVIEW (`finish --dry-run --json`, degrading
/// honestly until the M2 binary lands), destination radio mapped 1:1 to
/// flags, the literal finish line, and the decision bar (Approve / Send
/// back / Stop). Every fail-closed refusal renders verbatim with NO
/// override control — the only recovery affordances are the envelope's try
/// lines and next actions.
struct ReviewApproveSheet: View {
    let row: FleetRow
    /// The live fleet store: the gate must ride the FRESHEST receipt facts,
    /// not the row snapshot captured at sheet-open (a long-lived sheet's
    /// enablement would otherwise derive from facts no longer on disk).
    @ObservedObject var fleet: FleetStore
    /// Routes Send back / Stop to their own confirmation sheets.
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
        // The evidence engine: report --json for the two sign-offs and the
        // frozen definition (the APP-3 receipt fallback — see the gate
        // band's label).
        _evidence = StateObject(wrappedValue: JobDetailStore(
            jobID: row.jobID, scope: row.scope, goal: row.goal,
            cli: WriteCLI.client))
    }

    /// The freshest rollup row for this run (falling back to the open-time
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
            Divider().overlay(Theme.border)
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    twoKeyBand
                    contractBand
                    receiptBand
                    candidateBand
                    destinationBand
                }
                .padding(20)
            }
            Divider().overlay(Theme.border)
            decisionBar
        }
        .frame(width: 680, height: 700)
        .background(Theme.windowBg)
        .onAppear { evidence.open() }
        .onDisappear { evidence.close() }
        .task { await coordinator.loadPreview() }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Review & Approve")
                    .font(Theme.title)
                    .foregroundStyle(Theme.textPrimary)
                Text(row.goal)
                    .font(Theme.body(11.5))
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(2)
                Text(row.jobID)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textTertiary)
                    .textSelection(.enabled)
            }
            Spacer()
            // Trust rule 6: Verified only from the shared proof classifier,
            // read from the LIVE row (never the sheet-open snapshot).
            if liveRow.receipt?.verified == .valid {
                StatusChip(text: Lexicon.proofVerified, color: Theme.success, strong: true)
                    .help(Lexicon.verifiedHelp)
            } else if liveRow.receipt?.verified == .invalid {
                StatusChip(text: Lexicon.proofInvalid, color: Theme.danger, strong: true,
                           textColor: Theme.dangerText)
                    .help(liveRow.receipt?.error ?? "the signed receipt did not validate")
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    // MARK: Two sign-offs

    @ViewBuilder private var twoKeyBand: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Theme.sectionTitle("TWO SIGN-OFFS")
                Spacer()
                // The honesty label the design requires: these are report's
                // RECORDED facts; a fresh re-run for a run is a registered
                // CLI gap (CONTRACTS.md).
                Text("from the recorded report \u{2014} the app can\u{2019}t re-run checks (registered CLI gap)")
                    .font(Theme.body(9))
                    .foregroundStyle(Theme.textTertiary)
            }
            keyLine(
                title: "Checks passed \u{2014} signed record",
                present: gate.markerKeyPresent,
                detail: markerDetail)
            keyLine(
                title: "Judge\u{2019}s call",
                present: gate.judgmentKeyPresent,
                detail: judgmentDetail)
            if let judgment = evidence.report?.semantic?.judgment, let summary = judgment.summary {
                Text("\u{201C}\(summary)\u{201D}")
                    .font(Theme.small)
                    .italic()
                    .foregroundStyle(Theme.textSecondary)
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
            return "no signed record found"
        }
        var parts = ["status \(receipt.status)"]
        if receipt.contained == true { parts.append("sandboxed") }
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

    private func keyLine(title: String, present: Bool, detail: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Text(present ? "\u{2713}" : "\u{25CB}")
                .font(Theme.body(12, weight: .bold))
                .foregroundStyle(present ? Theme.success : Theme.warn)
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(Theme.body(11, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                Text(detail)
                    .font(Theme.body(10.5))
                    .foregroundStyle(present ? Theme.textSecondary : Theme.warn)
                    .textSelection(.enabled)
            }
        }
    }

    // MARK: What done means

    @ViewBuilder private var contractBand: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Theme.sectionTitle("WHAT DONE MEANS")
                Text("frozen acceptance.yaml")
                    .font(Theme.monoS)
                    .foregroundStyle(Theme.textTertiary)
            }
            if let contract = evidence.report?.contract {
                HStack(spacing: 6) {
                    if let approved = contract.approvedSHA256 {
                        Text("sha256 \(String(approved.prefix(12)))\u{2026}")
                            .font(Theme.monoS)
                            .foregroundStyle(Theme.textSecondary)
                    }
                    if let matches = contract.matchesApprovedDigest {
                        if matches {
                            StatusChip(text: "matches the approved version", color: Theme.success)
                        } else {
                            StatusChip(text: "CHANGED SINCE APPROVAL", color: Theme.danger,
                                       strong: true, textColor: Theme.dangerText)
                                .help("sha256 approved \(contract.approvedSHA256 ?? "-") \u{00B7} current \(contract.currentSHA256 ?? "-")")
                        }
                    }
                    if let network = evidence.status?.job?.job.policy?.execution?.gate?.network {
                        StatusChip(text: network == "deny" ? "network: not allowed" : "network: \(network)",
                                   color: Theme.textSecondary)
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
    /// mirror of the frozen definition (reordered, or from another
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
                        .foregroundStyle(result == nil ? Theme.textTertiary
                            : (result!.passed ? Theme.success : Theme.dangerText))
                        .help(result == nil ? "not recorded" : "")
                    Text(check.kind)
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.textPrimary)
                    Text(check.subject)
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)
                    if check.mustPass {
                        StatusChip(text: "must pass", color: Theme.textSecondary)
                    }
                    Spacer()
                    if let duration = result?.durationMS {
                        Text(String(format: "%.1fs", Double(duration) / 1000))
                            .font(Theme.monoS)
                            .foregroundStyle(Theme.textTertiary)
                    }
                    if result?.stdout != nil || result?.stderr != nil {
                        Button(expandedCheck == index ? "hide output" : "output \u{25B8}") {
                            expandedCheck = expandedCheck == index ? nil : index
                        }
                        .buttonStyle(.themeText(size: 9.5))
                    }
                }
                if expandedCheck == index, let result {
                    VStack(alignment: .leading, spacing: 2) {
                        if let stdout = result.stdout, !stdout.isEmpty {
                            Text(stdout)
                                .font(Theme.mono(9.5))
                                .foregroundStyle(Theme.textSecondary)
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
                        Text("clipped \u{2014} recorded when the checks ran, not re-run here")
                            .font(Theme.body(9))
                            .foregroundStyle(Theme.textTertiary)
                    }
                    .padding(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.well,
                                in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .strokeBorder(Theme.border, lineWidth: 1))
                }
            }
        }
    }

    // MARK: Proof

    @ViewBuilder private var receiptBand: some View {
        VStack(alignment: .leading, spacing: 6) {
            Theme.sectionTitle("PROOF")
            HStack(spacing: 6) {
                if let receipt = liveRow.receipt {
                    StatusChip(
                        text: Lexicon.proofWord(receipt.verified),
                        color: receipt.verified == .valid ? Theme.success
                            : receipt.verified == .invalid ? Theme.danger : Theme.textTertiary,
                        strong: receipt.verified == .valid || receipt.verified == .invalid,
                        textColor: receipt.verified == .invalid ? Theme.dangerText : nil)
                    if let error = receipt.error {
                        Text(error)
                            .font(Theme.monoS)
                            .foregroundStyle(Theme.dangerText)
                            .textSelection(.enabled)
                    }
                } else {
                    Text("no proof recorded for this run")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.warn)
                }
                if let gateCounts = liveRow.gate {
                    StatusChip(text: GlossaryText.gateCounts(gateCounts), color: Theme.textSecondary)
                        .help("From the signed record of check results, attempt \(gateCounts.attempt)")
                }
            }
            Text("Approving re-validates the proof before and after the files move; any drift refuses \u{2014} there is no override, by design.")
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.textTertiary)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    // MARK: Result preview (dry-run)

    @ViewBuilder private var candidateBand: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Theme.sectionTitle("RESULT PREVIEW \u{2014} NOTHING MOVES YET")
                Spacer()
                Button("Refresh preview") {
                    Task { await coordinator.loadPreview() }
                }
                .buttonStyle(.themeStandard(compact: true))
                .disabled(exportPathMissing)
                .help(exportPathMissing
                    ? "Enter an export folder first \u{2014} there is no default."
                    : "Re-runs finish --dry-run --json for the selected destination.")
            }
            CommandLineView(command: coordinator.dryRunCommandLine)
            if let previewedFor = coordinator.previewDestination,
               previewedFor != coordinator.destination {
                // The shown plan was computed for a DIFFERENT destination:
                // say so instead of silently pairing it with the new flags.
                Text("This preview was computed for a different destination \u{2014} refresh before trusting it.")
                    .font(Theme.body(10, weight: .medium))
                    .foregroundStyle(Theme.warn)
            }
            switch coordinator.preview {
            case .idle, .loading:
                ProgressView().controlSize(.small)
            case .unsupported(let words):
                VStack(alignment: .leading, spacing: 3) {
                    Text("Result preview needs a newer CLI.")
                        .font(Theme.body(11, weight: .semibold))
                        .foregroundStyle(Theme.warn)
                    Text(words)
                        .font(Theme.monoS)
                        .foregroundStyle(Theme.textSecondary)
                        .textSelection(.enabled)
                    Text("Approve below still runs the real fail-closed apply; only this file list is missing.")
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textTertiary)
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
            Text("Blocked \u{2014} nothing will be applied")
                .font(Theme.body(11.5, weight: .semibold))
                .foregroundStyle(Theme.dangerText)
            if let error = plan.receipt?.error {
                Text(error)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textPrimary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                Text("The CLI reported \u{201C}blocked\u{201D} without an error message.")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.dangerText)
            }
            ForEach(plan.nextActions, id: \.self) { action in
                Text("next: \(action)")
                    .font(Theme.monoS)
                    .foregroundStyle(Theme.accent)
                    .textSelection(.enabled)
            }
            Text("Approving would refuse the same way; there is no override.")
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.textTertiary)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.danger.opacity(0.06),
                    in: RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
            .strokeBorder(Theme.danger.opacity(0.35), lineWidth: 1))
    }

    @ViewBuilder private func planView(_ plan: FinishPlanEnvelope) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            if let diffstat = plan.diffstat {
                Text("\(diffstat.filesChanged ?? plan.staged.count) files \u{00B7} +\(diffstat.added ?? 0) \u{2212}\(diffstat.removed ?? 0)")
                    .font(Theme.body(11, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                    .monospacedDigit()
            }
            ForEach(Array(plan.staged.prefix(40).enumerated()), id: \.offset) { _, file in
                HStack(spacing: 8) {
                    Text(file.path)
                        .font(Theme.monoS)
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    Text("\(file.bytes) B")
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.textTertiary)
                    Text(String(file.sha256.prefix(8)))
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.textTertiary)
                }
            }
            if plan.staged.count > 40 {
                Text("\u{2026} \(plan.staged.count - 40) more staged files")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.textTertiary)
            }
            if !plan.irreversibleSteps.isEmpty {
                HStack(spacing: 6) {
                    Text("IRREVERSIBLE:")
                        .font(Theme.body(10, weight: .bold))
                        .foregroundStyle(Theme.dangerText)
                    Text(plan.irreversibleSteps.joined(separator: ", "))
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.dangerText)
                }
            }
            Text("Preview only \u{2014} approving re-validates and re-stages from scratch.")
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.textTertiary)
        }
    }

    // MARK: Where the result goes

    @ViewBuilder private var destinationBand: some View {
        VStack(alignment: .leading, spacing: 6) {
            Theme.sectionTitle("WHERE THE RESULT GOES")
            destinationRadio(
                selected: isApply,
                title: "Apply to the project",
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
                title: "Export to a folder",
                subtitle: nil
            ) {
                // No invented default: the operator names the destination or
                // Approve stays disabled with the missing fact named below.
                coordinator.destination = .export(path: exportPath)
            }
            if !isApply {
                HStack(spacing: 6) {
                    Text("--dest")
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.textTertiary)
                    TextField("/path/to/export", text: $exportPath)
                        .textFieldStyle(.plain)
                        .font(Theme.monoM)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .frame(maxWidth: 320)
                        .inputChrome()
                        .onChange(of: exportPath) { _, path in
                            coordinator.destination = .export(path: path)
                        }
                }
                .padding(.leading, 22)
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

    /// Export selected but no destination typed yet: Approve and the
    /// preview stay disabled with this fact named (no invented default
    /// path).
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
                    .foregroundStyle(selected ? Theme.accent : Theme.textTertiary)
                    .font(.system(size: 12))
                Text(title)
                    .font(Theme.body(11.5, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                if let subtitle {
                    Text(subtitle)
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textTertiary)
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
                    Text("applying \u{2014} validate, stage, revalidate, move, revalidate\u{2026}")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textSecondary)
                }
            case .succeeded(let envelope):
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Image(systemName: "checkmark.seal")
                            .foregroundStyle(Theme.success)
                        Text("Approved \u{00B7} \(envelope.status ?? "completed")"
                            + (envelope.delivery?.stagedFileCount.map { " \u{00B7} \($0) files" } ?? ""))
                            .font(Theme.body(11.5, weight: .medium))
                            .foregroundStyle(Theme.textPrimary)
                    }
                    // The envelope's own next actions, verbatim (trust rule
                    // 2): an apply success offers `deadreckon undo`; an
                    // export (--dest) success offers show/status. The sheet
                    // never asserts an affordance the binary did not offer.
                    ForEach(envelope.nextActions, id: \.self) { action in
                        Text("next: \(action)")
                            .font(Theme.monoS)
                            .foregroundStyle(Theme.accent)
                            .textSelection(.enabled)
                    }
                    Text(successLifecycleLine(envelope))
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textTertiary)
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
                Button(dismissTitle) { dismiss() }
                    .buttonStyle(.themeStandard)
                    .keyboardShortcut(.cancelAction)
                Spacer()
                // No dismiss() first: the router's .sheet(item:) swaps the
                // presented content on identity change. dismiss()-then-set
                // in the same tick can race the dismissal animation and
                // swallow the follow-up sheet on some macOS versions.
                Button("Send back\u{2026}") {
                    onSendBack()
                }
                .buttonStyle(.themeStandard)
                // §A0 Discard note: Stop is enabled only for a non-terminal
                // row — a finished run has nothing left to stop.
                Button("Stop\u{2026}") {
                    onKill()
                }
                .buttonStyle(.themeQuietDanger)
                .disabled(liveRow.projection.phase == .terminal)
                .help(liveRow.projection.phase == .terminal
                    ? "This run already finished \u{2014} there is nothing to stop"
                    : "Stop this run \u{2014} asks first, then force-quits; full details in the confirm")
                promoteButton
            }
            if let reason = promoteDisabledReason {
                Text("Approve disabled: \(reason)")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.warn)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    /// Sheet-dismiss word: "Cancel" before dispatch, "Close" after a result.
    private var dismissTitle: String {
        switch coordinator.promotion {
        case .idle, .running: return "Cancel"
        case .succeeded, .refused, .failed: return "Close"
        }
    }

    private var promoteButton: some View {
        Button("Approve") {
            Task { await coordinator.promote(gate: gate) }
        }
        .buttonStyle(.themePrimary)
        .disabled(!promoteEnabled)
        .keyboardShortcut(.defaultAction)
        .help("deadreckon finish \(row.jobID) --yes --json")
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
        if exportPathMissing { return "no export folder entered \u{2014} there is no default" }
        return nil
    }

    /// The lifecycle hint under an approve success. The undo claim is made
    /// ONLY when `deadreckon undo` appears among the envelope's own next
    /// actions (apply destinations); an export success gets no rollback
    /// claim, because the binary offers none (trust rule 2).
    private func successLifecycleLine(_ envelope: MutationEnvelope) -> String {
        let offersUndo = envelope.nextActions.contains { $0.contains("deadreckon undo") }
        return (offersUndo ? "Undo with one command: deadreckon undo \u{00B7} " : "")
            + "the run updates from its files, not from this sheet"
    }
    // Section titles render through Theme.sectionTitle (the one shared
    // kerned-uppercase style).
}

/// The send-back sheet (G9): a follow-up goal plus your note, recorded as
/// typed provenance on the parent run. Not approved, not stopped — sent
/// back with a receipt of why.
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
                Text("Send back")
                    .font(Theme.title)
                    .foregroundStyle(Theme.textPrimary)
                Text(row.goal)
                    .font(Theme.body(11.5))
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(2)
                Text(row.jobID)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textTertiary)
                    .textSelection(.enabled)
                Text("Starts a follow-up run with the same definition of done. Your note is recorded and the agent reads it first.")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            VStack(alignment: .leading, spacing: 4) {
                Theme.sectionTitle("WHAT TO DO NEXT")
                TextEditor(text: $coordinator.goal)
                    .font(Theme.body(12))
                    .scrollContentBackground(.hidden)
                    .padding(6)
                    .frame(height: 56)
                    .inputChrome()
            }

            VStack(alignment: .leading, spacing: 4) {
                Theme.sectionTitle("YOUR NOTE \u{2014} RECORDED ON THE RUN")
                TextEditor(text: $coordinator.note)
                    .font(Theme.body(12))
                    .scrollContentBackground(.hidden)
                    .padding(6)
                    .frame(height: 72)
                    .inputChrome()
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
                        .foregroundStyle(Theme.success)
                    Text("Follow-up started \(envelope.id ?? "")"
                        + " \u{00B7} done-definition \(envelope.extend?.contract ?? "?")"
                        + " \u{00B7} note \(envelope.extend?.noteRecorded == true ? "recorded" : "not recorded")")
                        .font(Theme.small)
                        .foregroundStyle(Theme.textSecondary)
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
                        // the runs (which update from the files).
                        Button("I checked my runs \u{2014} enable Send back again") {
                            coordinator.rearmAfterPossibleQueue()
                        }
                        .buttonStyle(.themeText(size: 10.5))
                    }
                }
            }

            HStack {
                Spacer()
                Button(dismissTitle) { dismiss() }
                    .buttonStyle(.themeStandard)
                    .keyboardShortcut(.cancelAction)
                Button("Send back") {
                    Task { await coordinator.submit() }
                }
                .buttonStyle(.themePrimary)
                .disabled(!coordinator.canSubmit)
            }
        }
        .padding(20)
        .frame(width: 560)
        .background(Theme.windowBg)
    }

    /// Sheet-dismiss word: "Cancel" before dispatch, "Close" after a result.
    private var dismissTitle: String {
        switch coordinator.state {
        case .idle, .running: return "Cancel"
        case .queued, .refused, .failed: return "Close"
        }
    }
}
