import AppKit
import DeadreckonKit
import SwiftUI

/// Settings (⌘,) rebuilt per SETTINGS-SCREENS-SPEC §S0–S6: a two-pane
/// fixed window (840×600) — 200px sidebar of six sections, scrolling
/// content pane of bordered panel cards. Every config value is file truth
/// from `config show --json`; every write dispatches through
/// MutationRunner and re-reads; nothing renders optimistically. Capability
/// probes decide what arms: against the vendored 0.8.4 binary (no config
/// envelopes) General/Agents degrade to honest read-only + terminal
/// handoff, never dead controls.
struct SettingsView: View {
    @ObservedObject var fleet: FleetStore
    @ObservedObject var attention: AttentionCenter
    @StateObject private var config: ConfigStore
    @StateObject private var service: ServiceController
    @StateObject private var doctor: DoctorStore
    @StateObject private var catalog: LayCourseCatalog

    /// §S0 deep link: the sidebar health footer and notification copy set
    /// this before `openSettings()`. The only navigation state.
    @AppStorage("settings.section") private var sectionRaw = SettingsSection.general.rawValue

    @MainActor
    init(fleet: FleetStore, attention: AttentionCenter) {
        self.fleet = fleet
        self.attention = attention
        _config = StateObject(wrappedValue: ConfigStore(cli: WriteCLI.client))
        _service = StateObject(wrappedValue: ServiceController(cli: WriteCLI.client))
        _doctor = StateObject(wrappedValue: DoctorStore(cli: WriteCLI.client))
        _catalog = StateObject(wrappedValue: LayCourseCatalog(cli: WriteCLI.client))
    }

    private var section: SettingsSection {
        SettingsSection(rawValue: sectionRaw) ?? .general
    }

    var body: some View {
        HStack(spacing: 0) {
            sidebar
            Rectangle()
                .fill(Theme.border)
                .frame(width: 1)
            content
        }
        .frame(width: 840, height: 600)
        .background(Theme.windowBg)
        // The one accent is also the control tint: AppKit-owned switches
        // and checkboxes otherwise render system blue — a foreign hue in
        // the committed world (DESIGN §2: exactly one accent).
        .tint(Theme.accent)
        .task {
            // One sweep on open: config probe, service verdict, doctor,
            // provider catalog. Each degrades independently.
            async let a: Void = config.load()
            async let b: Void = service.refreshStatus()
            async let c: Void = doctor.run()
            async let d: Void = catalog.load()
            _ = await (a, b, c, d)
        }
    }

    // MARK: - Sidebar (§S0)

    private var sidebar: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(SettingsSection.allCases) { candidate in
                SettingsSidebarRow(
                    section: candidate,
                    selected: candidate == section) {
                    sectionRaw = candidate.rawValue
                }
            }
            Spacer()
        }
        .padding(10)
        .frame(width: 200, alignment: .leading)
        .background(Theme.sidebarBg)
    }

    @ViewBuilder private var content: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                switch section {
                case .general:
                    GeneralSettingsSection(config: config, catalog: catalog, doctor: doctor)
                case .agents:
                    AgentsKeysSection(config: config, catalog: catalog)
                case .service:
                    ServiceSettingsSection(service: service)
                case .notifications:
                    NotificationsSettingsSection(attention: attention)
                case .binaries:
                    BinariesSettingsSection(fleet: fleet, doctor: doctor)
                case .health:
                    HealthSettingsSection(doctor: doctor)
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Theme.windowBg)
    }
}

/// §S0: the six sections. Raw values are the deep-link vocabulary
/// (`settings.section`); the sidebar health footer writes "health".
enum SettingsSection: String, CaseIterable, Identifiable {
    case general
    case agents
    case service
    case notifications
    case binaries
    case health

    var id: String { rawValue }

    var title: String {
        switch self {
        case .general: return "General"
        case .agents: return "Agents & Keys"
        case .service: return "Service"
        case .notifications: return "Notifications"
        case .binaries: return "Binaries"
        case .health: return "Health"
        }
    }

    var symbol: String {
        switch self {
        case .general: return "gearshape"
        case .agents: return "person.text.rectangle"
        case .service: return "arrow.triangle.2.circlepath"
        case .notifications: return "bell"
        case .binaries: return "shippingbox"
        case .health: return "stethoscope"
        }
    }
}

