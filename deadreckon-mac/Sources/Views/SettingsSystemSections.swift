import AppKit
import DeadreckonKit
import SwiftUI

// SETTINGS-SCREENS-SPEC §S3 (Service), §S5 (Binaries), §S6 (Health).
// Every state renders from re-reads: lifecycle sheets resolve on their
// envelope, then the verdict repaints only from a fresh `supervisor
// status --json` poll; repair outcomes come from the report's own
// repairs[] rows. Degradations quote the binary verbatim with the CLI
// escape hatch named.

// MARK: - §S3 Service

struct ServiceSettingsSection: View {
    @ObservedObject var service: ServiceController

    private enum PendingConfirm: String, Identifiable {
        case install, start, stop
        var id: String { rawValue }
    }

    @State private var confirm: PendingConfirm?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            settingsGroupTitle("Background service")
            VStack(alignment: .leading, spacing: 12) {
                verdictRow
                evidence
                buttons
                Text(Lexicon.serviceCaption)
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .cardChrome()
        }
        .sheet(item: $confirm) { pending in
            switch pending {
            case .install:
                ServiceLifecycleSheet(
                    title: "Install the background service?",
                    body: "The CLI writes a per-user LaunchAgent that starts at login and supervises your runs. Nothing system-wide.",
                    verb: .supervisorInstall,
                    confirmLabel: "Install",
                    service: service)
            case .start:
                ServiceLifecycleSheet(
                    title: "Start the background service?",
                    body: "The CLI enables and starts the installed per-user service; the service manager then owns the supervisor process.",
                    verb: .supervisorStart,
                    confirmLabel: "Start",
                    service: service)
            case .stop:
                ServiceStopSheet(service: service)
            }
        }
    }

    // MARK: verdict (one honest word + the reason verbatim)

    private var verdictRow: some View {
        let verdict = service.displayVerdict
        return VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                StateDot(color: dotColor(verdict))
                Text(Lexicon.serviceVerdictWord(verdict))
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                    .help(unknownReason ?? "")
            }
            if let reason = loadedReason {
                Text(reason)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textSecondary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if case .unavailable(let words) = service.status {
                // Degraded (§S3): verdict Unknown + the stderr words
                // verbatim + the terminal escape hatch.
                Text(words)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textPrimary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 8) {
                    Text("check `deadreckon supervisor status` in Terminal")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textTertiary)
                    Button("Copy") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(
                            "deadreckon supervisor status", forType: .string)
                    }
                    .buttonStyle(.themeStandard(compact: true))
                }
            }
        }
    }

    private var loadedReason: String? {
        guard case .loaded(let report) = service.status else { return nil }
        return report.verdictReason
    }

    private var unknownReason: String? {
        if case .unavailable(let words) = service.status { return words }
        return nil
    }

    private func dotColor(_ verdict: ServiceController.DisplayVerdict) -> Color {
        switch verdict {
        case .running: return Theme.success
        case .stopped, .notInstalled, .outdated, .runningForeignHome, .degraded:
            return Theme.warn
        case .unsupported: return Theme.danger
        case .unknown: return Theme.textSecondary
        }
    }

    // MARK: evidence (two sources, both quoted; open by default)

    @State private var evidenceOpen = true

    @ViewBuilder private var evidence: some View {
        if case .loaded(let report) = service.status {
            DisclosureGroup(isExpanded: $evidenceOpen) {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(evidenceRows(report), id: \.0) { key, value in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text(key)
                                .font(Theme.monoS)
                                .foregroundStyle(Theme.textTertiary)
                                .frame(width: 130, alignment: .leading)
                            Text(value)
                                .font(Theme.monoS)
                                .foregroundStyle(Theme.textPrimary)
                                .textSelection(.enabled)
                                .lineLimit(1)
                                .truncationMode(.middle)
                        }
                    }
                }
                .padding(.top, 6)
            } label: {
                Theme.sectionTitle("Evidence")
            }
            .tint(Theme.textTertiary)
        }
    }

    /// The SHIPPED report's facts, quoted — the app never averages the two
    /// sources into a guess. (The status report carries no launchd label
    /// or unit path; those land in the lifecycle envelopes.)
    private func evidenceRows(_ report: ServiceStatusReport) -> [(String, String)] {
        var rows: [(String, String)] = []
        var managerState = [String]()
        if let loaded = report.loaded { managerState.append(loaded ? "loaded" : "not loaded") }
        if let enabled = report.enabled { managerState.append(enabled) }
        if let active = report.active { managerState.append(active) }
        rows.append(("service manager",
                     "\(report.manager) \u{00B7} \(managerState.joined(separator: " \u{00B7} "))"))
        if let service = report.service {
            rows.append(("service", service.rawValue))
        }
        rows.append(("unit", report.installed.rawValue))
        if let checkpointState = report.homeCheckpoint {
            rows.append(("checkpoint", checkpointState.rawValue))
        }
        if let checkpoint = report.checkpoint {
            var facts = [String]()
            if let generation = checkpoint.generation { facts.append("generation \(generation)") }
            if let pid = checkpoint.pid { facts.append("pid \(pid)") }
            if let boot = checkpoint.bootID { facts.append("boot \(boot)") }
            if !facts.isEmpty {
                rows.append(("checkpoint id", facts.joined(separator: " \u{00B7} ")))
            }
            if let binary = checkpoint.binary { rows.append(("binary", binary)) }
            if let home = checkpoint.deadreckonHome { rows.append(("home", home)) }
        }
        if let boot = report.currentBootID { rows.append(("current boot", boot)) }
        return rows
    }

    // MARK: buttons by state (§S3.3)

    @ViewBuilder private var buttons: some View {
        let verdict = service.displayVerdict
        HStack(spacing: 8) {
            switch verdict {
            case .notInstalled, .outdated, .unsupported:
                if verdict != .unsupported {
                    Button("Install service\u{2026}") { confirm = .install }
                        .buttonStyle(.themePrimary)
                }
            case .stopped:
                Button("Start service\u{2026}") { confirm = .start }
                    .buttonStyle(.themePrimary)
                Button("Install service\u{2026}") { confirm = .install }
                    .buttonStyle(.themeStandard)
            case .running, .degraded, .runningForeignHome:
                Button("Stop service\u{2026}") { confirm = .stop }
                    .buttonStyle(.themeQuietDanger)
            case .unknown:
                // No buttons: the evidence and the escape hatch are the
                // surface.
                EmptyView()
            }
            Spacer()
            Button("Refresh") { Task { await service.refreshStatus() } }
                .buttonStyle(.themeStandard(compact: true))
        }
    }
}

