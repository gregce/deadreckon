import DeadreckonKit
import SwiftUI

/// Settings (Command-comma), matching the exemplar's simplicity: a segmented
/// bar over flat cards. Three sections — General (launch at login,
/// appearance note), Notifications (master + per-reason toggles), Info
/// (read-only facts: app/binary versions, vendored sha, DEADRECKON_HOME,
/// schema-handshake status). Every info row states its real provenance;
/// nothing here mutates anything under DEADRECKON_HOME.
struct SettingsView: View {
    @ObservedObject var fleet: FleetStore
    /// APP-5 fix pass: the AttentionCenter's per-job tail trouble
    /// (`issues`) renders here — honest degradation needs a surface.
    @ObservedObject var attention: AttentionCenter

    @State private var tab: SettingsTab = .general
    @State private var preferences = AttentionPreferences.load(from: .standard)
    @State private var launchAtLogin = LaunchAtLogin.isEnabled
    @State private var launchAtLoginIssue: String?

    private enum SettingsTab: String, CaseIterable, Identifiable {
        case general = "General"
        case notifications = "Notifications"
        case info = "Info"
        var id: String { rawValue }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                Text("Settings")
                    .font(Theme.display)
                    .foregroundStyle(Theme.textPrimary)

                // The one tab grammar (DESIGN.md §5): text tabs in a panel
                // strip, active = textPrimary on well.
                HStack(spacing: 2) {
                    ForEach(SettingsTab.allCases) { candidate in
                        TabButton(title: candidate.rawValue, active: tab == candidate) {
                            tab = candidate
                        }
                    }
                    Spacer()
                }
                .padding(4)
                .background(Theme.panel,
                            in: RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
                    .strokeBorder(Theme.border, lineWidth: 1))

                switch tab {
                case .general:
                    generalCard
                case .notifications:
                    notificationsCard
                case .info:
                    infoCard
                }
            }
            .padding(28)
            .frame(maxWidth: 560, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .center)
        }
        .background(Theme.windowBg)
        .frame(minWidth: 620, minHeight: 460)
    }

    // MARK: - General

    private var generalCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            settingsGroupTitle("Startup")
            Toggle(isOn: $launchAtLogin) {
                settingLabel(
                    "Launch at login",
                    detail: "Keep deadreckon in your menu bar so your runs are watched and decisions reach you.")
            }
            .toggleStyle(.switch)
            .onChange(of: launchAtLogin) { _, enabled in
                // Re-entrancy guard: reverting the toggle after a failure
                // re-fires onChange with the real state. Without this guard
                // that echo would call SMAppService again (unregister on a
                // never-registered service) and clobber the rendered
                // failure. A no-op transition never touches the service.
                guard enabled != LaunchAtLogin.isEnabled else { return }
                do {
                    try LaunchAtLogin.setEnabled(enabled)
                    launchAtLoginIssue = nil
                } catch {
                    // Render the failure and reflect the REAL state instead
                    // of a lying toggle.
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

            Divider().overlay(Theme.border)

            settingsGroupTitle("Appearance")
            Text("deadreckon is dark by design. There is no light mode.")
                .font(Theme.small)
                .foregroundStyle(Theme.textSecondary)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    // MARK: - Notifications

    private var notificationsCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            settingsGroupTitle("Notifications")
            Toggle(isOn: masterBinding) {
                settingLabel(
                    "Notify me when a run needs attention",
                    detail: "Derived from the attention entries the CLI writes to `notify.jsonl`. Signals only \u{2014} the app re-reads the files when you open it.")
            }
            .toggleStyle(.switch)

            Divider().overlay(Theme.border)

            ForEach(AttentionPreferences.notifiableReasons, id: \.self) { reason in
                Toggle(isOn: reasonBinding(reason)) {
                    settingLabel(AttentionDerivation.title(for: reason),
                                 detail: reasonDetail(reason))
                }
                .toggleStyle(.switch)
                .disabled(!preferences.masterEnabled)
            }

            Divider().overlay(Theme.border)

            Text("macOS permission is requested the first time a notification is delivered. If you declined it, enable deadreckon under System Settings > Notifications; a grant there takes effect on the next notification, no relaunch needed.")
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.textTertiary)

            // Honest degradation surface: a corrupt notify.jsonl stops that
            // job's notifications permanently (sticky per attempt). The
            // operator must be able to SEE that silence.
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

    // MARK: - Info

    private var infoCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            settingsGroupTitle("Read-only facts")

            infoRow("App version", appVersion)
            infoRow("CLI reports", fleet.binaryVersion ?? "not read yet",
                    detail: "Live `deadreckon --version` from the health poll.")

            if let override = ProcessInfo.processInfo.environment["DEADRECKON_BIN"],
               !override.isEmpty {
                infoRow("DEADRECKON_BIN override", override,
                        detail: "Dev override in effect: manifest verification is skipped for this binary.")
            } else {
                vendoredRows
            }

            Divider().overlay(Theme.border)

            infoRow("DEADRECKON_HOME", DeadreckonHome.url().path,
                    detail: ProcessInfo.processInfo.environment["DEADRECKON_HOME"]
                        .map { _ in "From the DEADRECKON_HOME environment variable." }
                        ?? "Default: ~/.deadreckon (no DEADRECKON_HOME set).")

            Divider().overlay(Theme.border)

            settingsGroupTitle("CLI handshake")
            infoRow("Status", schemaHandshakeStatus,
                    detail: "The bundled CLI has no schema-version report yet (registered gap); the health check is the honest signal until it lands.")
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardChrome()
    }

    /// The vendored manifest rows (Resources/bin/manifest.json, the same
    /// file BinaryLocator verifies against).
    @ViewBuilder private var vendoredRows: some View {
        if let manifest = Self.vendoredManifest() {
            infoRow("Vendored CLI", manifest.cliVersion ?? "unknown",
                    detail: manifest.gitCommit.map { "commit \($0)" })
            ForEach((manifest.sha256 ?? [:]).sorted(by: { $0.key < $1.key }), id: \.key) { entry in
                infoRow("sha256 (\(entry.key))", entry.value, mono: true)
            }
            if (manifest.sha256 ?? [:]).isEmpty {
                Text("No pinned hashes: run scripts/vendor-cli.sh to vendor a binary.")
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.warn)
            }
        } else {
            infoRow("Vendored CLI", "manifest.json unreadable",
                    detail: "Resources/bin/manifest.json could not be read from the bundle.")
        }
    }

    private var schemaHandshakeStatus: String {
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

    struct VendoredManifest: Decodable {
        let cliVersion: String?
        let gitCommit: String?
        let sha256: [String: String]?
    }

    static func vendoredManifest() -> VendoredManifest? {
        guard let url = Bundle.main.url(
            forResource: "manifest", withExtension: "json", subdirectory: "bin"),
            let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(VendoredManifest.self, from: data)
    }

    // MARK: - Shared chrome

    /// Settings group titles are scan-first headers: the one shared section
    /// title in textSecondary (DESIGN.md §5).
    private func settingsGroupTitle(_ text: String) -> some View {
        Theme.sectionTitle(text, color: Theme.textSecondary)
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

    private func infoRow(_ label: String, _ value: String,
                         detail: String? = nil, mono: Bool = false) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(label)
                    .font(Theme.body(11, weight: .semibold))
                    .foregroundStyle(Theme.textSecondary)
                    .frame(width: 170, alignment: .leading)
                Text(value)
                    .font(mono ? Theme.mono(10.5) : Theme.body(12))
                    .foregroundStyle(Theme.textPrimary)
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