/// One 32px sidebar row: SF symbol 13pt medium + label 13 medium.
/// Selection = well fill + borderHover (DESIGN §5 row rules — accent is
/// never selection).
private struct SettingsSidebarRow: View {
    let section: SettingsSection
    let selected: Bool
    let action: () -> Void

    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: section.symbol)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(selected ? Theme.textPrimary : Theme.textSecondary)
                    .frame(width: 18)
                Text(section.title)
                    .font(Theme.body(13, weight: .medium))
                    .foregroundStyle(selected ? Theme.textPrimary : Theme.textSecondary)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8)
            .frame(height: 32)
            .background(
                selected ? Theme.well : (hovering ? Theme.panelHover : .clear),
                in: RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous)
                .strokeBorder(selected ? Theme.borderHover : .clear, lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.tactile)
        .onHover { hovering = $0 }
    }
}

// MARK: - Shared settings atoms

/// Scan-first group header (DESIGN §5): the one section title in
/// textSecondary.
func settingsGroupTitle(_ text: String) -> some View {
    Theme.sectionTitle(text, color: Theme.textSecondary)
}

/// The shared info-row grammar (§S0): 170px label column, mono values,
/// selectable, middle-truncated.
struct SettingsInfoRow: View {
    let label: String
    let value: String
    var detail: String?
    var mono = false
    var valueColor: Color = Theme.textPrimary

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(label)
                    .font(Theme.body(11, weight: .semibold))
                    .foregroundStyle(Theme.textSecondary)
                    .frame(width: 170, alignment: .leading)
                Text(value)
                    .font(mono ? Theme.mono(10.5) : Theme.body(12))
                    .foregroundStyle(valueColor)
                    .textSelection(.enabled)
                    .lineLimit(2)
                    .truncationMode(.middle)
            }
            if let detail {
                Text(detail)
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
                    .padding(.leading, 178)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// Reveal one path in Finder (Binaries/General reveal affordances).
@MainActor
func revealInFinder(_ path: String) {
    NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
}

/// A generic menu control in the input chrome: options render
/// visible-disabled with their note verbatim (the SwiftUI Picker cannot
/// disable individual items, and the sandbox row must show unavailable
/// backends with the doctor note — §S1).
struct OptionMenu: View {
    struct Option: Identifiable {
        let id: String
        let title: String
        var note: String?
        var disabled = false
    }

    let options: [Option]
    let currentTitle: String
    let onSelect: (String) -> Void

    var body: some View {
        Menu {
            ForEach(options) { option in
                Button {
                    onSelect(option.id)
                } label: {
                    if let note = option.note, !note.isEmpty {
                        Text("\(option.title) \u{2014} \(note)")
                    } else {
                        Text(option.title)
                    }
                }
                .disabled(option.disabled)
            }
        } label: {
            HStack(spacing: 6) {
                Text(currentTitle)
                    .font(Theme.body(12))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                Spacer(minLength: 0)
                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(size: 9, weight: .medium))
                    .foregroundStyle(Theme.textTertiary)
            }
            .padding(.horizontal, 8)
            .frame(height: 24)
            .contentShape(Rectangle())
        }
        .menuStyle(.borderlessButton)
        .frame(width: 220)
        .inputChrome()
    }
}

// MARK: - §S1 General

/// Defaults every new goal inherits. Values are file truth from
/// `config show --json`; every control dispatches set/unset and re-reads.
/// Degraded (older binary): a banner + terminal handoff, no dead controls
/// — and no value rows, because the app reads config truth only through
/// the binary's redacted envelope (raw config.toml could carry key bytes).
struct GeneralSettingsSection: View {
    @ObservedObject var config: ConfigStore
    @ObservedObject var catalog: LayCourseCatalog
    @ObservedObject var doctor: DoctorStore

