import DeadreckonKit
import SwiftUI

/// The first-run gate (SETTINGS-SCREENS-SPEC §R2): decides, from live
/// probes, whether the empty-fleet center shows the ONE setup panel or the
/// standard "Start your first goal" empty state. Unknown facts never
/// summon the panel (SetupDerivation): a broken probe falls toward the
/// standard empty state, whose own degraded surfaces tell that story.
struct FirstRunGate: View {
    /// Opens the New Goal sheet; `true` arms the folder chooser.
    let onNewGoal: (_ armFolderChooser: Bool) -> Void
    /// "Set up later" — session-scoped; the panel returns on next launch
    /// only while setup is incomplete and the fleet is still empty.
    @Binding var dismissed: Bool

    @StateObject private var doctor = DoctorStore(cli: WriteCLI.client)
    @StateObject private var catalog = LayCourseCatalog(cli: WriteCLI.client)
    @StateObject private var service = ServiceController(cli: WriteCLI.client)
    @StateObject private var config = ConfigStore(cli: WriteCLI.client)
    @StateObject private var proof = TryController(cli: WriteCLI.client)

    var body: some View {
        Group {
            if dismissed {
                EmptyFleetView { onNewGoal(true) }
            } else if SetupDerivation.isIncomplete(inputs) {
                FirstRunPanel(
                    doctor: doctor, catalog: catalog, service: service,
                    config: config, proof: proof,
                    onNewGoal: onNewGoal,
                    onSetUpLater: { dismissed = true })
            } else if probesFinished {
                EmptyFleetView { onNewGoal(true) }
            } else {
                VStack(spacing: 10) {
                    ProgressView().controlSize(.small)
                    Text("Checking your setup\u{2026}")
                        .font(Theme.body(12))
                        .foregroundStyle(Theme.textSecondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .task {
            async let doctorRun: Void = doctor.run()
            async let catalogRun: Void = catalog.load()
            async let serviceRun: Void = service.refreshStatus()
            async let configRun: Void = config.load()
            _ = await (doctorRun, catalogRun, serviceRun, configRun)
        }
    }

    /// §R2 completeness facts, each nil until its probe positively
    /// answered.
    private var inputs: SetupDerivation.Inputs {
        var inputs = SetupDerivation.Inputs()
        if case .loaded(let report) = doctor.state {
            inputs.configPresent = report.configPresent
        }
        if case .loaded(let envelope) = catalog.providers {
            inputs.agentRouteCount = envelope.providers.count
        }
        if case .loaded = service.status {
            inputs.serviceNotInstalled = service.displayVerdict == .notInstalled
        }
        return inputs
    }

    /// Every probe reached a terminal state (loaded OR failed): the
    /// panel-or-empty-state decision is final for this pass.
    private var probesFinished: Bool {
        let doctorDone: Bool
        switch doctor.state {
        case .loaded, .unavailable: doctorDone = true
        default: doctorDone = false
        }
        let catalogDone: Bool
        switch catalog.providers {
        case .loaded, .failed: catalogDone = true
        default: catalogDone = false
        }
        let serviceDone: Bool
        switch service.status {
        case .loaded, .unavailable: serviceDone = true
        default: serviceDone = false
        }
        return doctorDone && catalogDone && serviceDone
    }
}

/// The setup panel (§R2): ONE surface, five rows, any order, never a
/// wizard. Every fact on it is a probe result or an envelope; every write
/// goes through the one dispatcher; the degraded paths name the Terminal
/// command instead of rendering a dead control.
struct FirstRunPanel: View {
    @ObservedObject var doctor: DoctorStore
    @ObservedObject var catalog: LayCourseCatalog
    @ObservedObject var service: ServiceController
    @ObservedObject var config: ConfigStore
    @ObservedObject var proof: TryController

    let onNewGoal: (_ armFolderChooser: Bool) -> Void
    let onSetUpLater: () -> Void

    /// The locally chosen agent route (radio). Seeded from the config
    /// envelope's defaults.provider when the config surface is armed.
    @State private var selectedRoute: String?
    @State private var keyText = ""
    @State private var installSheetShown = false

    @AppStorage("settings.section") private var settingsSection = SettingsSection.general.rawValue
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        // Centers vertically while the checklist fits (the app's other
        // empty states center); grows into a normal scroll when it does not.
        GeometryReader { geometry in
            ScrollView {
                VStack(spacing: 14) {
                    VStack(spacing: 4) {
                        Text("Set up deadreckon")
                            .font(Theme.display)
                            .foregroundStyle(Theme.textPrimary)
                        Text("Five checks and you\u{2019}re running goals. Do them in any order.")
                            .font(Theme.base)
                            .foregroundStyle(Theme.textSecondary)
                    }

                    VStack(spacing: 0) {
                        cliRow
                        Divider().overlay(Theme.border)
                        agentRow
                        Divider().overlay(Theme.border)
                        keyRow
                        Divider().overlay(Theme.border)
                        serviceRow
                        Divider().overlay(Theme.border)
                        proofRow
                    }
                    .cardChrome()

                    Button("Start your first goal") { onNewGoal(true) }
                        .buttonStyle(.themePrimary)
                        .disabled(!readyForFirstGoal)
                        .help(readyForFirstGoal
                            ? "Open New Goal"
                            : "Verify the CLI and pick a ready agent first \u{2014} the other rows are recommended, not required.")

                    Button("Set up later") { onSetUpLater() }
                        .buttonStyle(.themeText(size: 11))
                }
                .padding(24)
                .frame(maxWidth: 560)
                .frame(maxWidth: .infinity, minHeight: geometry.size.height)
            }
        }
        .sheet(isPresented: $installSheetShown) {
            InstallAndStartSheet(service: service)
        }
        .onAppear(perform: seedSelection)
        .onChange(of: config.capability) { _, _ in seedSelection() }
        // A landed config write can change doctor's config_present fact and
        // the providers picture (a saved key flips a probe): re-read both
        // so the completeness derivation rides fresh file truth and the
        // panel can yield the moment every row is green.
        .onChange(of: config.writeState) { _, state in
            if case .completed = state {
                Task {
                    async let doctorRun: Void = doctor.run()
                    async let catalogRun: Void = catalog.load()
                    _ = await (doctorRun, catalogRun)
                }
            }
        }
    }

    /// Rows 1–2 green gate the footer (§R2): key, service, and proof are
    /// recommended, not gates — the New Goal preview refuses honestly if
    /// something is actually missing.
    private var readyForFirstGoal: Bool {
        cliVerified && selectedProbe?.status == .ok
    }

    private func seedSelection() {
        guard selectedRoute == nil,
              let envelope = config.envelope,
              case .string(let route)? = envelope.settings["defaults.provider"]?.value,
              !route.isEmpty else { return }
        selectedRoute = route
    }

    // MARK: Row scaffold

    private enum Glyph {
        case done, missing, neutral, pending, busy

        @ViewBuilder var view: some View {
            switch self {
            case .done:
                Text("\u{2713}").font(Theme.body(12, weight: .bold)).foregroundStyle(Theme.success)
            case .missing:
                Text("\u{2717}").font(Theme.body(12, weight: .bold)).foregroundStyle(Theme.warn)
            case .neutral:
                Text("\u{2013}").font(Theme.body(12, weight: .bold)).foregroundStyle(Theme.textTertiary)
            case .pending:
                Text("\u{25CB}").font(Theme.body(12, weight: .bold)).foregroundStyle(Theme.textTertiary)
            case .busy:
                ProgressView().controlSize(.mini)
            }
        }
    }

    private func row(_ glyph: Glyph, _ label: String,
                     @ViewBuilder content: () -> some View) -> some View {
        HStack(alignment: .top, spacing: 10) {
            glyph.view.frame(width: 14, alignment: .center)
            Text(label)
                .font(Theme.baseMedium)
                .foregroundStyle(Theme.textPrimary)
                // 132 keeps the longest label ("Background service") on
                // one line at 13 medium.
                .frame(width: 132, alignment: .leading)
            VStack(alignment: .leading, spacing: 5) {
                content()
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }

    // MARK: Row 1 — CLI (BinaryLocator + manifest, computed once)

    private enum CLIState {
        case verified(String)
        case override(String)
        case failed(String)
    }

    private static let cliState: CLIState = {
        let override = ProcessInfo.processInfo.environment["DEADRECKON_BIN"] ?? ""
        do {
            _ = try BinaryLocator.locate()
            if !override.isEmpty {
                return .override(override)
            }
            let version = VendoredManifest.load()?.cliVersion ?? "version unknown"
            return .verified(version)
        } catch {
            return .failed((error as? BinaryLocatorError)?.errorDescription
                ?? error.localizedDescription)
        }
    }()

    private var cliVerified: Bool {
        switch Self.cliState {
        case .verified, .override: return true
        case .failed: return false
        }
    }

    @ViewBuilder private var cliRow: some View {
        switch Self.cliState {
        case .verified(let version):
            row(.done, "CLI") {
                Text("verified at launch \u{00B7} \(version)")
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.textSecondary)
            }
        case .override(let path):
            row(.done, "CLI") {
                Text("Dev override in effect: manifest verification is skipped for this binary.")
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.warn)
                Text(path)
                    .font(Theme.mono(10))
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
            }
        case .failed(let words):
            row(.missing, "CLI") {
                Text(words)
                    .font(Theme.monoM)
                    .foregroundStyle(Theme.dangerText)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Open Binaries settings") {
                    settingsSection = SettingsSection.binaries.rawValue
                    openSettings()
                }
                .buttonStyle(.themeStandard(compact: true))
            }
        }
    }

    // MARK: Row 2 — Agent (live probe rows, §C3 atoms)

    private var selectedProbe: ProviderProbeRow? {
        guard case .loaded(let envelope) = catalog.providers, let selectedRoute else {
            return nil
        }
        return envelope.providers.first(where: { $0.id == selectedRoute })
    }

    private var agentGlyph: Glyph {
        switch catalog.providers {
        case .idle, .loading: return .busy
        case .failed: return .missing
        case .loaded:
            return selectedProbe?.status == .ok ? .done : .pending
        }
    }

    @ViewBuilder private var agentRow: some View {
        row(agentGlyph, "Agent") {
            switch catalog.providers {
            case .idle, .loading:
                Text("checking your agents\u{2026}")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
            case .failed(let words):
                Text(words)
                    .font(Theme.monoM)
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            case .loaded(let envelope):
                if envelope.providers.isEmpty {
                    Text("No agents configured yet.")
                        .font(Theme.body(11))
                        .foregroundStyle(Theme.textSecondary)
                    CommandLineView(command: "deadreckon config provider cli:claude-code")
                }
                ForEach(envelope.providers, id: \.id) { probe in
                    agentChoice(probe)
                }
                writeEcho
            }
        }
    }

    /// One agent radio row: selectable when the probe passed; failed probes
    /// stay visible-disabled with the message + try lines verbatim (the New
    /// Goal grammar).
    @ViewBuilder private func agentChoice(_ probe: ProviderProbeRow) -> some View {
        let selected = selectedRoute == probe.id
        Button {
            choose(probe.id)
        } label: {
            HStack(spacing: 8) {
                Image(systemName: selected ? "largecircle.fill.circle" : "circle")
                    .foregroundStyle(selected ? Theme.accent : Theme.textTertiary)
                    .font(.system(size: 12))
                ProviderIcon(provider: probe.id, size: 13)
                Text(Lexicon.agentName(probe.id, displayName: probe.displayName) ?? probe.id)
                    .font(Theme.body(12, weight: .medium))
                    .foregroundStyle(probe.status == .ok ? Theme.textPrimary : Theme.textTertiary)
                Text(probe.id)
                    .font(Theme.mono(10))
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
                        .font(Theme.mono(10))
                        .foregroundStyle(Theme.textSecondary)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
                ForEach(probe.tryLines, id: \.self) { line in
                    HStack(spacing: 5) {
                        Text("try:")
                            .font(Theme.body(10, weight: .semibold))
                            .foregroundStyle(Theme.textTertiary)
                        Text(line)
                            .font(Theme.mono(10))
                            .foregroundStyle(Theme.accent)
                            .textSelection(.enabled)
                    }
                }
            }
            .padding(.leading, 20)
        }
    }

    /// Selecting writes `defaults.provider` through the armed config
    /// surface; the degraded path (older binary, no config envelopes)
    /// keeps the choice local and shows the command to copy instead —
    /// never a dead control, never parsed prose.
    private func choose(_ route: String) {
        selectedRoute = route
        keyText = ""
        if config.envelope != nil {
            Task { await config.set(key: "defaults.provider", value: route) }
        }
    }

    /// The last config write's state, quietly (§S1 grammar): refusals
    /// verbatim; the degraded copy-command well when the surface is not
    /// armed.
    @ViewBuilder private var writeEcho: some View {
        switch config.capability {
        case .degraded:
            if let selectedRoute {
                VStack(alignment: .leading, spacing: 3) {
                    Text("This CLI version has no machine envelopes for config yet \u{2014} set the default in Terminal:")
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.textTertiary)
                    CommandLineView(command: "deadreckon config provider \(selectedRoute)")
                }
            }
        default:
            switch config.writeState {
            case .refused(let refusal):
                RefusalView(refusal: refusal)
            case .failed(let words):
                Text(words)
                    .font(Theme.mono(10))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            case .writing:
                Text("writing\u{2026}")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.textTertiary)
            case .idle, .completed:
                EmptyView()
            }
        }
    }

