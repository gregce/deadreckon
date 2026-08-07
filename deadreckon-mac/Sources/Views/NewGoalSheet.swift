import AppKit
import DeadreckonKit
import SwiftUI

/// The New Goal sheet (⌘N; REDESIGN-SPEC §C — the G2 "As built" launch
/// protocol re-ordered project-first): recent projects, goal, agent & model
/// (providers/models from the binary's own catalogs, failed probes
/// visible-but-disabled with their try lines), budget, the
/// definition-of-done step (read-only rows when a definition resolves; an
/// inline editor otherwise — the binary's own `def-done --yes --json`
/// writes .deadreckon/acceptance.yaml in the project, never the app), and
/// the plain-language resolved plan with the exact command in a
/// disclosure. A budget over $50 swaps Start for a type-the-amount
/// confirmation (SpendAcknowledgement: the flag cannot be passed any other
/// way). The new run appears via FleetStore/FSEvents when job.json lands —
/// never optimistically from this sheet.
struct NewGoalSheet: View {
    /// The fresh-install invitation arms the folder chooser on open (§B4).
    var chooseFolderOnOpen = false

    @Environment(\.dismiss) private var dismiss
    @StateObject private var controller: LayCourseController
    @StateObject private var catalog: LayCourseCatalog
    @State private var goalText = ""
    @State private var capText = ""
    @State private var projectPath = ""
    /// Recent project folders (§C1): app-side MRU, newest first, written
    /// only on a successful start.
    @State private var recents = NewGoalSheet.loadRecents()
    /// The done-definition editor's plain-English criteria.
    @State private var criteriaText = ""
    /// True while the operator has explicitly reopened the editor over an
    /// existing definition (declare overwrites binary-side by design).
    @State private var redefining = false
    /// The exact command lines, one disclosure away (§C6).
    @State private var commandShown = false
    @FocusState private var goalFocused: Bool