    @State private var spendText = ""
    @State private var hoursText = ""
    @State private var advancedOpen = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            settingsGroupTitle("General \u{2014} defaults every new goal inherits")
            switch config.capability {
            case .unknown, .probing:
                probing
            case .degraded(let words):
                ConfigDegradedBanner(words: words)
            case .armed(let envelope):
                VStack(alignment: .leading, spacing: 14) {
                    agentRow(envelope)
                    modelRow(envelope)
                    spendRow(envelope)
                    timeRow(envelope)
                    sandboxRow(envelope)
                    preventSleepRow(envelope)
                    Divider().overlay(Theme.border)
                    lastCommandWell
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .cardChrome()
                advanced(envelope)
            }
            StartupCard()
        }
        .onAppear { seedFields() }
        .onChange(of: config.envelope?.displayValue("defaults.max_spend")) { _, _ in
            seedFields()
        }
        .onChange(of: config.envelope?.displayValue("defaults.cli_max_wall_seconds")) { _, _ in
            seedFields()
        }
    }

    private var probing: some View {
        Text("Reading the configuration\u{2026}")
            .font(Theme.small)
            .foregroundStyle(Theme.textTertiary)
    }

    private func seedFields() {
        guard let envelope = config.envelope else { return }
        spendText = envelope.displayValue("defaults.max_spend") ?? ""
        if let secondsText = envelope.displayValue("defaults.cli_max_wall_seconds"),
           let seconds = Double(secondsText) {
            let hours = seconds / 3600
            hoursText = hours == hours.rounded()
                ? String(Int(hours)) : String(format: "%.1f", hours)
        } else {
            hoursText = ""
        }
    }

    // MARK: rows

    private func write(_ key: String, _ value: String?) {
        Task {
            if let value {
                await config.set(key: key, value: value)
            } else {
                await config.unset(key: key)
            }
        }
    }

    private func agentRow(_ envelope: ConfigShowEnvelope) -> some View {
        let current = envelope.settings["defaults.provider"].flatMap {
            $0.isSet ? ConfigShowEnvelope.display($0.value) : nil
        }
        var options: [OptionMenu.Option] = [.init(id: "", title: "No default")]
        if case .loaded(let probes) = catalog.providers {
            options += probes.providers.map { probe in
                let name = Lexicon.agentName(probe.id, displayName: probe.displayName)
                return OptionMenu.Option(
                    id: probe.id,
                    title: name.map { "\($0)  \(probe.id)" } ?? probe.id,
                    note: probe.status == .ok ? nil : Lexicon.probeWord(probe.status),
                    disabled: false)
            }
        } else if let current {
            options.append(.init(id: current, title: current))
        }
        let currentTitle = current.map { id in
            Lexicon.agentName(id).map { "\($0)  \(id)" } ?? id
        } ?? "No default"
        return configRow(key: "defaults.provider", caption: nil) {
            OptionMenu(options: options, currentTitle: currentTitle) { id in
                write("defaults.provider", id.isEmpty ? nil : id)
            }
        }
    }

    private func modelRow(_ envelope: ConfigShowEnvelope) -> some View {
        let agent = envelope.settings["defaults.provider"].flatMap {
            $0.isSet ? ConfigShowEnvelope.display($0.value) : nil
        }
        let current = envelope.settings["defaults.model"].flatMap {
            $0.isSet ? ConfigShowEnvelope.display($0.value) : nil
        }
        var options: [OptionMenu.Option] = [.init(id: "", title: "Agent\u{2019}s default")]
        options += catalog.modelChoices(for: agent).map {
            .init(id: $0.id, title: $0.id, note: $0.recommended ? "recommended" : nil)
        }
        if let current, !options.contains(where: { $0.id == current }) {
            options.append(.init(id: current, title: current))
        }
        return configRow(key: "defaults.model", caption: nil) {
            OptionMenu(options: options, currentTitle: current ?? "Agent\u{2019}s default") { id in
                write("defaults.model", id.isEmpty ? nil : id)
            }
        }
    }

    private func spendRow(_ envelope: ConfigShowEnvelope) -> some View {
        configRow(key: "defaults.max_spend", caption: Lexicon.spendCapCaption) {
            HStack(spacing: 4) {
                Text("$")
                    .font(Theme.body(12))
                    .foregroundStyle(Theme.textSecondary)
                TextField("10", text: $spendText)
                    .textFieldStyle(.plain)
                    .font(Theme.mono(12))
                    .foregroundStyle(Theme.textPrimary)
                    .multilineTextAlignment(.trailing)
                    .frame(width: 70)
                    .onSubmit {
                        let trimmed = spendText.trimmingCharacters(in: .whitespaces)
                        guard !trimmed.isEmpty, Double(trimmed) != nil else {
                            seedFields()
                            return
                        }
                        write("defaults.max_spend", trimmed)
                    }
            }
            .padding(.horizontal, 8)
            .frame(height: 24)
            .inputChrome()
        }
    }

    private func timeRow(_ envelope: ConfigShowEnvelope) -> some View {
        configRow(key: "defaults.cli_max_wall_seconds",
                  caption: Lexicon.timeLimitCaption) {
            HStack(spacing: 6) {
                TextField("10", text: $hoursText)
                    .textFieldStyle(.plain)
                    .font(Theme.mono(12))
                    .foregroundStyle(Theme.textPrimary)
                    .multilineTextAlignment(.trailing)
                    .frame(width: 50)
                    .onSubmit {
                        guard let hours = Double(hoursText.trimmingCharacters(in: .whitespaces)),
                              hours > 0 else {
                            seedFields()
                            return
                        }
                        // Stored in seconds; the field converts (§S1).
                        write("defaults.cli_max_wall_seconds", String(Int(hours * 3600)))
                    }
                Text("hours")
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.textSecondary)
            }
            .padding(.horizontal, 8)
            .frame(height: 24)
            .inputChrome()
        }
    }

    private func sandboxRow(_ envelope: ConfigShowEnvelope) -> some View {
        let current = envelope.displayValue("defaults.sandbox") ?? "auto"
        var options: [OptionMenu.Option] = [.init(id: "auto", title: "auto")]
        for sandbox in doctor.report?.sandboxes ?? [] {
            // The doctor note rides verbatim; unavailable backends stay
            // visible-disabled; `none` always carries its own warning.
            let note = sandbox.available
                ? (sandbox.backend == "none" ? sandbox.note : nil)
                : sandbox.note
            options.append(.init(
                id: sandbox.backend, title: sandbox.backend,
                note: note, disabled: !sandbox.available))
        }
        if !options.contains(where: { $0.id == current }) {
            options.append(.init(id: current, title: current))
        }
        return configRow(key: "defaults.sandbox", caption: nil) {
            OptionMenu(options: options, currentTitle: current) { id in
                write("defaults.sandbox", id)
            }
        }
    }

    private func preventSleepRow(_ envelope: ConfigShowEnvelope) -> some View {
        let current = envelope.settings["defaults.prevent_sleep"].flatMap {
            $0.isSet ? ConfigShowEnvelope.display($0.value) : nil
        }
        let options: [OptionMenu.Option] = [
            .init(id: "auto", title: "auto"),
            .init(id: "on", title: "on"),
            .init(id: "off", title: "off"),
        ]
        return configRow(key: "defaults.prevent_sleep",
                         caption: Lexicon.preventSleepCaption) {
            OptionMenu(options: options, currentTitle: current ?? "auto") { id in
                write("defaults.prevent_sleep", id)
            }
        }
    }

    /// One §S1 row: label 13 medium / key mono 10.5 beneath / control
    /// right-aligned; caption under; the row's exact command in the
    /// tooltip (quiet provenance).
    private func configRow<Control: View>(
        key: String, caption: String?, @ViewBuilder control: () -> Control
    ) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(alignment: .center, spacing: 8) {
                VStack(alignment: .leading, spacing: 1) {
                    Text(Lexicon.configRowLabel(key))
                        .font(Theme.body(13, weight: .medium))
                        .foregroundStyle(Theme.textPrimary)
                    Text(key)
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.textTertiary)
                        .textSelection(.enabled)
                }
                Spacer(minLength: 12)
                if isWriting(key) {
                    Text("writing\u{2026}")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textTertiary)
                }
                control()
            }
            .help("deadreckon config set \(key) <value> --json")
            if let caption {
                Text(caption)
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if case .refused(let refusal) = config.writeState, refusalTarget(refusal) == key {
                RefusalView(refusal: refusal)
            }
        }
    }

    private func isWriting(_ key: String) -> Bool {
        if case .writing(let writingKey) = config.writeState { return writingKey == key }
        return false
    }

    private func refusalTarget(_ refusal: ErrorEnvelope) -> String? {
        // The refusal message names the key; render inline on the row it
        // belongs to, else in the shared well below.
        if case .writing = config.writeState { return nil }
        for key in ["defaults.provider", "defaults.model", "defaults.max_spend",
                    "defaults.cli_max_wall_seconds", "defaults.sandbox",
                    "defaults.prevent_sleep"] where refusal.message.contains(key) {
            return key
        }
        return nil
    }

    /// The shared command well: the LAST dispatched config command + its
    /// verdict word (§S1).
    @ViewBuilder private var lastCommandWell: some View {
        if let command = config.lastCommand {
            VStack(alignment: .leading, spacing: 4) {
                CommandLineView(command: command)
                switch config.writeState {
                case .completed(_, let word):
                    Text(word)
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textSecondary)
                case .failed(let words):
                    Text(words)
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.warn)
                        .textSelection(.enabled)
                case .refused(let refusal):
                    if refusalTarget(refusal) == nil {
                        RefusalView(refusal: refusal)
                    }
                case .writing, .idle:
                    EmptyView()
                }
            }
        } else {
            Text("Changes dispatch `deadreckon config set` and re-read the file.")
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.textTertiary)
        }
    }

    /// ADVANCED — the whole file (§S1): the complete configuration as the
    /// binary reports it, secrets structurally redacted. The app never
    /// renders raw config.toml — raw bytes could carry key material.
    private func advanced(_ envelope: ConfigShowEnvelope) -> some View {
        DisclosureGroup(isExpanded: $advancedOpen) {
            VStack(alignment: .leading, spacing: 8) {
                ScrollView {
                    Text(Self.prettyJSON(envelope.file) ?? "empty")
                        .font(Theme.monoS)
                        .foregroundStyle(Theme.textPrimary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(8)
                }
                .frame(maxHeight: 180)
                .background(Theme.well,
                            in: RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous)
                    .strokeBorder(Theme.border, lineWidth: 1))
                HStack(spacing: 8) {
                    Text(envelope.configPath)
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.textSecondary)
                        .textSelection(.enabled)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Button("Reveal in Finder") { revealInFinder(envelope.configPath) }
                        .buttonStyle(.themeStandard(compact: true))
                }
                Text("Everything the CLI reads, as `config show --json` reports it \u{2014} "
                     + "API keys are redacted to \u{201C}configured\u{201D}. Keys not surfaced "
                     + "above (`defaults.doc_*`, `defaults.plain`, `defaults.start_attach`, "
                     + "\u{2026}) are TUI- or doc-pipeline-scoped and are edited with "
                     + "`deadreckon config set` in Terminal.")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.top, 8)
        } label: {
            Theme.sectionTitle("Advanced \u{2014} the whole file")
        }
        .tint(Theme.textTertiary)
    }

    static func prettyJSON(_ value: JSONValue?) -> String? {
        guard let value,
              let data = try? JSONEncoder().encode(value),
              let object = try? JSONSerialization.jsonObject(with: data),
              let pretty = try? JSONSerialization.data(
                  withJSONObject: object, options: [.prettyPrinted, .sortedKeys])
        else { return nil }
        return String(decoding: pretty, as: UTF8.self)
    }
}