/// Install/Start confirm (520): what will happen + the command well;
/// resolution only from the returned envelope, then the section's fresh
/// status poll (the controller re-polls inside dispatch).
private struct ServiceLifecycleSheet: View {
    let title: String
    let body_: String
    let verb: PlannedVerb
    let confirmLabel: String
    @ObservedObject var service: ServiceController
    @Environment(\.dismiss) private var dismiss

    init(title: String, body: String, verb: PlannedVerb, confirmLabel: String,
         service: ServiceController) {
        self.title = title
        self.body_ = body
        self.verb = verb
        self.confirmLabel = confirmLabel
        self.service = service
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(title)
                .font(Theme.title)
                .foregroundStyle(Theme.textPrimary)
            Text(body_)
                .font(Theme.base)
                .foregroundStyle(Theme.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
            CommandLineView(command: service.commandLine(for: verb))
            ServiceActionOutcome(service: service)
            HStack {
                Spacer()
                Button(isDone ? "Done" : "Cancel") {
                    service.resetAction()
                    dismiss()
                }
                .buttonStyle(.themeStandard)
                if !isDone {
                    Button(isRunning ? "Working\u{2026}" : confirmLabel) {
                        Task {
                            switch verb {
                            case .supervisorInstall: await service.install()
                            case .supervisorStart: await service.start()
                            default: break
                            }
                        }
                    }
                    .buttonStyle(.themePrimary)
                    .disabled(isRunning)
                }
            }
        }
        .padding(20)
        .frame(width: 520)
        .background(Theme.windowBg)
    }

    private var isRunning: Bool {
        if case .running = service.action { return true }
        return false
    }

    private var isDone: Bool {
        switch service.action {
        case .completed, .refused, .failed: return true
        case .idle, .running: return false
        }
    }
}

/// Stop confirm (§S3): the one typed-confirm in Settings — stopping
/// silently degrades every durable run, which is the severity the
/// ceremony exists for. Type `stop` to arm; stroke warn -> success on
/// match (the typed-confirm input grammar).
private struct ServiceStopSheet: View {
    @ObservedObject var service: ServiceController
    @Environment(\.dismiss) private var dismiss
    @State private var typed = ""

