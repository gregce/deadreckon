import XCTest

@testable import DeadreckonKit

/// ConfigStore: the capability probe (decode-or-degrade against the SHIPPED
/// `config show --json` shape), the write-then-re-read discipline, and the
/// one redaction rule — no api key byte ever lands in any model, command
/// line, or state description.
@MainActor
final class ConfigStoreTests: XCTestCase {

    func testProbeArmsFromTheShippedShowEnvelope() async {
        let cli = SettingsFakeCLI()
        cli.script("config show", stdout: SettingsFixtures.configShow)
        let store = ConfigStore(cli: cli)

        await store.load()

        guard case .armed(let envelope) = store.capability else {
            return XCTFail("expected armed, got \(store.capability)")
        }
        XCTAssertEqual(envelope.configPath, "/Users/op/.deadreckon/config.toml")
        XCTAssertTrue(envelope.configExists)
        // Per-key set-vs-default provenance, from the shipped `settings` map.
        XCTAssertEqual(envelope.settings["defaults.provider"]?.isSet, true)
        XCTAssertEqual(envelope.settings["defaults.sandbox"]?.isSet, false)
        XCTAssertEqual(envelope.displayValue("defaults.provider"), "cli:claude-code")
        // Integers stay integers in display ("15", never "15.0").
        XCTAssertEqual(envelope.displayValue("defaults.max_spend"), "15")
        XCTAssertEqual(envelope.displayValue("defaults.cli_max_wall_seconds"), "36000")
        // Null value = unset with no pinned default: nil, never a guess.
        XCTAssertNil(envelope.displayValue("defaults.model"))
        // Key state is structural: api_key slot present == configured.
        XCTAssertTrue(envelope.keyConfigured(route: "anthropic"))
        XCTAssertFalse(envelope.keyConfigured(route: "cli:claude-code"))
        XCTAssertFalse(envelope.keyConfigured(route: "missing"))
        // The verdict block rides along verbatim.
        XCTAssertEqual(envelope.verdict?.label, "completed config show")
        XCTAssertEqual(envelope.verdict?.evidencePairs.first?.0, "config")
    }

    /// The vendored 0.8.4 binary predates `config show`: clap prose, exit
    /// 2, no envelope. The section must degrade with the words — never a
    /// dead control, never a guessed value.
    func testProbeDegradesOnOlderBinaryClapProse() async {
        let cli = SettingsFakeCLI()
        cli.script("config show", stdout: "",
                   stderr: SettingsFixtures.configShowOlderBinaryProse, exitCode: 2)
        let store = ConfigStore(cli: cli)

        await store.load()

        guard case .degraded(let words) = store.capability else {
            return XCTFail("expected degraded, got \(store.capability)")
        }
        XCTAssertTrue(words.contains("unrecognized subcommand 'show'"),
                      "the binary's own words must render verbatim")
    }

    func testProbeDegradesWhenTheBinaryIsUnavailable() async {
        let cli = SettingsFakeCLI()
        cli.scriptFailure("config show", FleetCLIError.binaryUnavailable("no trusted binary"))
        let store = ConfigStore(cli: cli)

        await store.load()

        guard case .degraded(let words) = store.capability else {
            return XCTFail("expected degraded, got \(store.capability)")
        }
        XCTAssertEqual(words, "no trusted binary")
    }

    /// Rule 1: file truth is re-read after every write; rendered values come
    /// only from the fresh read. The set's own echo (value 25) must NOT
    /// paint the store while the re-read still says 15.
    func testSetDispatchesThenRendersFromTheReReadOnly() async {
        let cli = SettingsFakeCLI()
        cli.script("config show", stdout: SettingsFixtures.configShow)
        let store = ConfigStore(cli: cli)
        await store.load()

        cli.script("config set", stdout: SettingsFixtures.configSetCompleted)
        // The re-read is scripted to STILL say 15: a lagging file must win
        // over the write's echo.
        await store.set(key: "defaults.max_spend", value: "25")

        XCTAssertEqual(cli.calls(withPrefix: "config set").first,
                       ["config", "set", "--json", "--", "defaults.max_spend", "25"])
        // Dispatch order: set, then the show re-read.
        let joined = cli.calls.map { $0.prefix(2).joined(separator: " ") }
        XCTAssertEqual(joined, ["config show", "config set", "config show"])
        guard case .completed(let key, let word) = store.writeState else {
            return XCTFail("expected completed, got \(store.writeState)")
        }
        XCTAssertEqual(key, "defaults.max_spend")
        XCTAssertEqual(word, "completed")
        // The rendered value is the RE-READ's (still 15), not the echo's 25.
        XCTAssertEqual(store.envelope?.displayValue("defaults.max_spend"), "15")

        // Once the file actually changes, the next load renders it.
        cli.script("config show", stdout: SettingsFixtures.configShowAfterSet)
        await store.load()
        XCTAssertEqual(store.envelope?.displayValue("defaults.max_spend"), "25")
    }