/// §S1 degraded: this CLI version has no machine envelopes for config.
/// The binary's words verbatim + the terminal handoff, copy affordance.
struct ConfigDegradedBanner: View {
    let words: String

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("This CLI version has no machine envelopes for config yet \u{2014} "
                 + "edit with `deadreckon config set KEY VALUE` in Terminal.")
                .font(Theme.small)
                .foregroundStyle(Theme.warn)
                .fixedSize(horizontal: false, vertical: true)
            Text(words)
                .font(Theme.mono(10.5))
                .foregroundStyle(Theme.textSecondary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                CommandLineView(command: "deadreckon config set KEY VALUE")
                Button("Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(
                        "deadreckon config set KEY VALUE", forType: .string)
                }
                .buttonStyle(.themeStandard(compact: true))
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }
}

// MARK: - §S2 Agents & Keys

/// One bordered row-card per route from `providers list --json` +
/// `models --json` (both real today). Key state and key writes ride the
/// config envelopes; on an older binary the key controls degrade to the
/// stdin-safe terminal command.
struct AgentsKeysSection: View {
    @ObservedObject var config: ConfigStore
    @ObservedObject var catalog: LayCourseCatalog

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            settingsGroupTitle("Agents")
            switch catalog.providers {
            case .idle, .loading:
                Text("Probing agents\u{2026}")
                    .font(Theme.small)
                    .foregroundStyle(Theme.textTertiary)
            case .failed(let words):
                VStack(alignment: .leading, spacing: 6) {
                    Text("Agents unavailable \u{2014} the probe said:")
                        .font(Theme.small)
                        .foregroundStyle(Theme.warn)
                    Text(words)
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.textSecondary)
                        .textSelection(.enabled)
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .cardChrome()
            case .loaded(let envelope) where envelope.providers.isEmpty:
                emptyState
            case .loaded(let envelope):
                ForEach(envelope.providers, id: \.id) { probe in
                    AgentRouteCard(probe: probe, config: config, catalog: catalog)
                }
                rolesCard
            }
        }
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("No agents configured yet.")
                .font(Theme.heading)
                .foregroundStyle(Theme.textPrimary)
            Text("Pick an agent in the first-run panel, or configure one in Terminal.")
                .font(Theme.base)
                .foregroundStyle(Theme.textSecondary)
            CommandLineView(command: "deadreckon config provider cli:claude-code")
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    /// ROLES (§S2): reviewer agent + reviewer model — agent routing, not
    /// launch defaults.
    private var rolesCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            settingsGroupTitle("Roles")
            reviewerRow(key: "defaults.reviewer_provider")
            reviewerRow(key: "defaults.reviewer_model")
            Text(Lexicon.reviewerRolesCaption)
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.textTertiary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    @ViewBuilder private func reviewerRow(key: String) -> some View {
        let current = config.envelope?.settings[key].flatMap {
            $0.isSet ? ConfigShowEnvelope.display($0.value) : nil
        }
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 1) {
                Text(Lexicon.configRowLabel(key))
                    .font(Theme.body(13, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                Text(key)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textTertiary)
            }
            Spacer(minLength: 12)
            if config.envelope != nil {
                OptionMenu(
                    options: reviewerOptions(key: key, current: current),
                    currentTitle: current ?? "No override") { id in
                    Task {
                        if id.isEmpty {
                            await config.unset(key: key)
                        } else {
                            await config.set(key: key, value: id)
                        }
                    }
                }
            } else {
                Text(current ?? "not readable on this CLI")
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textTertiary)
            }
        }
    }

    private func reviewerOptions(key: String, current: String?) -> [OptionMenu.Option] {
        var options: [OptionMenu.Option] = [.init(id: "", title: "No override")]
        if key.hasSuffix("provider"), case .loaded(let probes) = catalog.providers {
            options += probes.providers.map { .init(id: $0.id, title: $0.id) }
        } else if key.hasSuffix("model") {
            let agent = config.envelope?.settings["defaults.reviewer_provider"].flatMap {
                $0.isSet ? ConfigShowEnvelope.display($0.value) : nil
            }
            options += catalog.modelChoices(for: agent).map { .init(id: $0.id, title: $0.id) }
        }
        if let current, !options.contains(where: { $0.id == current }) {
            options.append(.init(id: current, title: current))
        }
        return options
    }
}