    // MARK: Row 3 — Key (only for api: routes; stdin-backed, §S2 grammar)

    private var isAPIKeyRoute: Bool {
        selectedRoute?.hasPrefix("api:") == true
    }

    /// Key state from the show envelope's structural redaction only.
    private var keyConfigured: Bool {
        guard let selectedRoute, let envelope = config.envelope else { return false }
        let entry = envelope.providers[selectedRoute]
            ?? envelope.providers[String(selectedRoute.dropFirst("api:".count))]
        if case .object(let fields)? = entry,
           case .string(let marker)? = fields["api_key"] {
            return marker == "configured"
        }
        return false
    }

    private var keyGlyph: Glyph {
        guard let probe = selectedProbe else { return .neutral }
        if !isAPIKeyRoute { return probe.status == .ok ? .done : .neutral }
        return keyConfigured ? .done : .pending
    }

    @ViewBuilder private var keyRow: some View {
        row(keyGlyph, "Key") {
            if selectedRoute == nil {
                Text("pick an agent first")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
            } else if !isAPIKeyRoute {
                Text(Lexicon.subscriptionRouteCaption)
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.textSecondary)
            } else if config.envelope != nil {
                HStack(spacing: 6) {
                    StatusChip(
                        text: keyConfigured ? "configured" : "not configured",
                        color: keyConfigured ? Theme.textSecondary : Theme.textTertiary)
                    SecureField("Paste API key\u{2026}", text: $keyText)
                        .textFieldStyle(.plain)
                        .font(Theme.body(11.5))
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .frame(maxWidth: 220)
                        .inputChrome()
                    Button("Save key") {
                        let secret = keyText
                        keyText = ""
                        Task { await config.saveKey(route: selectedRoute ?? "", secret: secret) }
                    }
                    .buttonStyle(.themeStandard(compact: true))
                    .disabled(keyText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                Text(Lexicon.keyOverStdinCaption)
                    .font(Theme.body(9.5))
                    .foregroundStyle(Theme.textTertiary)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                Text("This CLI version has no key envelope yet \u{2014} paste the key in Terminal (it travels over stdin):")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.textTertiary)
                    .fixedSize(horizontal: false, vertical: true)
                CommandLineView(command: "deadreckon config set-key \(selectedRoute ?? "")")
            }
        }
    }

