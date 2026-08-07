import AppKit
import DeadreckonKit
import SwiftUI

/// The Lay Course sheet (Command-N; design B4 composed per section 6.2 and
/// the G2 "As built" launch protocol): goal, route (providers/models from
/// the binary's own catalogs, failed probes visible-but-disabled with their
/// try lines), limits, the done-contract step (read-only rows when the
/// preview resolves one; an inline declare editor when the preview is
/// blocked for a missing contract — the binary's own `def-done --yes
/// --json` writes .deadreckon/acceptance.yaml in the project, never the
/// app), the resolved launch preview, and the execute leg replaying the
/// embedded plan verbatim. A cap over $50 swaps Start for a
/// type-the-amount confirmation (SpendAcknowledgement: the flag cannot be
/// passed any other way). The new job appears via FleetStore/FSEvents when
/// job.json lands — never optimistically from this sheet.
struct LayCourseSheet: View {
    @Environment(\.dismiss) private var dismiss
    @StateObject private var controller: LayCourseController
    @StateObject private var catalog: LayCourseCatalog
    @State private var goalText = ""
    @State private var capText = ""
    @State private var projectPath = ""
    /// The done-contract editor's plain-English criteria.
    @State private var criteriaText = ""
    /// True while the operator has explicitly reopened the editor over an
    /// existing contract (declare overwrites binary-side by design).
    @State private var redefining = false
    /// Autofocus on open (the CommandPalette discipline): Command-N then
    /// type, no mouse click between.
    @FocusState private var goalFocused: Bool