    init(chooseFolderOnOpen: Bool = false) {
        self.chooseFolderOnOpen = chooseFolderOnOpen
        _controller = StateObject(wrappedValue: LayCourseController(cli: WriteCLI.client))
        _catalog = StateObject(wrappedValue: LayCourseCatalog(cli: WriteCLI.client))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider().overlay(Theme.border)
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    projectSection
                    goalSection
                    routeSection
                    limitsSection
                    doneSection
                    previewSection
                }
                .padding(20)
            }
            Divider().overlay(Theme.border)
            footer
        }
        .frame(width: 680, height: 720)
        .background(Theme.windowBg)
        .task { await catalog.load() }
        .onAppear {
            if chooseFolderOnOpen {
                // The invitation path: the sheet opens with the chooser
                // armed. Deferred a beat so the sheet finishes presenting
                // before the modal panel runs.
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
                    chooseProjectDirectory()
                }
            }
        }
        // Autofocus the goal once a project exists (§C2): pick a folder,
        // then type — no mouse click between.
        .onChange(of: resolvedProjectDirectory) { _, project in
            if project != nil { goalFocused = true }
        }
    }

    private var header: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text("New Goal")
                    .font(Theme.title)
                    .foregroundStyle(Theme.textPrimary)
                Text("Everything is visible before anything runs.")
                    .font(Theme.small)
                    .foregroundStyle(Theme.textSecondary)
            }
            Spacer()
            Button(dismissTitle) { dismiss() }
                .buttonStyle(.themeStandard)
                .keyboardShortcut(.cancelAction)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    /// Sheet-dismiss word: "Cancel" before dispatch, "Close" after a result
    /// (§A3.6).
    private var dismissTitle: String {
        if case .launched = controller.execution { return "Close" }
        return "Cancel"
    }

    /// One bordered section panel with the shared label header (§C).
    private func sectionPanel(_ title: String,
                              @ViewBuilder content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Theme.sectionTitle(title)
            content()
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    // MARK: Project (first — the source dimension: start --from)

    /// The source-tree dimension: without it every GUI launch resolves its
    /// source from the client's working directory. The chosen directory
    /// rides `--from <path>` on BOTH legs (the launch plan does not embed
    /// the source; start.rs resolves it per invocation), and the preview's
    /// source fact line shows what the binary resolved.
    private var projectSection: some View {
        sectionPanel("PROJECT") {
            ForEach(recents, id: \.self) { path in
                recentRow(path)
            }
            HStack(spacing: 8) {
                Button("Choose Folder\u{2026}") { chooseProjectDirectory() }
                    .buttonStyle(.themeStandard)
                if let project = resolvedProjectDirectory {
                    Text(project)
                        .font(Theme.monoM)
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .padding(.horizontal, 6)
                        .frame(height: 18)
                        .background(Theme.well,
                                    in: RoundedRectangle(cornerRadius: Theme.chipRadius, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: Theme.chipRadius, style: .continuous)
                            .strokeBorder(Theme.border, lineWidth: 1))
                        .help(project)
                }
            }
            Text("The folder is copied into the run\u{2019}s workspace before launch \u{2014} your working tree is untouched until you approve.")
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.textTertiary)
                .help("passed as --from; the preview's source line below is the binary's own resolution")
        }
    }

    /// One recent-project radio row: folder name plain, full path mono.
    private func recentRow(_ path: String) -> some View {
        let selected = projectPath == path
        return Button {
            projectPath = path
        } label: {
            HStack(spacing: 8) {
                Image(systemName: selected ? "largecircle.fill.circle" : "circle")
                    .foregroundStyle(selected ? Theme.accent : Theme.textTertiary)
                    .font(.system(size: 12))
                Text(Lexicon.projectName(path))
                    .font(Theme.baseMedium)
                    .foregroundStyle(Theme.textPrimary)
                Text(path)
                    .font(Theme.monoM)
                    .foregroundStyle(Theme.textTertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer()
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.tactile)
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

    /// The chosen project path, tilde-expanded; nil when none chosen.
    private var resolvedProjectDirectory: String? {
        let trimmed = projectPath.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        return (trimmed as NSString).expandingTildeInPath
    }

    // MARK: Recents MRU (UserDefaults `recentProjects`, capped 5)

    private static let recentsKey = "recentProjects"

    static func loadRecents() -> [String] {
        UserDefaults.standard.stringArray(forKey: recentsKey) ?? []
    }

    private func recordRecent(_ path: String) {
        var list = Self.loadRecents().filter { $0 != path }
        list.insert(path, at: 0)
        recents = Array(list.prefix(5))
        UserDefaults.standard.set(recents, forKey: Self.recentsKey)
    }

    // MARK: Goal

    private var goalSection: some View {
        sectionPanel("GOAL") {
            TextEditor(text: $goalText)
                .font(Theme.body(12.5))
                .focused($goalFocused)
                .scrollContentBackground(.hidden)
                .padding(8)
                .frame(minHeight: 64, maxHeight: 110)
                .inputChrome(focused: goalFocused)
                .overlay(alignment: .topLeading) {
                    if goalText.isEmpty {
                        Text("What should the agent accomplish?")
                            .font(Theme.body(12.5))
                            .foregroundStyle(Theme.textTertiary)
                            .padding(.horizontal, 13)
                            .padding(.vertical, 8)
                            .allowsHitTesting(false)
                    }
                }
        }
    }

    // MARK: Agent & model

    @ViewBuilder private var routeSection: some View {
        sectionPanel("AGENT & MODEL") {
            switch catalog.providers {
            case .idle, .loading:
                Text("checking which agents are ready\u{2026}")
                    .font(Theme.small)
                    .foregroundStyle(Theme.textTertiary)
            case .failed(let reason):
                Text("couldn\u{2019}t list agents: \(reason)")
                    .font(Theme.small)
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

    /// One agent: selectable when the probe passed; a failed probe stays
    /// VISIBLE but disabled, with its message and try lines as the fix
    /// hints, verbatim. Plain display name first; the route id stays mono
    /// beside it (machine truth).
    @ViewBuilder private func providerRow(_ probe: ProviderProbeRow) -> some View {
        let selected = controller.request.provider == probe.id
        VStack(alignment: .leading, spacing: 3) {
            Button {
                controller.request.provider = probe.id
                controller.request.model = nil
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: selected ? "largecircle.fill.circle" : "circle")
                        .foregroundStyle(selected ? Theme.accent : Theme.textTertiary)
                        .font(.system(size: 12))
                    ProviderIcon(provider: probe.id, size: 14)
                    Text(Lexicon.agentName(probe.id, displayName: probe.displayName) ?? probe.id)
                        .font(Theme.baseMedium)
                        .foregroundStyle(probe.status == .ok ? Theme.textPrimary : Theme.textTertiary)
                    Text(probe.id)
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.textTertiary)
                    StatusChip(
                        text: Lexicon.probeWord(probe.status),
                        color: probe.status == .ok ? Theme.success
                            : probe.status == .failed ? Theme.danger : Theme.textTertiary,
                        textColor: probe.status == .failed ? Theme.dangerText : nil)
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
                            .font(Theme.monoS)
                            .foregroundStyle(Theme.warn)
                            .textSelection(.enabled)
                    }
                    ForEach(probe.tryLines, id: \.self) { line in
                        Text("try: \(line)")
                            .font(Theme.monoS)
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
                Text("Model")
                    .font(Theme.body(10.5, weight: .semibold))
                    .foregroundStyle(Theme.textTertiary)
                Picker("", selection: modelBinding) {
                    Text("Default for this agent").tag(String?.none)
                    ForEach(choices, id: \.id) { entry in
                        Text(entry.id + (entry.recommended ? " (recommended)" : ""))
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

    // MARK: Budget

    private var limitsSection: some View {
        sectionPanel("BUDGET") {
            HStack(spacing: 8) {
                Text("Up to $")
                    .font(Theme.small)
                    .foregroundStyle(Theme.textSecondary)
                TextField("agent default", text: $capText)
                    .textFieldStyle(.plain)
                    .font(Theme.mono(11.5))
                    .frame(width: 90)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .inputChrome()
                Text("Over $50, Start asks you to type the amount.")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.textTertiary)
            }
        }
    }

    // MARK: What does done mean? (§C5)

    /// The definition-of-done step. Rows always come from the binary's own
    /// envelopes (the def_done_result declare/show envelope when held, else
    /// the preview's done_contract block; `capabilities.network` defaults
    /// deny in the compiled contract) — no YAML is ever parsed app-side. An
    /// existing definition renders read-only with a Rewrite affordance that
    /// reopens the editor (declare overwrites binary-side by design); with
    /// nothing resolved yet the plain-English editor is simply here —
    /// everything visible, no hidden steps.
    @ViewBuilder private var doneSection: some View {
        sectionPanel("WHAT DOES DONE MEAN?") {
            if case .declared(let declared) = controller.contract {
                declaredContractRows(declared)
                redefineButton
                if redefining { editorBody }
            } else if let contract = previewEnvelope?.doneContract {
                if let network = contract.network {
                    factLine("Network", network == "deny" ? "not allowed (default)" : network)
                }
                ForEach(Array(contract.checks.enumerated()), id: \.offset) { _, check in
                    HStack(spacing: 6) {
                        Text(check.kind)
                            .font(Theme.mono(10.5))
                            .foregroundStyle(Theme.textPrimary)
                        if check.mustPass {
                            StatusChip(text: "must pass", color: Theme.textSecondary)
                        }
                    }
                    .padding(.leading, 12)
                }
                redefineButton
                if redefining { editorBody }
            } else {
                editorBody
            }
        }
    }

    /// The current preview envelope when one has resolved (ready or
    /// blocked), for the done-definition rows.
    private var previewEnvelope: StartPreviewEnvelope? {
        switch controller.preview {
        case .ready(let envelope), .blocked(let envelope): return envelope
        default: return nil
        }
    }

    /// The written definition's own facts, verbatim from the def_done_result
    /// envelope: check rows (kind, target, must_pass), network capability,
    /// the written file path, and who drafted.
    @ViewBuilder private func declaredContractRows(_ declared: DefDoneResultEnvelope) -> some View {
        if let network = declared.network {
            factLine("Network", network == "deny" ? "not allowed (default)" : network)
        }
        ForEach(Array(declared.checks.enumerated()), id: \.offset) { _, check in
            HStack(spacing: 6) {
                Text(check.kind)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textPrimary)
                if let target = check.target {
                    Text(target)
                        .font(Theme.monoS)
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                if check.mustPass {
                    StatusChip(text: "must pass", color: Theme.textSecondary)
                }
            }
            .padding(.leading, 12)
        }
        if let path = declared.contractPath {
            factLine("written to", path, mono: true)
        }
        if let draftedBy = declared.draftedBy {
            factLine("drafted by", draftedBy)
        }
    }

    private var redefineButton: some View {
        Button("Rewrite\u{2026}") { redefining = true }
            .buttonStyle(.themeText(size: 10))
            .disabled(redefining)
            .padding(.leading, 12)
            .help("Reopens the plain-English editor; drafting again replaces the checks (the CLI's own def-done).")
    }

    /// The inline editor: declaration, not silent mutation. The operator
    /// says in plain English what should count as done; Draft checks
    /// dispatches the binary's own `def-done --yes --json` verb against the
    /// chosen project — the click IS the approval `--yes` formalizes, the
    /// binary (never the app) writes .deadreckon/acceptance.yaml, and on
    /// success the preview re-runs by itself so the sheet flows refusal ->
    /// declare -> launchable.
    @ViewBuilder private var editorBody: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Say it in plain English \u{2014} checks are drafted from your words.")
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.textSecondary)
            HStack(spacing: 8) {
                TextField("builds, opens in a browser, and has no console errors",
                          text: $criteriaText)
                    .textFieldStyle(.plain)
                    .font(Theme.body(11.5))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 5)
                    .inputChrome()
                Button("Draft checks") {
                    Task {
                        await controller.declareContract(criteria: criteriaText)
                        if case .declared = controller.contract {
                            redefining = false
                        }
                    }
                }
                .buttonStyle(.themeStandard)
                .disabled(criteriaText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    || controller.contract == .drafting)
            }
            CommandLineView(command: controller.contractCommandLine(criteria: criteriaText))
            Text("The agent drafts checks from your words; the CLI writes `acceptance.yaml` in the project (the app writes nothing). The preview re-runs by itself.")
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.textTertiary)
                .fixedSize(horizontal: false, vertical: true)

            switch controller.contract {
            case .idle, .declared:
                EmptyView()
            case .drafting:
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("drafting checks \u{2014} the agent is reading your words\u{2026}")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textSecondary)
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
    }

    // MARK: Preview & start (§C6)

    @ViewBuilder private var previewSection: some View {
        sectionPanel("PREVIEW & START") {
            HStack(spacing: 8) {
                if resolvedProjectDirectory == nil {
                    Text("choose a project folder first")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textTertiary)
                }
                Spacer()
                Button("Preview the plan") {
                    controller.request.goal = goalText
                    controller.request.maxSpendUSD = SpendAcknowledgement.parseAmount(capText)
                    controller.request.projectDirectory = resolvedProjectDirectory
                    redefining = false
                    Task { await controller.runPreview() }
                }
                .buttonStyle(.themeStandard)
                .disabled(goalText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    || resolvedProjectDirectory == nil)
            }

            switch controller.preview {
            case .idle:
                // After a launch the armed preview is deliberately dropped
                // (round-2 disarm); say so instead of "nothing has run yet".
                if case .launched = controller.execution {
                    Text("Run started \u{2014} preview again to start another.")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textTertiary)
                } else {
                    Text("Nothing runs during preview.")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textTertiary)
                        .help("the preview is read-only (will_start: false)")
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
                    // binary resolving the definition from the app's working
                    // directory instead of --from). Say so plainly — the
                    // written file path in the section above is real; the
                    // editor there is the fix surface.
                    if case .declared = controller.contract {
                        Text("Your definition of done was written (see the file above), but the fresh preview still reports it missing \u{2014} the CLI\u{2019}s suggestions above are the fix.")
                            .font(Theme.body(10.5))
                            .foregroundStyle(Theme.warn)
                            .fixedSize(horizontal: false, vertical: true)
                    } else {
                        Text("This plan can\u{2019}t start yet \u{2014} say what done means above, then preview again.")
                            .font(Theme.body(10.5))
                            .foregroundStyle(Theme.warn)
                    }
                } else {
                    Text("This plan can\u{2019}t start yet \u{2014} the fixes above are the CLI\u{2019}s own suggestions.")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.warn)
                }
            case .ready(let envelope):
                previewFacts(envelope)
            }

            commandDisclosure
        }
    }

    /// The resolved plan in plain language, one fact per line — values
    /// verbatim from the envelope (§C6).
    @ViewBuilder private func previewFacts(_ envelope: StartPreviewEnvelope) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            if let project = resolvedProjectDirectory {
                factLine("Run in", project, mono: true)
            }
            factLine("How", (envelope.selectedMode ?? "?")
                + (envelope.reason.map { " \u{00B7} \($0)" } ?? ""))
            factLine("Agent", agentFact(envelope))
            factLine("Source", envelope.sourceMode ?? "?")
            factLine("Budget", budgetFact)
            factLine("Done means", doneFact(envelope))
            if !envelope.tryLines.isEmpty {
                ForEach(envelope.tryLines, id: \.self) { line in
                    Text("try: \(line)")
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.accent)
                        .textSelection(.enabled)
                }
            }
            if envelope.isLaunchable {
                factLine("Start", "not yet \u{2014} Start replays this exact plan")
            }
        }
    }

    private func agentFact(_ envelope: StartPreviewEnvelope) -> String {
        let id = envelope.provider ?? "?"
        var words = Lexicon.agentName(id).map { "\($0) (\(id))" } ?? id
        if let source = envelope.providerSource { words += " \u{00B7} \(source)" }
        words += " \u{00B7} model \(controller.request.model ?? "default")"
        return words
    }

    private var budgetFact: String {
        if let cap = SpendAcknowledgement.parseAmount(capText) {
            return String(format: "up to $%.2f", cap)
        }
        return "agent default"
    }

    private func doneFact(_ envelope: StartPreviewEnvelope) -> String {
        var words = envelope.doneCriteria ?? "none resolved"
        if let source = envelope.doneCriteriaSource { words += " (\(source))" }
        if let contract = envelope.doneContract {
            words += " \u{00B7} \(contract.checks.count) check\(contract.checks.count == 1 ? "" : "s")"
            if let network = contract.network {
                words += " \u{00B7} network \(network)"
            }
        }
        return words
    }

    /// The exact CLI truth, one disclosure away: the preview's own line,
    /// and the start line once a plan is armed.
    @ViewBuilder private var commandDisclosure: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                withAnimation(.easeOut(duration: 0.15)) { commandShown.toggle() }
            } label: {
                Text(commandShown ? "Command \u{25BE}" : "Command \u{25B8}")
            }
            .buttonStyle(.themeText(size: 11))
            if commandShown {
                CommandLineView(command: controller.previewCommandLine)
                if case .ready = controller.preview {
                    CommandLineView(command: controller.executeCommandLine)
                }
            }
        }
    }

    private func factLine(_ label: String, _ value: String, mono: Bool = false) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Text(label)
                .font(Theme.body(10, weight: .bold))
                .foregroundStyle(Theme.textTertiary)
                .frame(width: 92, alignment: .trailing)
            Text(value)
                .font(mono ? Theme.monoM : Theme.small)
                .foregroundStyle(Theme.textSecondary)
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
                    Text("starting the run\u{2026}")
                        .font(Theme.small)
                        .foregroundStyle(Theme.textSecondary)
                }
            case .launched(let envelope):
                HStack(spacing: 6) {
                    Image(systemName: "checkmark.circle")
                        .foregroundStyle(Theme.success)
                    Text("Started \(envelope.id ?? "") \u{2014} it appears in the sidebar once its files land on disk.")
                        .font(Theme.small)
                        .foregroundStyle(Theme.textSecondary)
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
                HStack(spacing: 10) {
                    Spacer()
                    if controller.acknowledgement.required {
                        spendAckField
                    }
                    startButton
                }
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    /// The >$50 typed confirmation: the GUI-honest --i-know-its-a-lot. The
    /// button stays disabled until the typed amount matches the resolved
    /// plan ceiling; the flag itself derives ONLY from that match. The
    /// field's stroke is semantic: warn until matched, success when matched.
    private var spendAckField: some View {
        HStack(spacing: 6) {
            Text(String(format: "Budget over $50 \u{2014} type %.2f to confirm:",
                        controller.acknowledgement.capUSD ?? 0))
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.warn)
            TextField("amount", text: $controller.acknowledgement.typedAmount)
                .textFieldStyle(.plain)
                .font(Theme.mono(11.5))
                .frame(width: 80)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .inputChrome(stroke: controller.acknowledgement.typedMatches
                    ? Theme.success : Theme.warn)
        }
    }

    private var startButton: some View {
        Button("Start Run") {
            Task {
                await controller.execute()
                // The recents MRU records only a start that actually
                // dispatched (§C1: written on successful start).
                if case .launched = controller.execution,
                   let project = resolvedProjectDirectory {
                    recordRecent(project)
                }
            }
        }
        .buttonStyle(.themePrimary)
        .disabled(!startEnabled)
        .keyboardShortcut(.defaultAction)
        .help(controller.acknowledgement.required && !controller.acknowledgement.typedMatches
            ? "Budgets over $50 need the exact amount typed (the CLI's --i-know-its-a-lot acknowledgment)."
            : "Runs the previewed plan exactly: start --plan \u{2026} --yes")
    }

    private var startEnabled: Bool {
        if case .ready = controller.preview {
            if case .running = controller.execution { return false }
            return controller.acknowledgement.readyToStart
        }
        return false
    }
}