    func testRefusedWriteRendersTheRefusalVerbatimAndKeepsFileTruth() async {
        let cli = SettingsFakeCLI()
        cli.script("config show", stdout: SettingsFixtures.configShow)
        let store = ConfigStore(cli: cli)
        await store.load()

        cli.script("config set", stdout: SettingsFixtures.configSetRefusal, exitCode: 1)
        await store.set(key: "defaults.max_spend", value: "banana")

        guard case .refused(let refusal) = store.writeState else {
            return XCTFail("expected refused, got \(store.writeState)")
        }
        XCTAssertTrue(refusal.message.contains("expects a number greater than zero"))
        XCTAssertEqual(refusal.tryLines, ["deadreckon config set defaults.max_spend <value>"])
        // The armed truth is untouched (and was re-read anyway).
        XCTAssertEqual(store.envelope?.displayValue("defaults.max_spend"), "15")
    }

    func testUnsetArgvAndReRead() async {
        let cli = SettingsFakeCLI()
        cli.script("config show", stdout: SettingsFixtures.configShow)
        let store = ConfigStore(cli: cli)
        await store.load()

        cli.script("config unset", stdout: SettingsFixtures.configSetCompleted)
        await store.unset(key: "defaults.max_spend")

        XCTAssertEqual(cli.calls(withPrefix: "config unset").first,
                       ["config", "unset", "--json", "--", "defaults.max_spend"])
        XCTAssertEqual(cli.calls.last?.prefix(2).joined(separator: " "), "config show")
    }

    // MARK: - The one redaction rule (spec rule 4)

    /// The secret travels ONLY as stdin bytes: argv carries the route
    /// alone, the recorded stdin is the exact key, and the re-read follows.
    func testSaveKeySendsTheSecretOverStdinNeverArgv() async {
        let secret = "sk-ant-SECRET-9f8e7d6c"
        let cli = SettingsFakeCLI()
        cli.script("config show", stdout: SettingsFixtures.configShow)
        let store = ConfigStore(cli: cli)
        await store.load()

        cli.script("config set-key", stdout: SettingsFixtures.configSetKeyCompleted)
        await store.saveKey(route: "anthropic", secret: secret)

        XCTAssertEqual(cli.calls(withPrefix: "config set-key").first,
                       ["config", "set-key", "--json", "--", "anthropic"])
        XCTAssertEqual(cli.stdin(forCallWithPrefix: "config set-key"), Data(secret.utf8))
        for argv in cli.calls {
            for word in argv {
                XCTAssertFalse(word.contains(secret), "secret leaked into argv: \(argv)")
            }
        }
        // Write resolved from the envelope; file truth re-read after.
        guard case .completed = store.writeState else {
            return XCTFail("expected completed, got \(store.writeState)")
        }
        XCTAssertEqual(cli.calls.last?.prefix(2).joined(separator: " "), "config show")
    }

    /// The redaction pin: after the full save-key flow, NO observable state
    /// on the store — capability envelope, write state, last command —
    /// contains a single byte of the secret. Key state renders only from
    /// the show envelope's structural "configured" marker.
    func testNoAPIKeyByteEverLandsInAnyModelOrCommand() async {
        let secret = "sk-ant-SECRET-9f8e7d6c"
        let cli = SettingsFakeCLI()
        cli.script("config show", stdout: SettingsFixtures.configShow)
        let store = ConfigStore(cli: cli)
        await store.load()
        cli.script("config set-key", stdout: SettingsFixtures.configSetKeyCompleted)

        await store.saveKey(route: "anthropic", secret: secret)

        let observable = [
            String(describing: store.capability),
            String(describing: store.writeState),
            store.lastCommand ?? "",
        ].joined(separator: "\n")
        XCTAssertFalse(observable.contains(secret),
                       "an api key byte reached observable store state")
        // The command well line for set-key is the argv truth: route only.
        XCTAssertEqual(store.lastCommand,
                       "deadreckon config set-key --json -- anthropic")
        // Key state is the marker, never material.
        XCTAssertTrue(store.envelope?.keyConfigured(route: "anthropic") ?? false)
    }

    func testRemoveKeyDispatchesUnsetKey() async {
        let cli = SettingsFakeCLI()
        cli.script("config show", stdout: SettingsFixtures.configShow)
        let store = ConfigStore(cli: cli)
        await store.load()

        cli.script("config unset-key", stdout: SettingsFixtures.configSetKeyCompleted
            .replacingOccurrences(of: "set-key", with: "unset-key"))
        await store.removeKey(route: "anthropic")

        XCTAssertEqual(cli.calls(withPrefix: "config unset-key").first,
                       ["config", "unset-key", "--json", "--", "anthropic"])
    }

    /// An empty pasted secret dispatches nothing (no pointless child, no
    /// misleading "writing…" flash).
    func testEmptySecretDispatchesNothing() async {
        let cli = SettingsFakeCLI()
        cli.script("config show", stdout: SettingsFixtures.configShow)
        let store = ConfigStore(cli: cli)
        await store.load()

        await store.saveKey(route: "anthropic", secret: "   ")

        XCTAssertTrue(cli.calls(withPrefix: "config set-key").isEmpty)
    }
}