/// One agent route card (§S2): mark + name + route id + probe chip +
/// [Test]; model default; key state per route kind.
private struct AgentRouteCard: View {
    let probe: ProviderProbeRow
    @ObservedObject var config: ConfigStore
    @ObservedObject var catalog: LayCourseCatalog

    @State private var keyDraft = ""
    @State private var confirmRemoveKey = false
    @State private var testing = false

    private var isAPIKeyRoute: Bool {
        probe.id.hasPrefix("api:")
            || (config.envelope?.keyConfigured(route: probe.id) ?? false)
    }

    private var chipColor: Color {
        switch probe.status {
        case .ok: return Theme.success
        case .failed: return Theme.warn
        case .skipped, .unknown: return Theme.textSecondary
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                ProviderIcon(provider: probe.id, size: 16)
                if let name = Lexicon.agentName(probe.id, displayName: probe.displayName) {
                    Text(name)
                        .font(Theme.body(13, weight: .medium))
                        .foregroundStyle(Theme.textPrimary)
                }
                Text(probe.id)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textSecondary)
                    .textSelection(.enabled)
                StatusChip(text: Lexicon.probeWord(probe.status), color: chipColor)
                Spacer()
                Button(testing ? "Testing\u{2026}" : "Test") {
                    testing = true
                    Task {
                        await catalog.load()
                        testing = false
                    }
                }
                .buttonStyle(.themeStandard(compact: true))
                .disabled(testing)
            }
            if probe.status == .failed, let message = probe.message {
                Text(message)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textPrimary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                ForEach(probe.tryLines, id: \.self) { line in
                    HStack(spacing: 5) {
                        Text("try:")
                            .font(Theme.body(10, weight: .semibold))
                            .foregroundStyle(Theme.textTertiary)
                        Text(line)
                            .font(Theme.mono(10.5))
                            .foregroundStyle(Theme.accent)
                            .textSelection(.enabled)
                    }
                }
            }
            modelDefaultRow
            keyRow
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
        .sheet(isPresented: $confirmRemoveKey) {
            RemoveKeySheet(route: probe.id, config: config)
        }
    }

    /// Model default per route: writes `providers.<route>.model` (SHIPPED
    /// grammar — the validated provider entry field; the text-only
    /// `config model` verb never grew an envelope).
    @ViewBuilder private var modelDefaultRow: some View {
        let choices = catalog.modelChoices(for: probe.id)
        if !choices.isEmpty {
            let key = "providers.\(probe.id).model"
            let current = currentRouteModel
            HStack(spacing: 8) {
                Text("Model default")
                    .font(Theme.body(11, weight: .semibold))
                    .foregroundStyle(Theme.textSecondary)
                Spacer()
                if config.envelope != nil {
                    OptionMenu(
                        options: [.init(id: "", title: "Agent\u{2019}s default")]
                            + choices.map {
                                .init(id: $0.id, title: $0.id,
                                      note: $0.recommended ? "recommended" : nil)
                            },
                        currentTitle: current ?? "Agent\u{2019}s default") { id in
                        Task {
                            if id.isEmpty {
                                await config.unset(key: key)
                            } else {
                                await config.set(key: key, value: id)
                            }
                        }
                    }
                } else {
                    Text(current ?? "Agent\u{2019}s default")
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.textTertiary)
                }
            }
        }
    }

    private var currentRouteModel: String? {
        guard case .object(let entry)? = config.envelope?.providers[probe.id],
              case .string(let model)? = entry["model"] else { return nil }
        return model
    }

    @ViewBuilder private var keyRow: some View {
        if !isAPIKeyRoute {
            Text(Lexicon.subscriptionRouteCaption)
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.textTertiary)
        } else if let envelope = config.envelope {
            let configured = envelope.keyConfigured(route: probe.id)
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    Text("Key")
                        .font(Theme.body(11, weight: .semibold))
                        .foregroundStyle(Theme.textSecondary)
                    StatusChip(
                        text: configured ? "configured" : "not configured",
                        color: configured ? Theme.textSecondary : Theme.textTertiary)
                    if envelope.keyFromEnvironment(route: probe.id), !configured {
                        Text("from env")
                            .font(Theme.body(10.5))
                            .foregroundStyle(Theme.textTertiary)
                    }
                    Spacer()
                    if configured {
                        Button("Remove key\u{2026}") { confirmRemoveKey = true }
                            .buttonStyle(ThemeButtonStyle(kind: .quietDanger, compact: true))
                    }
                }
                HStack(spacing: 8) {
                    SecureField("Paste API key\u{2026}", text: $keyDraft)
                        .textFieldStyle(.plain)
                        .font(Theme.body(12))
                        .foregroundStyle(Theme.textPrimary)
                        .padding(.horizontal, 8)
                        .frame(height: 24)
                        .frame(maxWidth: 280)
                        .inputChrome()
                    Button("Save key") {
                        let secret = keyDraft
                        // The draft clears immediately; the secret exists
                        // only inside this dispatch (redaction rule).
                        keyDraft = ""
                        Task { await config.saveKey(route: probe.id, secret: secret) }
                    }
                    .buttonStyle(.themeStandard(compact: true))
                    .disabled(keyDraft.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                CommandLineView(
                    command: "deadreckon config set-key \(probe.id) --json")
                Text(Lexicon.keyOverStdinCaption)
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        } else {
            // Older binary: no set-key envelope — honest terminal handoff,
            // stdin-safe as documented by the binary itself.
            VStack(alignment: .leading, spacing: 4) {
                Text("This CLI version stores keys from Terminal only:")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
                CommandLineView(
                    command: "printf '%s' \"$KEY\" | deadreckon config set-key \(probe.id)")
            }
        }
    }
}