    // MARK: Row 4 — Background service

    private var serviceGlyph: Glyph {
        switch service.status {
        case .idle, .loading: return .busy
        case .unavailable: return .missing
        case .loaded:
            switch service.displayVerdict {
            case .running: return .done
            case .unknown: return .pending
            default: return .missing
            }
        }
    }

    @ViewBuilder private var serviceRow: some View {
        row(serviceGlyph, "Background service") {
            switch service.status {
            case .idle, .loading:
                Text("checking the service\u{2026}")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
            case .unavailable(let words):
                Text(words)
                    .font(Theme.monoM)
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            case .loaded:
                HStack(spacing: 8) {
                    Text(Lexicon.serviceVerdictWord(service.displayVerdict))
                        .font(Theme.body(11.5, weight: .medium))
                        .foregroundStyle(service.displayVerdict == .running
                            ? Theme.textPrimary : Theme.textSecondary)
                    if service.displayVerdict != .running,
                       service.displayVerdict != .unsupported {
                        Button("Install & start\u{2026}") { installSheetShown = true }
                            .buttonStyle(.themeStandard(compact: true))
                    }
                }
                if service.displayVerdict == .running {
                    Text("claims queued runs, supervises them, records proven cleanup")
                        .font(Theme.body(9.5))
                        .foregroundStyle(Theme.textTertiary)
                }
            }
        }
    }

