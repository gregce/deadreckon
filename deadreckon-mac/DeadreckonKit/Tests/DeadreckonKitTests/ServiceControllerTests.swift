import XCTest

@testable import DeadreckonKit

/// ServiceController: the SHIPPED v4 two-source report decode, the typed
/// display verdict over it (never an averaged guess), the pre-v4 prose
/// refusal degradation, and the lifecycle resolution discipline (envelope
/// first, then a fresh status re-poll).
@MainActor
final class ServiceControllerTests: XCTestCase {

    func testStatusDecodesTheShippedV4Report() async {
        let cli = SettingsFakeCLI()
        cli.script("supervisor status", stdout: SettingsFixtures.serviceStatusHealthy)
        let controller = ServiceController(cli: cli)

        await controller.refreshStatus()

        guard case .loaded(let report) = controller.status else {
            return XCTFail("expected loaded, got \(controller.status)")
        }
        XCTAssertEqual(report.schemaVersion, 4)
        XCTAssertEqual(report.manager, "launchd")
        XCTAssertEqual(report.installed, .current)
        XCTAssertEqual(report.service, .running)
        XCTAssertEqual(report.homeCheckpoint, .present)
        XCTAssertEqual(report.verdict, .healthy)
        XCTAssertEqual(report.checkpoint?.generation, 12)
        XCTAssertEqual(report.checkpoint?.pid, 79697)
        XCTAssertEqual(report.checkpoint?.bootID, "macos:7A2C9D")
        XCTAssertEqual(report.checkpoint?.deadreckonHome, "/Users/op/.deadreckon")
        XCTAssertEqual(controller.displayVerdict, .running)
    }

    /// The verdict mapping types on the SHIPPED vocabulary
    /// (healthy | degraded | foreign_home | down + service + installed).
    func testDisplayVerdictMapsTheShippedVocabulary() throws {
        func report(_ json: String) throws -> ServiceStatusReport {
            try XCTUnwrap(ServiceStatusReport(data: Data(json.utf8)))
        }

        // foreign_home: the FULL-DRIVE B5a typed state.
        XCTAssertEqual(
            ServiceController.displayVerdict(for: try report(
                SettingsFixtures.serviceStatus(
                    service: "running", homeCheckpoint: "absent", verdict: "foreign_home",
                    installed: "current",
                    reason: "service manager reports the supervisor running, but its instance checkpoint is absent"))),
            .runningForeignHome)
        // degraded + stale unit = the spec's "Outdated".
        XCTAssertEqual(
            ServiceController.displayVerdict(for: try report(
                SettingsFixtures.serviceStatus(
                    service: "running", homeCheckpoint: "present", verdict: "degraded",
                    installed: "stale", reason: "points at a different binary"))),
            .outdated)
        // degraded on a current unit keeps the degraded word + reason.
        XCTAssertEqual(
            ServiceController.displayVerdict(for: try report(
                SettingsFixtures.serviceStatus(
                    service: "running", homeCheckpoint: "stale", verdict: "degraded",
                    installed: "current", reason: "not enabled after restart"))),
            .degraded)
        XCTAssertEqual(
            ServiceController.displayVerdict(for: try report(
                SettingsFixtures.serviceStatus(
                    service: "not_installed", homeCheckpoint: "absent", verdict: "down",
                    installed: "not_installed", reason: nil))),
            .notInstalled)
        XCTAssertEqual(
            ServiceController.displayVerdict(for: try report(
                SettingsFixtures.serviceStatus(
                    service: "stopped", homeCheckpoint: "present", verdict: "down",
                    installed: "current", reason: "the service manager reports the supervisor stopped"))),
            .stopped)
    }

    /// A v3 report (older binary: typed two-source fields absent) still
    /// classifies from installed + manager runtime — the pre-v4 chip logic.
    func testV3ReportWithoutTypedFieldsDegradesHonestly() throws {
        let v3 = """
        {"schema_version": 3, "manager": "launchd", "installed": "current",
         "loaded": true, "enabled": "enabled", "active": null,
         "current_boot_id": "macos:1", "boot_identity_source": "macos_sysctl",
         "test_override": false}
        """
        let report = try XCTUnwrap(ServiceStatusReport(data: Data(v3.utf8)))
        XCTAssertNil(report.verdict)
        XCTAssertEqual(ServiceController.displayVerdict(for: report), .running)

        let stopped = v3.replacingOccurrences(of: "\"loaded\": true", with: "\"loaded\": false")
        XCTAssertEqual(
            ServiceController.displayVerdict(
                for: try XCTUnwrap(ServiceStatusReport(data: Data(stopped.utf8)))),
            .stopped)
    }

