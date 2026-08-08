import XCTest

@testable import DeadreckonKit

/// §R3: the library read contract. The fixture mirrors the LIVE 0.8.4
/// envelope (real home, 2026-08-07): kind "library_list", artifacts[] of
/// {manifest, path, materialized_count}, with payload_files/payload_bytes
/// integer fields ABSENT on schema_version-1 manifests.
@MainActor
final class LibraryStoreTests: XCTestCase {
    func testDecodesLiveShape() async throws {
        let cli = SettingsFakeCLI()
        cli.script("library list", stdout: Self.envelope)
        let store = LibraryStore(cli: cli)
        await store.load()
        let envelope = try XCTUnwrap(store.envelope)
        XCTAssertEqual(envelope.artifacts.count, 2)
        let rich = envelope.artifacts[0]
        XCTAssertEqual(rich.manifest.runID, "e024632cbd34889535d526563b14d593")
        XCTAssertEqual(rich.manifest.scope, "task-5-265804aa")
        XCTAssertEqual(rich.manifest.payloadFiles, 68)
        XCTAssertEqual(rich.manifest.payloadBytes, 2_112_058)
        XCTAssertNotNil(rich.manifest.promotedAt, "RFC3339 with fractional seconds must parse")
        XCTAssertEqual(rich.path, "/Users/op/.deadreckon/library/task-5-265804aa/e024632cbd34889535d526563b14d593")
        // Older manifest: size facts absent stay nil, never invented.
        let sparse = envelope.artifacts[1]
        XCTAssertNil(sparse.manifest.payloadFiles)
        XCTAssertNil(sparse.manifest.payloadBytes)
    }

    /// Default scope is ALL projects (the app's seat is not a project
    /// directory); the toggle drops --all for the current-scope view.
    func testScopeToggleRidesAllFlag() async {
        let cli = SettingsFakeCLI()
        cli.script("library list", stdout: Self.envelope)
        let store = LibraryStore(cli: cli)
        await store.load()
        XCTAssertEqual(cli.calls.first, ["library", "list", "--all", "--json"])
        store.allProjects = false
        await store.load()
        XCTAssertEqual(cli.calls.last, ["library", "list", "--json"])
    }

    /// One bad artifact row costs exactly that row, counted; siblings
    /// survive (the fleet quarantine discipline).
    func testUnreadableRowIsCountedNotFatal() async throws {
        let cli = SettingsFakeCLI()
        cli.script("library list", stdout: Self.envelopeWithBadRow)
        let store = LibraryStore(cli: cli)
        await store.load()
        let envelope = try XCTUnwrap(store.envelope)
        XCTAssertEqual(envelope.artifacts.count, 1)
        XCTAssertEqual(envelope.unreadableCount, 1)
    }

    func testDegradedCarriesTheFailingWords() async {
        let cli = SettingsFakeCLI()
        cli.script("library list", stdout: "", stderr: "error: home unreadable", exitCode: 1)
        let store = LibraryStore(cli: cli)
        await store.load()
        guard case .unavailable(let words) = store.state else {
            return XCTFail("expected unavailable, got \(store.state)")
        }
        XCTAssertTrue(words.contains("home unreadable"))
    }

    func testClientFilterMatchesGoalScopeAndRunID() throws {
        let envelope = try XCTUnwrap(LibraryListEnvelope(data: Data(Self.envelope.utf8)))
        let all = envelope.artifacts
        XCTAssertEqual(LibraryStore.filter(all, query: "").count, 2)
        XCTAssertEqual(LibraryStore.filter(all, query: "flaky").map(\.manifest.runID),
                       ["11aa22bb33cc44dd55ee66ff77aa88bb"])
        XCTAssertEqual(LibraryStore.filter(all, query: "task-5").count, 1)
        XCTAssertEqual(LibraryStore.filter(all, query: "E024632C").count, 1,
                       "run-id match is case-insensitive")
        XCTAssertTrue(LibraryStore.filter(all, query: "no-such-thing").isEmpty)
    }

    private static let envelope = """
    {
      "artifacts": [
        {
          "manifest": {
            "capture_policy_sha256": "sha256:e7f9774db5441e8a35181a5a38dfb09f68ab8122",
            "goal": "Ship the durable ledger rewrite",
            "payload_bytes": 2112058,
            "payload_files": 68,
            "payload_tree_sha256": "sha256:823296f9e2f4d4b62ffe0d827fdbc395",
            "promoted_at": "2026-08-07T07:29:59.179469Z",
            "provenance_hash": "9fb20b7fcb0278e0",
            "run_id": "e024632cbd34889535d526563b14d593",
            "schema_version": 2,
            "scope": "task-5-265804aa",
            "source_working_dir": "/Users/op/.deadreckon/worktrees/task-5-265804aa-e024632c"
          },
          "materialized_count": 0,
          "path": "/Users/op/.deadreckon/library/task-5-265804aa/e024632cbd34889535d526563b14d593"
        },
        {
          "manifest": {
            "goal": "Fix the flaky ledger tail tests",
            "promoted_at": "2026-08-01T12:00:00Z",
            "provenance_hash": "0011223344556677",
            "run_id": "11aa22bb33cc44dd55ee66ff77aa88bb",
            "schema_version": 1,
            "scope": "deadreckon-mac",
            "source_working_dir": "/Users/op/deadreckon/deadreckon-mac"
          },
          "materialized_count": 1,
          "path": "/Users/op/.deadreckon/library/deadreckon-mac/11aa22bb33cc44dd55ee66ff77aa88bb"
        }
      ],
      "id": "all-scopes",
      "kind": "library_list",
      "next_actions": ["deadreckon finish <id>", "deadreckon export <id>"],
      "paths": { "home": "/Users/op/.deadreckon" },
      "status": "ok",
      "try_lines": []
    }
    """

    private static let envelopeWithBadRow = """
    {
      "kind": "library_list", "id": "all-scopes", "status": "ok",
      "next_actions": [], "try_lines": [],
      "artifacts": [
        { "path": "/Users/op/.deadreckon/library/x/y" },
        {
          "manifest": {
            "goal": "Fix the flaky ledger tail tests",
            "promoted_at": "2026-08-01T12:00:00Z",
            "run_id": "11aa22bb33cc44dd55ee66ff77aa88bb",
            "schema_version": 1,
            "scope": "deadreckon-mac"
          },
          "materialized_count": 0,
          "path": "/Users/op/.deadreckon/library/deadreckon-mac/11aa22bb33cc44dd55ee66ff77aa88bb"
        }
      ]
    }
    """
}