    private var armed: Bool { typed == "stop" }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Stop the background service?")
                .font(Theme.title)
                .foregroundStyle(Theme.textPrimary)
            Text("Running goals lose their supervisor: no new runs start, crashed runs stay down, and cleanup recording pauses until it starts again. Runs themselves are durable \u{2014} nothing is deleted.")
                .font(Theme.base)
                .foregroundStyle(Theme.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
            CommandLineView(command: service.commandLine(for: .supervisorStop))
            HStack(spacing: 8) {
                Text("Type `stop` to confirm")
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.textSecondary)
                TextField("stop", text: $typed)
                    .textFieldStyle(.plain)
                    .font(Theme.mono(12))
                    .foregroundStyle(Theme.textPrimary)
                    .padding(.horizontal, 8)
                    .frame(width: 100, height: 24)
                    .inputChrome(stroke: armed ? Theme.success : Theme.warn)
            }
            ServiceActionOutcome(service: service)
            HStack {
                Spacer()
                Button(isDone ? "Done" : "Cancel") {
                    service.resetAction()
                    dismiss()
                }
                .buttonStyle(.themeStandard)
                if !isDone {
                    Button(isRunning ? "Stopping\u{2026}" : "Stop Service") {
                        Task { await service.stop() }
                    }
                    .buttonStyle(.themeDangerConfirm)
                    .disabled(!armed || isRunning)
                }
            }
        }
        .padding(20)
        .frame(width: 520)
        .background(Theme.windowBg)
    }

    private var isRunning: Bool {
        if case .running = service.action { return true }
        return false
    }

    private var isDone: Bool {
        switch service.action {
        case .completed, .refused, .failed: return true
        case .idle, .running: return false
        }
    }
}

/// The lifecycle outcome inside a sheet: the envelope's facts verbatim, a
/// refusal card, or the envelope-free words. Never a guessed state.
private struct ServiceActionOutcome: View {
    @ObservedObject var service: ServiceController

    var body: some View {
        switch service.action {
        case .idle, .running:
            EmptyView()
        case .completed(let envelope):
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    StateDot(color: Theme.success)
                    Text("\(envelope.action): \(envelope.result ?? envelope.status ?? "completed")")
                        .font(Theme.body(12, weight: .medium))
                        .foregroundStyle(Theme.textPrimary)
                }
                if let unit = envelope.unitPath {
                    Text(unit)
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.textSecondary)
                        .textSelection(.enabled)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                if let state = envelope.serviceState {
                    Text("service manager now reports: \(state)")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textTertiary)
                }
            }
        case .refused(let refusal):
            RefusalView(refusal: refusal)
        case .failed(let words):
            Text(words)
                .font(Theme.mono(10.5))
                .foregroundStyle(Theme.warn)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

// MARK: - §S5 Binaries