    // MARK: Row 5 — Prove it (try --json, real and keyless)

    private var proofGlyph: Glyph {
        switch proof.state {
        case .idle: return .pending
        case .running: return .busy
        case .proof: return .done
        case .refused, .failed: return .missing
        }
    }

    @ViewBuilder private var proofRow: some View {
        row(proofGlyph, "Prove it") {
            switch proof.state {
            case .idle:
                HStack(spacing: 8) {
                    Button("Run a test") { Task { await proof.run() } }
                        .buttonStyle(.themeStandard(compact: true))
                    Text("a real tiny run in a throwaway workspace; no credentials needed")
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.textTertiary)
                }
            case .running(let startedAt):
                TimelineView(.periodic(from: startedAt, by: 1)) { context in
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text("Running the proof \u{2014} a real tiny run in a throwaway workspace, no credentials needed\u{2026} \(Int(context.date.timeIntervalSince(startedAt)))s")
                            .font(Theme.body(10.5))
                            .foregroundStyle(Theme.textSecondary)
                            .monospacedDigit()
                    }
                }
            case .proof(let envelope):
                HStack(spacing: 6) {
                    StatusChip(text: "proof signed", color: Theme.success)
                    if let path = envelope.proofPath {
                        Text(path)
                            .font(Theme.mono(9.5))
                            .foregroundStyle(Theme.textSecondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .textSelection(.enabled)
                    }
                }
                // The binary's own trust words, verbatim — the proof row
                // never claims more than the binary claimed.
                Text(envelope.trust)
                    .font(Theme.mono(10))
                    .foregroundStyle(Theme.textTertiary)
                    .textSelection(.enabled)
                if let gate = envelope.gate {
                    Text(gate)
                        .font(Theme.mono(10))
                        .foregroundStyle(Theme.textTertiary)
                        .textSelection(.enabled)
                }
            case .refused(let refusal):
                RefusalView(refusal: refusal)
            case .failed(let words):
                Text(words)
                    .font(Theme.monoM)
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Run it again") { Task { await proof.run() } }
                    .buttonStyle(.themeText(size: 10.5))
            }
        }
    }
}

