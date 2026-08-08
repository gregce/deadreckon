import XCTest

@testable import DeadreckonKit

/// DoctorStore: full-document retention (the raw-report disclosure), the
/// binary's own finding order, the SHIPPED repair capability probe (no
/// per-finding repairable flag — section-level signals only), and repair
/// outcomes from the report's repairs[] rows.
@MainActor
final class DoctorStoreTests: XCTestCase {

    func testRunRetainsTheFullDocumentInOrder() async {
        let cli = SettingsFakeCLI()
        cli.script("doctor --json", stdout: SettingsFixtures.doctorReport())
        let store = DoctorStore(cli: cli)
        store.nowProvider = { Date(timeIntervalSince1970: 1_000) }

        await store.run()

        guard case .loaded(let report) = store.state else {
            return XCTFail("expected loaded, got \(store.state)")
        }
        // The binary's order is its triage order — preserved verbatim.
        XCTAssertEqual(report.findings.map(\.subject),
                       ["source", "config", "sandbox sandbox-exec", "supervisor service"])
        XCTAssertEqual(report.findings[1].status, "failed")
        XCTAssertEqual(report.findings[1].detail, "/Users/op/.deadreckon/config.toml missing")
        XCTAssertEqual(report.verdict?.label, "blocked doctor")
        XCTAssertEqual(report.status, "blocked")
        XCTAssertEqual(report.failedCount, 1)
        // binary_health decodes the live-corroborated fields.
        XCTAssertEqual(report.binaryHealth?.currentVersion, "0.8.4")
        XCTAssertEqual(report.binaryHealth?.installations.count, 2)
        XCTAssertEqual(report.binaryHealth?.installations.first?.channel, "shell")
        XCTAssertEqual(report.binaryHealth?.installations.first?.updateCommand,
                       "deadreckon update")
        XCTAssertEqual(report.binaryHealth?.conflicts.count, 2)
        XCTAssertTrue(report.binaryHealth?.conflicts.first?.contains("0.8.1") ?? false)
        XCTAssertEqual(report.binaryHealth?.gateProtocolVersion, 1)
        XCTAssertEqual(report.binaryHealth?.gateHelperCompatible, true)
        // Sandboxes carry the doctor notes verbatim (the `none` warning).
        XCTAssertEqual(report.sandboxes.count, 3)
        XCTAssertEqual(report.sandboxes.last?.note,
                       "available but unsafe; use only when explicitly requested")
        // The raw document is retained byte-for-byte for the disclosure.
        XCTAssertTrue(report.rawJSON.contains("\"kind\": \"doctor\""))
        XCTAssertEqual(store.lastChecked, Date(timeIntervalSince1970: 1_000))
    }

    /// SHIPPED has no per-finding repairable flag: the section-level
    /// capability derives from the binary-health booleans or a failed
    /// supervisor-service finding — and is absent otherwise (no dead
    /// control, no repair theater).
    func testRepairCapabilityProbesTheShippedSignals() throws {
        func report(_ json: String) throws -> DoctorReportEnvelope {
            try XCTUnwrap(DoctorReportEnvelope(data: Data(json.utf8)))
        }
        XCTAssertFalse(try report(SettingsFixtures.doctorReport()).repairAvailable)
        XCTAssertTrue(try report(
            SettingsFixtures.doctorReport(repairableReceipt: true)).repairAvailable)
        XCTAssertTrue(try report(
            SettingsFixtures.doctorReport(supervisorFindingStatus: "failed")).repairAvailable)
    }

    func testRepairParsesOutcomeRowsAndReplacesTheFindings() async {
        let cli = SettingsFakeCLI()
        cli.script("doctor --repair --json",
                   stdout: SettingsFixtures.doctorReport(
                       repairs: SettingsFixtures.doctorRepairs, repairableReceipt: true))
        let store = DoctorStore(cli: cli)

        await store.runRepair()

        XCTAssertEqual(cli.calls.first, ["doctor", "--repair", "--json"])
        guard case .completed(let report) = store.repair else {
            return XCTFail("expected completed, got \(store.repair)")
        }
        XCTAssertEqual(report.repairs.map(\.attempted),
                       ["repair active installation", "repair install receipt",
                        "repair supervisor service"])
        XCTAssertEqual(report.repairs.last?.result, "failed")
        XCTAssertEqual(report.repairs.last?.detail, "left unchanged: unit is unmanaged")
        // The post-repair document replaces the findings table (fresh
        // report embedded in the same envelope).
        guard case .loaded = store.state else {
            return XCTFail("repair must refresh the findings from its own document")
        }
    }

    func testUnavailableKeepsTheFailureWordsVerbatim() async {
        let cli = SettingsFakeCLI()
        cli.scriptFailure("doctor --json",
                          FleetCLIError.binaryUnavailable("The bundled CLI binary is missing"))
        let store = DoctorStore(cli: cli)

        await store.run()

        guard case .unavailable(let words) = store.state else {
            return XCTFail("expected unavailable, got \(store.state)")
        }
        XCTAssertEqual(words, "The bundled CLI binary is missing")
        XCTAssertNil(store.lastChecked)
    }

    /// A repair refusal (shared G1 error envelope, verb "doctor") renders
    /// verbatim and leaves the previous findings alone.
    func testRepairRefusalIsAuthoritative() async {
        let cli = SettingsFakeCLI()
        cli.script("doctor --json", stdout: SettingsFixtures.doctorReport())
        cli.script("doctor --repair --json", stdout: """
        {"kind": "error", "code": 1, "verb": "doctor",
         "message": "repair refused: the unit is unmanaged", "try_lines": ["deadreckon supervisor status"]}
        """, exitCode: 1)
        let store = DoctorStore(cli: cli)
        await store.run()

        await store.runRepair()

        guard case .refused(let refusal) = store.repair else {
            return XCTFail("expected refused, got \(store.repair)")
        }
        XCTAssertTrue(refusal.message.contains("repair refused"))
        guard case .loaded(let report) = store.state else {
            return XCTFail("previous findings must survive a refused repair")
        }
        XCTAssertEqual(report.findings.count, 4)
    }
}