/// THE operator ask: see all the binaries. Manifest + BinaryLocator rows
/// render instantly; doctor-sourced groups skeleton until the poll lands,
/// then degrade with the reason verbatim.
struct BinariesSettingsSection: View {
    @ObservedObject var fleet: FleetStore
    @ObservedObject var doctor: DoctorStore

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            group("This app") {
                SettingsInfoRow(label: "App version", value: appVersion)
            }
            group("Vendored CLI") { vendoredRows }
            installedGroup
            mismatchGroup
            group("Handshake & home") {
                SettingsInfoRow(
                    label: "CLI handshake", value: handshakeStatus,
                    detail: "The bundled CLI has no schema-version report yet (registered gap); the health check is the honest signal until it lands.")
                SettingsInfoRow(
                    label: "DEADRECKON_HOME", value: DeadreckonHome.url().path,
                    detail: ProcessInfo.processInfo.environment["DEADRECKON_HOME"]
                        .map { _ in "From the DEADRECKON_HOME environment variable." }
                        ?? "Default: ~/.deadreckon (no DEADRECKON_HOME set).",
                    mono: true)
            }
        }
    }

    private func group<Rows: View>(_ title: String, @ViewBuilder rows: () -> Rows) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            settingsGroupTitle(title)
            rows()
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    // MARK: vendored (manifest + locator, instant)

    @ViewBuilder private var vendoredRows: some View {
        if let override = ProcessInfo.processInfo.environment["DEADRECKON_BIN"],
           !override.isEmpty {
            SettingsInfoRow(
                label: "DEADRECKON_BIN override", value: override,
                detail: "Dev override in effect: manifest verification is skipped for this binary.",
                mono: true, valueColor: Theme.warn)
        } else if let manifest = VendoredManifest.load() {
            SettingsInfoRow(label: "Vendored CLI", value: manifest.cliVersion ?? "unknown",
                            detail: manifest.gitCommit.map { "commit \($0)" })
            pathRow(label: "Path", path: vendoredBinaryPath)
            shaRow(manifest: manifest)
            SettingsInfoRow(label: "CLI reports", value: fleet.binaryVersion ?? "not read yet",
                            detail: "Live `deadreckon --version` from the health poll.")
            Divider().overlay(Theme.border)
            gateRows(manifest: manifest)
        } else {
            SettingsInfoRow(label: "Vendored CLI", value: "manifest.json unreadable",
                            detail: "Resources/bin/manifest.json could not be read from the bundle.")
        }
    }

    /// sha256 row + the verification chip: BinaryLocator hashed the bytes
    /// against the pin at (or before) launch; a failure renders the
    /// locator's own words as a danger row (checksum mismatch carries both
    /// hashes already).
    @ViewBuilder private func shaRow(manifest: VendoredManifest) -> some View {
        let pinned = (manifest.sha256 ?? [:]).sorted(by: { $0.key < $1.key })
        ForEach(pinned, id: \.key) { entry in
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("sha256 (\(entry.key))")
                    .font(Theme.body(11, weight: .semibold))
                    .foregroundStyle(Theme.textSecondary)
                    .frame(width: 170, alignment: .leading)
                Text(entry.value)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textPrimary)
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
                verificationChip
            }
        }
        if pinned.isEmpty {
            Text("No pinned hashes: run scripts/vendor-cli.sh to vendor a binary.")
                .font(Theme.body(11))
                .foregroundStyle(Theme.warn)
        }
        if case .failure(let words) = locatorState {
            Text(words)
                .font(Theme.mono(10.5))
                .foregroundStyle(Theme.dangerText)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private enum LocatorState {
        case verified
        case failure(String)
    }

    /// One locate() call: cached per process, re-verifies nothing on
    /// repeat renders. Read-only.
    private var locatorState: LocatorState {
        do {
            _ = try BinaryLocator.locate()
            return .verified
        } catch {
            return .failure((error as? BinaryLocatorError)?.errorDescription
                ?? error.localizedDescription)
        }
    }

    @ViewBuilder private var verificationChip: some View {
        switch locatorState {
        case .verified:
            StatusChip(text: "verified at launch", color: Theme.success)
        case .failure:
            StatusChip(text: "verification failed", color: Theme.danger,
                       textColor: Theme.dangerText)
        }
    }

    /// dr-gate (§S5): pinned hash + path + doctor's protocol/compat facts.
    @ViewBuilder private func gateRows(manifest: VendoredManifest) -> some View {
        let gatePins = (manifest.gateSha256 ?? [:]).sorted(by: { $0.key < $1.key })
        ForEach(gatePins, id: \.key) { entry in
            SettingsInfoRow(label: "dr-gate sha256 (\(entry.key))", value: entry.value, mono: true)
        }
        if let gatePath = doctor.report?.binaryHealth?.gateHelperPath ?? bundledGatePath {
            pathRow(label: "dr-gate", path: gatePath)
        }
        if let health = doctor.report?.binaryHealth {
            HStack(spacing: 8) {
                Text("gate protocol v\(health.gateProtocolVersion ?? 0)")
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textPrimary)
                if let compatible = health.gateHelperCompatible {
                    StatusChip(
                        text: compatible ? "compatible" : "incompatible",
                        color: compatible ? Theme.success : Theme.danger,
                        textColor: compatible ? nil : Theme.dangerText)
                }
            }
            .padding(.leading, 178)
        }
        Text(Lexicon.drGateCaption)
            .font(Theme.body(10.5))
            .foregroundStyle(Theme.textTertiary)
            .fixedSize(horizontal: false, vertical: true)
    }

    private var bundledGatePath: String? {
        Bundle.main.url(forResource: "dr-gate", withExtension: nil, subdirectory: "bin")?.path
    }

    private var vendoredBinaryPath: String {
        Bundle.main.url(forResource: "deadreckon_darwin_arm64", withExtension: nil,
                        subdirectory: "bin")?.path
            ?? "Resources/bin/deadreckon_darwin_arm64"
    }

    private func pathRow(label: String, path: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(label)
                .font(Theme.body(11, weight: .semibold))
                .foregroundStyle(Theme.textSecondary)
                .frame(width: 170, alignment: .leading)
            Text(path)
                .font(Theme.mono(10.5))
                .foregroundStyle(Theme.textPrimary)
                .textSelection(.enabled)
                .lineLimit(1)
                .truncationMode(.middle)
            Button("Reveal in Finder") { revealInFinder(path) }
                .buttonStyle(.themeStandard(compact: true))
        }
    }

    // MARK: installed CLIs (doctor-sourced; skeleton -> rows -> degrade)

    @ViewBuilder private var installedGroup: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                settingsGroupTitle("Installed CLIs")
                if let checked = doctor.lastChecked {
                    Text("from the health check, \(Lexicon.relativeTime(checked))")
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textTertiary)
                }
                Spacer()
                Button("Refresh") { Task { await doctor.run() } }
                    .buttonStyle(.themeStandard(compact: true))
            }
            switch doctor.state {
            case .idle, .loading:
                Text("Running the health check\u{2026}")
                    .font(Theme.small)
                    .foregroundStyle(Theme.textTertiary)
            case .unavailable(let reason):
                Text("doctor unavailable \u{2014} \(reason)")
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            case .loaded(let report):
                if let health = report.binaryHealth, !health.installations.isEmpty {
                    ForEach(health.installations, id: \.canonicalPath) { installation in
                        InstalledCLIRow(installation: installation,
                                        pathSelected: health.pathSelected,
                                        currentPath: health.currentPath)
                    }
                } else {
                    Text("No installed CLIs reported.")
                        .font(Theme.small)
                        .foregroundStyle(Theme.textTertiary)
                }
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    /// Mismatch warnings (§S5.4): conflicts verbatim, one warn row each;
    /// the group is ABSENT with zero conflicts (no all-clear theater).
    @ViewBuilder private var mismatchGroup: some View {
        if let conflicts = doctor.report?.binaryHealth?.conflicts, !conflicts.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                settingsGroupTitle("Mismatch warnings")
                ForEach(conflicts, id: \.self) { conflict in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Image(systemName: "exclamationmark.triangle")
                            .font(.system(size: 10, weight: .medium))
                            .foregroundStyle(Theme.warn)
                        Text(conflict)
                            .font(Theme.mono(10.5))
                            .foregroundStyle(Theme.textPrimary)
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .cardChrome()
        }
    }

    private var handshakeStatus: String {
        switch fleet.harbor.doctor {
        case .unknown(let reason): return "\(Lexicon.healthWord(fleet.harbor.doctor)): \(reason)"
        default: return Lexicon.healthWord(fleet.harbor.doctor)
        }
    }

    private var appVersion: String {
        let short = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String
        switch (short, build) {
        case (let short?, let build?): return "\(short) (\(build))"
        case (let short?, nil): return short
        default: return "unknown"
        }
    }
}