    /// The live-observed pre-v4 refusal: prose on stderr, exit 1. Verdict
    /// Unknown + the words verbatim — never a guessed state.
    func testProseRefusalDegradesWithTheWordsVerbatim() async {
        let cli = SettingsFakeCLI()
        cli.script("supervisor status", stdout: "",
                   stderr: SettingsFixtures.serviceStatusProseRefusal, exitCode: 1)
        let controller = ServiceController(cli: cli)

        await controller.refreshStatus()

        guard case .unavailable(let words) = controller.status else {
            return XCTFail("expected unavailable, got \(controller.status)")
        }
        XCTAssertTrue(words.contains("instance checkpoint is absent"))
        XCTAssertEqual(controller.displayVerdict, .unknown)
    }

    /// Resolution discipline (§P6): install resolves on ITS envelope, then
    /// status is re-polled before the verdict repaints.
    func testInstallResolvesOnEnvelopeThenRePollsStatus() async {
        let cli = SettingsFakeCLI()
        cli.script("supervisor install",
                   stdout: SettingsFixtures.supervisorLifecycle(
                       action: "install", result: "installed", serviceState: "stopped"))
        cli.script("supervisor status", stdout: SettingsFixtures.serviceStatusHealthy)
        let controller = ServiceController(cli: cli)

        await controller.install()

        guard case .completed(let envelope) = controller.action else {
            return XCTFail("expected completed, got \(controller.action)")
        }
        XCTAssertEqual(envelope.action, "install")
        XCTAssertEqual(envelope.result, "installed")
        XCTAssertEqual(envelope.unitPath,
                       "/Users/op/Library/LaunchAgents/com.deadreckon.supervisor.plist")
        // The envelope resolved the action; the verdict came from a FRESH poll.
        let order = cli.calls.map { $0.prefix(2).joined(separator: " ") }
        XCTAssertEqual(order, ["supervisor install", "supervisor status"])
        guard case .loaded = controller.status else {
            return XCTFail("status must be re-polled after the lifecycle verb")
        }
    }

    func testStopArgvAndEnvelope() async {
        let cli = SettingsFakeCLI()
        cli.script("supervisor stop",
                   stdout: SettingsFixtures.supervisorLifecycle(
                       action: "stop", result: "stopped", serviceState: "stopped"))
        cli.script("supervisor status",
                   stdout: SettingsFixtures.serviceStatus(
                       service: "stopped", homeCheckpoint: "present", verdict: "down",
                       installed: "current", reason: "the service manager reports the supervisor stopped"))
        let controller = ServiceController(cli: cli)

        await controller.stop()

        XCTAssertEqual(cli.calls.first, ["supervisor", "stop", "--json"])
        guard case .completed(let envelope) = controller.action else {
            return XCTFail("expected completed, got \(controller.action)")
        }
        XCTAssertEqual(envelope.result, "stopped")
        XCTAssertEqual(controller.displayVerdict, .stopped)
    }

    /// An unmanaged unit refuses with the shared error envelope — rendered
    /// verbatim, never forced aside (the app offers no force, anywhere).
    func testUnmanagedUnitRefusalIsAuthoritative() async {
        let cli = SettingsFakeCLI()
        cli.script("supervisor install",
                   stdout: SettingsFixtures.supervisorUnmanagedRefusal, exitCode: 1)
        cli.script("supervisor status",
                   stdout: SettingsFixtures.serviceStatus(
                       service: "stopped", homeCheckpoint: "absent", verdict: "down",
                       installed: "not_installed", reason: nil))
        let controller = ServiceController(cli: cli)

        await controller.install()

        guard case .refused(let refusal) = controller.action else {
            return XCTFail("expected refused, got \(controller.action)")
        }
        XCTAssertTrue(refusal.message.contains("unmanaged unit"))
        XCTAssertEqual(refusal.tryLines, ["deadreckon supervisor status"])
    }

    /// Unsupported platforms classify from the report, not a crash.
    func testUnsupportedReportClassifies() throws {
        let unsupported = """
        {"schema_version": 4, "manager": "unsupported", "installed": "unsupported",
         "loaded": null, "enabled": null, "active": null,
         "service": "not_installed", "home_checkpoint": "absent", "verdict": "down",
         "verdict_reason": "machine-restart durability is available only on macOS launchd and Linux systemd",
         "checkpoint": null, "current_boot_id": "unknown-boot",
         "boot_identity_source": "unknown", "test_override": false}
        """
        let report = try XCTUnwrap(ServiceStatusReport(data: Data(unsupported.utf8)))
        XCTAssertEqual(ServiceController.displayVerdict(for: report), .unsupported)
    }
}