/// The §R2 chained lifecycle confirm: ONE sheet listing BOTH commands
/// (install, then start). Each leg resolves on its envelope; refusals
/// render verbatim and STOP the chain; the verdict row repaints only from
/// the controller's fresh status re-poll.
private struct InstallAndStartSheet: View {
    @ObservedObject var service: ServiceController
    @Environment(\.dismiss) private var dismiss

    @State private var chainRunning = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Install & start the background service?")
                    .font(Theme.title)
                    .foregroundStyle(Theme.textPrimary)
                Text("The CLI writes a per-user LaunchAgent that starts at login and supervises your runs. Nothing system-wide. Then it starts the service.")
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.textSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            CommandLineView(command: service.commandLine(for: .supervisorInstall))
            CommandLineView(command: service.commandLine(for: .supervisorStart))

            switch service.action {
            case .idle:
                EmptyView()
            case .running(let word):
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("\(word)ing\u{2026}")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textSecondary)
                }
            case .completed(let envelope):
                HStack(spacing: 6) {
                    Image(systemName: "checkmark.circle")
                        .foregroundStyle(Theme.success)
                    Text("\(envelope.action) \u{00B7} \(envelope.result ?? "")")
                        .font(Theme.small)
                        .foregroundStyle(Theme.textSecondary)
                        .textSelection(.enabled)
                }
            case .refused(let refusal):
                RefusalView(refusal: refusal)
            case .failed(let words):
                Text(words)
                    .font(Theme.monoM)
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }

            HStack {
                Spacer()
                Button(dismissTitle) { dismiss() }
                    .buttonStyle(.themeStandard)
                    .keyboardShortcut(.cancelAction)
                Button("Install & Start") {
                    chainRunning = true
                    Task {
                        await service.install()
                        if case .completed = service.action {
                            await service.start()
                        }
                        chainRunning = false
                    }
                }
                .buttonStyle(.themePrimary)
                .disabled(chainRunning || finished)
            }
        }
        .padding(20)
        .frame(width: 520)
        .background(Theme.windowBg)
        .onDisappear { service.resetAction() }
    }

    private var finished: Bool {
        switch service.action {
        case .completed, .refused, .failed: return true
        case .idle, .running: return false
        }
    }

    private var dismissTitle: String {
        finished ? "Close" : "Cancel"
    }
}