/// Remove key confirm (§S2): quiet-danger opens it; the destructive
/// confirm dispatches `config unset-key` and resolves from the envelope.
private struct RemoveKeySheet: View {
    let route: String
    @ObservedObject var config: ConfigStore
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Remove the stored API key?")
                .font(Theme.title)
                .foregroundStyle(Theme.textPrimary)
            Text("The route stops authenticating until a new key is stored or its "
                 + "environment variable is set. Nothing else changes.")
                .font(Theme.base)
                .foregroundStyle(Theme.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
            CommandLineView(command: config.commandLine(for: .configUnsetKey(route: route)))
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(.themeStandard)
                Button("Remove Key") {
                    Task {
                        await config.removeKey(route: route)
                        dismiss()
                    }
                }
                .buttonStyle(.themeDangerConfirm)
            }
        }
        .padding(20)
        .frame(width: 520)
        .background(Theme.windowBg)
    }
}

// MARK: - §S4 Notifications (existing behavior, section card)

/// Startup + appearance facts (kept from the old General tab — simplify,
/// never remove).
struct StartupCard: View {
    @State private var launchAtLogin = LaunchAtLogin.isEnabled
    @State private var launchAtLoginIssue: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            settingsGroupTitle("Startup")
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Launch at login")
                        .font(Theme.body(13, weight: .medium))
                        .foregroundStyle(Theme.textPrimary)
                    Text("Keep deadreckon in your menu bar so your runs are watched and decisions reach you.")
                        .font(Theme.body(11))
                        .foregroundStyle(Theme.textSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 12)
                Toggle("Launch at login", isOn: $launchAtLogin)
                    .labelsHidden()
                    .toggleStyle(.switch)
            }
            .onChange(of: launchAtLogin) { _, enabled in
                // Re-entrancy guard: reverting the toggle after a failure
                // re-fires onChange with the real state; a no-op
                // transition never touches SMAppService.
                guard enabled != LaunchAtLogin.isEnabled else { return }
                do {
                    try LaunchAtLogin.setEnabled(enabled)
                    launchAtLoginIssue = nil
                } catch {
                    launchAtLoginIssue = error.localizedDescription
                    launchAtLogin = LaunchAtLogin.isEnabled
                }
            }
            if let issue = launchAtLoginIssue {
                Text(issue)
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            }
            Text("deadreckon is dark by design. There is no light mode.")
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.textTertiary)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }
}