/// The vendored manifest (Resources/bin/manifest.json — the same file
/// BinaryLocator verifies against), incl. the dr-gate pin.
struct VendoredManifest: Decodable {
    let cliVersion: String?
    let gitCommit: String?
    let sha256: [String: String]?
    let gateSha256: [String: String]?

    static func load() -> VendoredManifest? {
        guard let url = Bundle.main.url(
            forResource: "manifest", withExtension: "json", subdirectory: "bin"),
            let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(VendoredManifest.self, from: data)
    }
}

/// One installed CLI (§S5.3): channel chip + version + canonical path +
/// role facts in plain words + sha256 + the guided self-update handoff
/// (update_command, copyable).
private struct InstalledCLIRow: View {
    let installation: DoctorReportEnvelope.Installation
    let pathSelected: String?
    let currentPath: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 8) {
                StatusChip(text: installation.channel, color: Theme.textSecondary)
                Text(installation.version ?? "version unknown")
                    .font(Theme.body(12, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                Text(installation.canonicalPath)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textSecondary)
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer()
                Button("Reveal in Finder") { revealInFinder(installation.canonicalPath) }
                    .buttonStyle(.themeStandard(compact: true))
            }
            if !roleWords.isEmpty {
                Text(roleWords.joined(separator: " \u{00B7} "))
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textSecondary)
            }
            if let sha = installation.sha256 {
                Text(sha)
                    .font(Theme.monoS)
                    .foregroundStyle(Theme.textTertiary)
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            if let probeError = installation.probeError {
                Text(probeError)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            }
            if let update = installation.updateCommand {
                // The guided self-update handoff (§R5/B8): the command is
                // shown and copyable; the app never runs it.
                HStack(spacing: 8) {
                    CommandLineView(command: update)
                    Button("Copy") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(update, forType: .string)
                    }
                    .buttonStyle(.themeStandard(compact: true))
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.well.opacity(0.4),
                    in: RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous)
            .strokeBorder(Theme.border, lineWidth: 1))
    }

    /// Role facts in plain words (§S5.3), derived from the shipped roles
    /// vocabulary; unknown words pass through verbatim.
    private var roleWords: [String] {
        var words: [String] = []
        for role in installation.roles {
            switch role {
            case "path": words.append("on PATH")
            case "path-selected": words.append("PATH selects this one")
            case "current": words.append("current process")
            case "known-install-location": continue
            default: words.append(role)
            }
        }
        if pathSelected == installation.canonicalPath,
           !words.contains("PATH selects this one") {
            words.append("PATH selects this one")
        }
        if currentPath == installation.canonicalPath, !words.contains("current process") {
            words.append("current process")
        }
        return words
    }
}

