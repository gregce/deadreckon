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
                    .font(Theme.display(24))
                    .foregroundStyle(Theme.ink)

                Picker("Settings section", selection: $tab) {
                    ForEach(SettingsTab.allCases) { tab in
                        Text(tab.rawValue).tag(tab)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()

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
        .background(Theme.paper)
        .frame(minWidth: 620, minHeight: 460)
    }

    // MARK: - General

    private var generalCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            settingsGroupTitle("Startup")
            Toggle(isOn: $launchAtLogin) {
                settingLabel(
                    "Launch at login",
                    detail: "Keep deadreckon in your menu bar so the fleet is watched and decisions reach you.")
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

            Divider().overlay(Theme.hairline)

            settingsGroupTitle("Appearance")
            Text("deadreckon follows the system appearance. Every color is a light/dark dynamic pair; there is no separate theme setting.")
                .font(Theme.body(11))
                .foregroundStyle(Theme.inkSecondary)
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
                    "Notify me when a job needs attention",
                    detail: "Derived from the typed operator_attention rows the binary appends to notify.jsonl. Display-only signals; the app re-reads the durable files when you open it.")
            }
            .toggleStyle(.switch)

            Divider().overlay(Theme.hairline)

            ForEach(AttentionPreferences.notifiableReasons, id: \.self) { reason in
                Toggle(isOn: reasonBinding(reason)) {
                    settingLabel(AttentionDerivation.title(for: reason),
                                 detail: reasonDetail(reason))
                }
                .toggleStyle(.switch)
                .disabled(!preferences.masterEnabled)
            }

            Divider().overlay(Theme.hairline)

            Text("macOS permission is requested the first time a notification is delivered. If you declined it, enable deadreckon under System Settings > Notifications; a grant there takes effect on the next notification, no relaunch needed.")
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.inkTertiary)

            // Honest degradation surface: a corrupt notify.jsonl stops that
            // job's notifications permanently (sticky per attempt). The
            // operator must be able to SEE that silence.
            if !attention.issues.isEmpty {
                Divider().overlay(Theme.hairline)
                settingsGroupTitle("Notify tail trouble")
                ForEach(attention.issues.sorted(by: { $0.key < $1.key }), id: \.key) { jobID, reason in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(jobID)
                            .font(Theme.mono(11))
                            .foregroundStyle(Theme.ink)
                            .textSelection(.enabled)
                        Text("Notifications from this job's current attempt are stopped: \(reason)")
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
            return "A two-key receipt sealed; the result waits at the gate for your promote."
        case .pausedAtCap:
            return "The run loop stopped at a spend or wall-time cap."
        case .waitingInput:
            return "The job waits for your review before it can continue."
        case .blocked:
            return "The supervisor classified the job as blocked."
        case .failed:
            return "The supervisor classified the job as failed."
        case .cancelled:
            return "The supervisor classified the job as cancelled."
        case .unknown:
            return GlossaryText.unknownState
        }
    }

    // MARK: - Info

    private var infoCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            settingsGroupTitle("Read-only facts")

            infoRow("App version", appVersion)
            infoRow("Binary reports", fleet.binaryVersion ?? "not read yet",
                    detail: "Live deadreckon --version from the Harbor poll.")

            if let override = ProcessInfo.processInfo.environment["DEADRECKON_BIN"],
               !override.isEmpty {
                infoRow("DEADRECKON_BIN override", override,
                        detail: "Dev override in effect: manifest verification is skipped for this binary.")
            } else {
                vendoredRows
            }

            Divider().overlay(Theme.hairline)

            infoRow("DEADRECKON_HOME", DeadreckonHome.url().path,
                    detail: ProcessInfo.processInfo.environment["DEADRECKON_HOME"]
                        .map { _ in "From the DEADRECKON_HOME environment variable." }
                        ?? "Default: ~/.deadreckon (no DEADRECKON_HOME set).")

            Divider().overlay(Theme.hairline)

            settingsGroupTitle("Schema handshake")
            infoRow("Status", schemaHandshakeStatus,
                    detail: "The committed binary has no surface reporting the home's schema version yet (registered Rust-side gap). Until it lands, doctor --json is the honest health signal.")
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
        case .ok(let warnings) where warnings == 0: return "doctor ok"
        case .ok(let warnings): return "doctor ok, \(warnings) warning\(warnings == 1 ? "" : "s")"
        case .failed(let count): return "doctor: \(count) finding\(count == 1 ? "" : "s") failed"
        case .unknown(let reason): return "doctor unknown: \(reason)"
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

    /// Settings' deliberate 11pt sub-scale of the shared section-title
    /// token — named distinctly so it cannot shadow `Theme.sectionTitle`,
    /// and delegating to it so the metric lives in one place.
    private func settingsGroupTitle(_ text: String) -> some View {
        Theme.sectionTitle(text, size: 11, kerning: 0.5)
    }

    private func settingLabel(_ title: String, detail: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(Theme.body(13, weight: .medium))
                .foregroundStyle(Theme.ink)
            Text(detail)
                .font(Theme.body(11))
                .foregroundStyle(Theme.inkSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func infoRow(_ label: String, _ value: String,
                         detail: String? = nil, mono: Bool = false) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(label)
                    .font(Theme.body(11, weight: .semibold))
                    .foregroundStyle(Theme.inkSecondary)
                    .frame(width: 170, alignment: .leading)
                Text(value)
                    .font(mono ? Theme.mono(10.5) : Theme.body(12))
                    .foregroundStyle(Theme.ink)
                    .textSelection(.enabled)
                    .lineLimit(2)
                    .truncationMode(.middle)
            }
            if let detail {
                Text(detail)
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.inkTertiary)
                    .padding(.leading, 178)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}