    init() {
        _controller = StateObject(wrappedValue: LayCourseController(cli: WriteCLI.client))
        _catalog = StateObject(wrappedValue: LayCourseCatalog(cli: WriteCLI.client))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider().overlay(Theme.hairline)
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    goalSection
                    projectSection
                    routeSection
                    limitsSection
                    previewSection
                }
                .padding(18)
            }
            Divider().overlay(Theme.hairline)
            footer
        }
        .frame(width: 640, height: 640)
        .background(Theme.paper)
        .task { await catalog.load() }
        .onAppear { goalFocused = true }
    }

    private var header: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text("Lay Course")
                    .font(Theme.display(20))
                    .foregroundStyle(Theme.ink)
                Text("preview before launch \u{00B7} the plan is the decision")
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.inkTertiary)
                if let project = resolvedProjectDirectory {
                    Text("project: \(project)")
                        .font(Theme.mono(10))
                        .foregroundStyle(Theme.inkSecondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            Spacer()
            Button("Close") { dismiss() }
                .buttonStyle(.tactile)
                .keyboardShortcut(.cancelAction)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
    }

    // MARK: Goal

    private var goalSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Theme.sectionTitle("GOAL")
            TextEditor(text: $goalText)
                .font(Theme.body(12.5))
                .focused($goalFocused)
                .scrollContentBackground(.hidden)
                .padding(8)
                .frame(minHeight: 64, maxHeight: 110)
                .background(Theme.card, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(Theme.hairline, lineWidth: 1))
        }
    }

    // MARK: Project (source dimension: start --from)

    /// The source-tree dimension design B4 assumes ("new durable Job in
    /// itavero/billing"): without it every GUI launch resolves its source
    /// from the client's working directory. A chosen directory rides
    /// `--from <path>` on BOTH legs (the launch plan does not embed the
    /// source; start.rs resolves it per invocation), and the preview's
    /// source fact line shows what the binary resolved.
    private var projectSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Theme.sectionTitle("PROJECT")
            HStack(spacing: 8) {
                TextField("resolved from the app's working directory unless set",
                          text: $projectPath)
                    .textFieldStyle(.plain)
                    .font(Theme.mono(11))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(Theme.card, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .strokeBorder(Theme.hairline, lineWidth: 1))
                Button("Choose\u{2026}") { chooseProjectDirectory() }
                    .buttonStyle(.tactile)
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.accent)
            }
            Text("passed as --from (the source directory is copied into runstate before launch); the preview's source line below is the binary's own resolution")
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.inkTertiary)
        }
    }

    private func chooseProjectDirectory() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.prompt = "Use as project"
        if panel.runModal() == .OK, let url = panel.url {
            projectPath = url.path
        }
    }

    /// The typed/chosen project path, tilde-expanded; nil when empty.
    private var resolvedProjectDirectory: String? {
        let trimmed = projectPath.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        return (trimmed as NSString).expandingTildeInPath
    }

    // MARK: Route (Pennant)

    @ViewBuilder private var routeSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Theme.sectionTitle("ROUTE (Pennant)")
            switch catalog.providers {
            case .idle, .loading:
                Text("probing provider routes\u{2026}")
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.inkTertiary)
            case .failed(let reason):
                Text("providers list failed: \(reason)")
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            case .loaded(let envelope):
                providerRows(envelope)
            }
        }
    }

    @ViewBuilder private func providerRows(_ envelope: ProvidersEnvelope) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            ForEach(envelope.providers, id: \.id) { probe in
                providerRow(probe)
            }
            if let provider = controller.request.provider {
                modelPicker(for: provider)
            }
        }
    }

    /// One provider route: selectable when the probe passed; a failed probe
    /// stays VISIBLE but disabled, with its message and try lines as the fix
    /// hints, verbatim.
    @ViewBuilder private func providerRow(_ probe: ProviderProbeRow) -> some View {
        let selected = controller.request.provider == probe.id
        VStack(alignment: .leading, spacing: 3) {
            Button {
                controller.request.provider = probe.id
                controller.request.model = nil
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: selected ? "largecircle.fill.circle" : "circle")
                        .foregroundStyle(selected ? Theme.accent : Theme.inkTertiary)
                        .font(.system(size: 12))
                    Text(probe.id)
                        .font(Theme.mono(11.5))
                        .foregroundStyle(probe.status == .ok ? Theme.ink : Theme.inkTertiary)
                    if let name = probe.displayName {
                        Text(name)
                            .font(Theme.body(10.5))
                            .foregroundStyle(Theme.inkTertiary)
                    }
                    StatusChip(
                        text: GlossaryText.providerProbeWord(probe.status),
                        color: probe.status == .ok ? Theme.verified
                            : probe.status == .failed ? Theme.danger : Theme.inkTertiary)
                    Spacer()
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.tactile)
            .disabled(probe.status != .ok)

            if probe.status != .ok {
                VStack(alignment: .leading, spacing: 2) {
                    if let message = probe.message {
                        Text(message)
                            .font(Theme.body(10))
                            .foregroundStyle(Theme.warn)
                            .textSelection(.enabled)
                    }
                    ForEach(probe.tryLines, id: \.self) { line in
                        Text("try: \(line)")
                            .font(Theme.mono(10))
                            .foregroundStyle(Theme.accent)
                            .textSelection(.enabled)
                    }
                }
                .padding(.leading, 20)
            }
        }
    }

    @ViewBuilder private func modelPicker(for provider: String) -> some View {
        let choices = catalog.modelChoices(for: provider)
        if !choices.isEmpty {
            HStack(spacing: 8) {
                Text("model")
                    .font(Theme.body(10.5, weight: .semibold))
                    .foregroundStyle(Theme.inkTertiary)
                Picker("", selection: modelBinding) {
                    Text("route default").tag(String?.none)
                    ForEach(choices, id: \.id) { entry in
                        Text(entry.id + (entry.recommended ? " \u{2605}" : ""))
                            .tag(String?.some(entry.id))
                    }
                }
                .labelsHidden()
                .frame(maxWidth: 320, alignment: .leading)
            }
            .padding(.top, 2)
        }
    }

    private var modelBinding: Binding<String?> {
        Binding(get: { controller.request.model },
                set: { controller.request.model = $0 })
    }

    // MARK: Limits

    private var limitsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Theme.sectionTitle("LIMITS")
            HStack(spacing: 8) {
                Text("spend cap $")
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.inkSecondary)
                TextField("route default", text: $capText)
                    .textFieldStyle(.plain)
                    .font(Theme.mono(11.5))
                    .frame(width: 90)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(Theme.card, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .strokeBorder(Theme.hairline, lineWidth: 1))
                Text("above $50 the Start button becomes a typed confirmation")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.inkTertiary)
            }
        }
    }

    // MARK: Preview + contract

    @ViewBuilder private var previewSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Theme.sectionTitle("LAUNCH PLAN PREVIEW")
                Spacer()
                Button("Preview course") {
                    controller.request.goal = goalText
                    controller.request.maxSpendUSD = SpendAcknowledgement.parseAmount(capText)
                    controller.request.projectDirectory = resolvedProjectDirectory
                    redefining = false
                    Task { await controller.runPreview() }
                }
                .buttonStyle(.tactile)
                .font(Theme.body(11, weight: .semibold))
                .foregroundStyle(Theme.accent)
                .disabled(goalText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            CommandLineView(command: controller.previewCommandLine)

            switch controller.preview {
            case .idle:
                // After a launch the armed preview is deliberately dropped
                // (round-2 disarm); say so instead of "nothing has run yet".
                if case .launched = controller.execution {
                    Text("course queued \u{2014} run a fresh preview to lay another")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkTertiary)
                } else {
                    Text("nothing has run yet \u{2014} the preview is read-only (will_start: false)")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkTertiary)
                }
            case .loading:
                ProgressView().controlSize(.small)
            case .refused(let refusal):
                RefusalView(refusal: refusal)
            case .failed(let words):
                Text(words)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            case .blocked(let envelope):
                previewFacts(envelope)
                if envelope.missingDoneContract {
                    // The auto re-preview after a successful declare can
                    // still come back "missing" (e.g. an older vendored
                    // binary resolving the contract from the app's working
                    // directory instead of --from). Say so plainly instead
                    // of silently re-offering the editor as if nothing
                    // happened — the declared file path above is real.
                    if case .declared = controller.contract {
                        Text("the contract was declared (see the declared file above), but the re-run preview still reports it missing \u{2014} the binary's try lines above are the fix")
                            .font(Theme.body(10.5))
                            .foregroundStyle(Theme.warn)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    contractEditor
                } else {
                    Text("this preview is not launchable \u{2014} the binary's try lines above are the fix")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.warn)
                    if redefining {
                        contractEditor
                    }
                }
            case .ready(let envelope):
                previewFacts(envelope)
                if redefining {
                    contractEditor
                }
            }
        }
    }

    @ViewBuilder private func previewFacts(_ envelope: StartPreviewEnvelope) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            factLine("mode", (envelope.selectedMode ?? "?")
                + (envelope.reason.map { " \u{00B7} \($0)" } ?? ""))
            factLine("route", (envelope.provider ?? "?")
                + (envelope.providerSource.map { " (\($0))" } ?? ""))
            factLine("source", envelope.sourceMode ?? "?")
            doneContractBand(envelope)
            if !envelope.tryLines.isEmpty {
                ForEach(envelope.tryLines, id: \.self) { line in
                    Text("try: \(line)")
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.accent)
                        .textSelection(.enabled)
                }
            }
            if envelope.isLaunchable {
                factLine("will start", "not yet \u{2014} Start replays this exact plan with --yes")
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    /// The done-contract step. Rows always come from the binary's own
    /// envelopes (the def_done_result declare/show envelope when held, else
    /// the preview's done_contract block; `capabilities.network` defaults
    /// deny in the compiled contract) — no YAML is ever parsed app-side. An
    /// existing contract renders read-only with a Redefine affordance that
    /// reopens the editor (declare overwrites binary-side by design).
    @ViewBuilder private func doneContractBand(_ envelope: StartPreviewEnvelope) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            factLine("done contract", (envelope.doneCriteria ?? "none resolved")
                + (envelope.doneCriteriaSource.map { " (\($0))" } ?? ""))
            if case .declared(let declared) = controller.contract {
                declaredContractRows(declared)
                redefineButton
            } else if let contract = envelope.doneContract {
                if let network = contract.network {
                    factLine("network", network + (network == "deny" ? " (default)" : ""))
                }
                ForEach(Array(contract.checks.enumerated()), id: \.offset) { _, check in
                    HStack(spacing: 6) {
                        Text(check.kind)
                            .font(Theme.mono(10.5))
                            .foregroundStyle(Theme.ink)
                        if check.mustPass {
                            StatusChip(text: "must pass", color: Theme.inkSecondary)
                        }
                    }
                    .padding(.leading, 12)
                }
                redefineButton
            }
        }
    }

    /// The declared contract's own facts, verbatim from the def_done_result
    /// envelope: check rows (kind, target, must_pass), network capability,
    /// the declared file path, and who drafted.
    @ViewBuilder private func declaredContractRows(_ declared: DefDoneResultEnvelope) -> some View {
        if let network = declared.network {
            factLine("network", network + (network == "deny" ? " (default)" : ""))
        }
        ForEach(Array(declared.checks.enumerated()), id: \.offset) { _, check in
            HStack(spacing: 6) {
                Text(check.kind)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.ink)
                if let target = check.target {
                    Text(target)
                        .font(Theme.mono(10))
                        .foregroundStyle(Theme.inkSecondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                if check.mustPass {
                    StatusChip(text: "must pass", color: Theme.inkSecondary)
                }
            }
            .padding(.leading, 12)
        }
        if let path = declared.contractPath {
            factLine("declared file", path)
        }
        if let draftedBy = declared.draftedBy {
            factLine("drafted by", draftedBy)
        }
    }

    private var redefineButton: some View {
        Button("Redefine\u{2026}") { redefining = true }
            .buttonStyle(.tactile)
            .font(Theme.body(10))
            .foregroundStyle(Theme.accent)
            .disabled(redefining)
            .padding(.leading, 12)
            .help("Reopens the plain-English editor; declaring again overwrites the contract via the binary's own def-done verb.")
    }

    // MARK: Done-contract editor (declare leg)

    /// The inline DONE CONTRACT editor: declaration, not silent mutation.
    /// The operator says in plain English what should count as done;
    /// Draft contract dispatches the binary's own `def-done --yes --json`
    /// verb against the chosen project — the click IS the approval `--yes`
    /// formalizes, the binary (never the app) writes
    /// .deadreckon/acceptance.yaml, and on success the preview re-runs by
    /// itself so the sheet flows refusal -> declare -> launchable.
    @ViewBuilder private var contractEditor: some View {
        VStack(alignment: .leading, spacing: 6) {
            Theme.sectionTitle("DONE CONTRACT")
            Text("what should count as done, in plain English")
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.inkSecondary)
            HStack(spacing: 8) {
                TextField("builds, opens in a browser, and has no console errors",
                          text: $criteriaText)
                    .textFieldStyle(.plain)
                    .font(Theme.body(11.5))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 5)
                    .background(Theme.card, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .strokeBorder(Theme.hairline, lineWidth: 1))
                Button("Draft contract") {
                    Task {
                        await controller.declareContract(criteria: criteriaText)
                        if case .declared = controller.contract {
                            redefining = false
                        }
                    }
                }
                .buttonStyle(.tactile)
                .font(Theme.body(11, weight: .semibold))
                .foregroundStyle(Theme.accent)
                .disabled(criteriaText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    || controller.contract == .drafting)
            }
            CommandLineView(command: controller.contractCommandLine(criteria: criteriaText))
            Text("drafts checks from your words via the configured provider, then the binary writes .deadreckon/acceptance.yaml (and acceptance.md) in the project \u{2014} the app itself writes nothing; on success the preview re-runs automatically")
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.inkTertiary)
                .fixedSize(horizontal: false, vertical: true)

            switch controller.contract {
            case .idle, .declared:
                EmptyView()
            case .drafting:
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("drafting the contract\u{2026} the binary is calling the provider")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkSecondary)
                }
            case .refused(let refusal):
                RefusalView(refusal: refusal)
            case .failed(let words):
                Text(words)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    private func factLine(_ label: String, _ value: String) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Text(label)
                .font(Theme.body(10, weight: .bold))
                .foregroundStyle(Theme.inkTertiary)
                .frame(width: 92, alignment: .trailing)
            Text(value)
                .font(Theme.body(11))
                .foregroundStyle(Theme.inkSecondary)
                .textSelection(.enabled)
        }
    }

    // MARK: Footer (execute leg)

    @ViewBuilder private var footer: some View {
        VStack(alignment: .leading, spacing: 8) {
            switch controller.execution {
            case .idle:
                EmptyView()
            case .running:
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("queueing the durable Job\u{2026}")
                        .font(Theme.body(11))
                        .foregroundStyle(Theme.inkSecondary)
                }
            case .launched(let envelope):
                HStack(spacing: 6) {
                    Image(systemName: "checkmark.circle")
                        .foregroundStyle(Theme.verified)
                    Text("queued \(envelope.id ?? "") \u{00B7} the row appears when job.json lands (file-backed, never optimistic)")
                        .font(Theme.body(11))
                        .foregroundStyle(Theme.inkSecondary)
                        .textSelection(.enabled)
                }
            case .refused(let refusal):
                RefusalView(refusal: refusal)
            case .failed(let words):
                Text(words)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            }

            if case .ready = controller.preview {
                CommandLineView(command: controller.executeCommandLine)
                HStack(spacing: 10) {
                    Spacer()
                    if controller.acknowledgement.required {
                        spendAckField
                    }
                    startButton
                }
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
    }

    /// The >$50 typed confirmation: the GUI-honest --i-know-its-a-lot. The
    /// button stays disabled until the typed amount matches the resolved
    /// plan ceiling; the flag itself derives ONLY from that match.
    private var spendAckField: some View {
        HStack(spacing: 6) {
            Text(String(format: "cap $%.2f \u{2014} type the amount to arm:",
                        controller.acknowledgement.capUSD ?? 0))
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.warn)
            TextField("amount", text: $controller.acknowledgement.typedAmount)
                .textFieldStyle(.plain)
                .font(Theme.mono(11.5))
                .frame(width: 80)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(Theme.card, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .strokeBorder(
                        controller.acknowledgement.typedMatches ? Theme.verified : Theme.warn,
                        lineWidth: 1))
        }
    }

    private var startButton: some View {
        Button {
            Task { await controller.execute() }
        } label: {
            Text("\u{2693} Start \u{2014} queues the Job, detaches")
                .font(Theme.body(12, weight: .semibold))
                .foregroundStyle(Theme.onFill)
                .padding(.horizontal, 14)
                .padding(.vertical, 7)
                .background(startEnabled ? Theme.accent : Theme.inkTertiary, in: Capsule())
        }
        .buttonStyle(.tactile)
        .disabled(!startEnabled)
        .keyboardShortcut(.defaultAction)
        .help(controller.acknowledgement.required && !controller.acknowledgement.typedMatches
            ? "A launch budget above $50 requires typing the exact amount (the --i-know-its-a-lot acknowledgment)."
            : "Replays the previewed plan verbatim: start --plan <file> --yes --json")
    }

    private var startEnabled: Bool {
        if case .ready = controller.preview {
            if case .running = controller.execution { return false }
            return controller.acknowledgement.readyToStart
        }
        return false
    }

    // Section titles render through Theme.sectionTitle (the one shared
    // kerned-uppercase style).
}