struct NotificationsSettingsSection: View {
    @ObservedObject var attention: AttentionCenter
    @State private var preferences = AttentionPreferences.load(from: .standard)

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            settingsGroupTitle("Notifications")
            VStack(alignment: .leading, spacing: 12) {
                toggleRow(
                    "Notify me when a run needs attention",
                    detail: "Derived from the attention entries the CLI writes to `notify.jsonl`. Signals only \u{2014} the app re-reads the files when you open it.",
                    isOn: masterBinding)

                Divider().overlay(Theme.border)

                ForEach(AttentionPreferences.notifiableReasons, id: \.self) { reason in
                    toggleRow(AttentionDerivation.title(for: reason),
                              detail: reasonDetail(reason),
                              isOn: reasonBinding(reason))
                        .disabled(!preferences.masterEnabled)
                }

                Divider().overlay(Theme.border)

                Text("macOS permission is requested the first time a notification is delivered. If you declined it, enable deadreckon under System Settings > Notifications; a grant there takes effect on the next notification, no relaunch needed.")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
                    .fixedSize(horizontal: false, vertical: true)

                if !attention.issues.isEmpty {
                    Divider().overlay(Theme.border)
                    settingsGroupTitle("Notification trouble")
                    ForEach(attention.issues.sorted(by: { $0.key < $1.key }), id: \.key) { jobID, reason in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(jobID)
                                .font(Theme.monoM)
                                .foregroundStyle(Theme.textPrimary)
                                .textSelection(.enabled)
                            Text("Notifications from this run are stopped: \(reason)")
                                .font(Theme.body(10.5))
                                .foregroundStyle(Theme.warn)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .cardChrome()
        }
    }

    private var masterBinding: Binding<Bool> {
        Binding(
            get: { preferences.masterEnabled },
            set: { enabled in
                preferences.masterEnabled = enabled
                preferences.save(to: .standard)
            })
    }

    private func reasonBinding(_ reason: OperatorAttentionReason) -> Binding<Bool> {
        Binding(
            get: { preferences.enabledReasons.contains(reason) },
            set: { enabled in
                if enabled {
                    preferences.enabledReasons.insert(reason)
                } else {
                    preferences.enabledReasons.remove(reason)
                }
                preferences.save(to: .standard)
            })
    }

    private func reasonDetail(_ reason: OperatorAttentionReason) -> String {
        switch reason {
        case .verifiedAwaitingPromote:
            return "Both sign-offs landed; the result waits for your approval."
        case .pausedAtCap:
            return "The run paused at a budget or time limit."
        case .waitingInput:
            return "The run waits for your review before it can continue."
        case .blocked:
            return "The service classified the run as blocked."
        case .failed:
            return "The service classified the run as failed."
        case .cancelled:
            return "The service classified the run as stopped."
        case .unknown:
            return "unknown"
        }
    }

    private func settingLabel(_ title: String, detail: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(Theme.body(13, weight: .medium))
                .foregroundStyle(Theme.textPrimary)
            Text(detail)
                .font(Theme.body(11))
                .foregroundStyle(Theme.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// One toggle row: label + detail leading, switch pinned to the card's
    /// trailing edge — the switches align in one column instead of ragging
    /// after each label's text.
    private func toggleRow(_ title: String, detail: String,
                           isOn: Binding<Bool>) -> some View {
        HStack(alignment: .top, spacing: 12) {
            settingLabel(title, detail: detail)
            Spacer(minLength: 12)
            Toggle(title, isOn: isOn)
                .labelsHidden()
                .toggleStyle(.switch)
        }
    }
}