// MARK: - §S6 Health

/// Doctor, as a surface: the verdict banner in doctor's own words, the
/// findings table in the binary's order, the section-level repair (SHIPPED
/// has no per-finding repairable flag), and the raw document behind a
/// disclosure — the evidence floor.
struct HealthSettingsSection: View {
    @ObservedObject var doctor: DoctorStore
    @State private var rawOpen = false
    @State private var confirmRepair = false
    @State private var expandedFinding: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            settingsGroupTitle("Health")
            switch doctor.state {
            case .idle, .loading:
                Text("Running the health check\u{2026}")
                    .font(Theme.small)
                    .foregroundStyle(Theme.textTertiary)
            case .unavailable(let reason):
                VStack(alignment: .leading, spacing: 6) {
                    Text("doctor unavailable \u{2014} the words verbatim:")
                        .font(Theme.small)
                        .foregroundStyle(Theme.warn)
                    Text(reason)
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.textPrimary)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    CommandLineView(command: "deadreckon doctor")
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .cardChrome()
            case .loaded(let report):
                loaded(report)
            }
        }
        .sheet(isPresented: $confirmRepair) {
            RepairSheet(doctor: doctor)
        }
    }

    @ViewBuilder private func loaded(_ report: DoctorReportEnvelope) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            // Verdict banner: the envelope's own words, no app judgment.
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 8) {
                    Text(report.status ?? "unknown")
                        .font(Theme.heading)
                        .foregroundStyle(bannerColor(report))
                    if let checked = doctor.lastChecked {
                        Text("checked \(Lexicon.relativeTime(checked))")
                            .font(Theme.caption)
                            .foregroundStyle(Theme.textTertiary)
                    }
                }
                if let explanation = report.verdict?.explanation {
                    Text(explanation)
                        .font(Theme.base)
                        .foregroundStyle(Theme.textSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            Divider().overlay(Theme.border)

            // Findings, the binary's order (its triage order), verbatim.
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(report.findings.enumerated()), id: \.offset) { index, finding in
                    findingRow(finding, id: "\(index)-\(finding.subject)")
                }
            }

            if report.repairAvailable {
                Button("Repair\u{2026}") { confirmRepair = true }
                    .buttonStyle(.themeStandard)
            }
            repairOutcome

            Divider().overlay(Theme.border)

            HStack(spacing: 8) {
                Button("Run health check") { Task { await doctor.run() } }
                    .buttonStyle(.themeStandard)
                Spacer()
            }

            DisclosureGroup(isExpanded: $rawOpen) {
                ScrollView {
                    Text(report.rawJSON)
                        .font(Theme.monoS)
                        .foregroundStyle(Theme.textPrimary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(8)
                }
                .frame(maxHeight: 220)
                .background(Theme.well,
                            in: RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous)
                    .strokeBorder(Theme.border, lineWidth: 1))
                .padding(.top, 6)
            } label: {
                Theme.sectionTitle("Raw report")
            }
            .tint(Theme.textTertiary)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    private func bannerColor(_ report: DoctorReportEnvelope) -> Color {
        switch report.status {
        case "blocked", "failed": return Theme.dangerText
        case "verified", "completed": return Theme.success
        default: return Theme.textPrimary
        }
    }

    /// One 40px finding row: state glyph + subject + detail mono
    /// middle-truncated; click expands to the full detail, selectable.
    private func findingRow(_ finding: DoctorReportEnvelope.Finding, id: String) -> some View {
        let expanded = expandedFinding == id
        return VStack(alignment: .leading, spacing: 0) {
            Button {
                expandedFinding = expanded ? nil : id
            } label: {
                HStack(spacing: 8) {
                    Text(glyph(finding.status))
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(glyphColor(finding.status))
                        .frame(width: 14)
                    Text(finding.subject)
                        .font(Theme.body(13, weight: .medium))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1)
                    if !expanded {
                        Text(finding.detail)
                            .font(Theme.mono(11))
                            .foregroundStyle(Theme.textSecondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    Spacer(minLength: 0)
                }
                .frame(height: 40)
                .contentShape(Rectangle())
            }
            .buttonStyle(.tactile)
            if expanded {
                VStack(alignment: .leading, spacing: 4) {
                    Text(finding.detail)
                        .font(Theme.mono(11))
                        .foregroundStyle(Theme.textPrimary)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    if let action = finding.action {
                        HStack(spacing: 5) {
                            Text("try:")
                                .font(Theme.body(10, weight: .semibold))
                                .foregroundStyle(Theme.textTertiary)
                            Text(action)
                                .font(Theme.mono(10.5))
                                .foregroundStyle(Theme.accent)
                                .textSelection(.enabled)
                        }
                    }
                }
                .padding(.leading, 22)
                .padding(.bottom, 8)
            }
        }
    }

    private func glyph(_ status: String) -> String {
        switch status {
        case "passed": return "\u{2713}"
        case "warning": return "!"
        case "failed": return "\u{2717}"
        default: return "?"
        }
    }

    private func glyphColor(_ status: String) -> Color {
        switch status {
        case "passed": return Theme.success
        case "warning": return Theme.warn
        case "failed": return Theme.danger
        default: return Theme.textTertiary
        }
    }

    /// Repair outcomes (§P5): the report's own repairs[] rows, verbatim.
    @ViewBuilder private var repairOutcome: some View {
        switch doctor.repair {
        case .idle, .running:
            EmptyView()
        case .completed(let report) where !report.repairs.isEmpty:
            VStack(alignment: .leading, spacing: 3) {
                settingsGroupTitle("Repairs attempted")
                ForEach(Array(report.repairs.enumerated()), id: \.offset) { _, repair in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(glyph(repair.result))
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(glyphColor(repair.result))
                            .frame(width: 14)
                        Text(repair.attempted)
                            .font(Theme.body(12, weight: .medium))
                            .foregroundStyle(Theme.textPrimary)
                        Text(repair.detail)
                            .font(Theme.mono(10.5))
                            .foregroundStyle(Theme.textSecondary)
                            .textSelection(.enabled)
                            .lineLimit(2)
                            .truncationMode(.middle)
                    }
                }
            }
        case .completed:
            EmptyView()
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

/// Repair confirm (520): what will be attempted + the command well;
/// primary [Repair]; outcomes render in the section from the returned
/// document's repairs[] rows.
private struct RepairSheet: View {
    @ObservedObject var doctor: DoctorStore
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Repair setup issues?")
                .font(Theme.title)
                .foregroundStyle(Theme.textPrimary)
            Text("The CLI attempts its three bounded repairs: the active shell installation, the install receipt, and the managed supervisor binding. Each outcome is reported; nothing else is touched.")
                .font(Theme.base)
                .foregroundStyle(Theme.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
            CommandLineView(command: doctor.commandLine(for: .doctorRepair))
            if case .running = doctor.repair {
                Text("Repairing\u{2026}")
                    .font(Theme.small)
                    .foregroundStyle(Theme.textTertiary)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(.themeStandard)
                Button("Repair") {
                    Task {
                        await doctor.runRepair()
                        dismiss()
                    }
                }
                .buttonStyle(.themePrimary)
                .disabled({ if case .running = doctor.repair { return true }; return false }())
            }
        }
        .padding(20)
        .frame(width: 520)
        .background(Theme.windowBg)
    }
}
