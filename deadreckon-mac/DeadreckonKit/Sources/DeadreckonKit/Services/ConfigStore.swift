import Foundation

/// Settings > General / Agents & Keys engine (SETTINGS spec §S1/§S2).
///
/// Capability probe first: `config show --json` decode-or-degrade. The
/// vendored 0.8.4 binary predates the config envelopes entirely ("error:
/// unrecognized subcommand 'show'", exit 2), so the degraded state is a
/// first-class citizen: read-only banner + terminal handoff, never a dead
/// control and never a guessed value.
///
/// Write discipline (spec rule 1): every mutation dispatches one
/// MutationRunner verb and then RE-READS `config show --json`; rendered
/// values only ever come from a fresh read, never from the write's echo.
///
/// The one redaction rule (spec rule 4): API keys enter through
/// `saveKey(route:secret:)`, travel as stdin bytes through the CLI seam,
/// and are never stored on this object, never echoed into `lastCommand`
/// (argv carries only the route), and never read back — key state renders
/// exclusively from the show envelope's structural redaction
/// ("configured" / nothing).
@MainActor
public final class ConfigStore: ObservableObject {
    /// The section's capability state, decided by the probe.
    public enum Capability: Equatable {
        case unknown
        case probing
        /// The envelope landed: controls arm themselves from it.
        case armed(ConfigShowEnvelope)
        /// No machine envelope (older binary, launch failure, refusal):
        /// the words are rendered verbatim with the CLI escape hatch.
        case degraded(String)
    }

    /// One write's lifecycle. The command well shows the LAST dispatched
    /// command and its verdict word (§S1); failures render inline.
    public enum WriteState: Equatable {
        case idle
        case writing(key: String)
        /// The envelope's own status word ("completed" / "no-op").
        case completed(key: String, statusWord: String)
        case refused(ErrorEnvelope)
        case failed(String)
    }

    @Published public private(set) var capability: Capability = .unknown
    @Published public private(set) var writeState: WriteState = .idle
    /// The literal CLI line of the last dispatched write (display-only,
    /// from PlannedVerb argv — a set-key line never contains the secret).
    @Published public private(set) var lastCommand: String?

    private let cli: FleetCLIRunning
    private let runner: MutationRunner

    public init(cli: FleetCLIRunning) {
        self.cli = cli
        self.runner = MutationRunner(cli: cli)
    }

    /// The armed envelope, when the probe succeeded.
    public var envelope: ConfigShowEnvelope? {
        if case .armed(let envelope) = capability { return envelope }
        return nil
    }

    public func commandLine(for verb: PlannedVerb) -> String {
        runner.literalCommandLine(for: verb)
    }

    // MARK: - Probe / read

    /// One `config show --json` read: decode-or-degrade. Also the re-read
    /// leg after every write.
    public func load() async {
        if case .probing = capability { return }
        if case .unknown = capability { capability = .probing }
        do {
            let result = try await cli.run(arguments: ["config", "show", "--json"], timeout: 60)
            if let envelope = ConfigShowEnvelope(data: Data(result.stdout.utf8)) {
                capability = .armed(envelope)
                return
            }
            // A typed refusal is still a degraded section (config show has
            // no refusal path today, but fail closed either way).
            let classified = MutationResult.classify(
                stdout: result.stdout, stderr: result.stderr, exitCode: result.exitCode)
            if let refusal = classified.refusal {
                capability = .degraded(refusal.message)
            } else {
                capability = .degraded(classified.envelopeFreeWords)
            }
        } catch {
            capability = .degraded(
                (error as? FleetCLIError)?.errorDescription ?? error.localizedDescription)
        }
    }

    // MARK: - Writes (dispatch, then re-read; never optimistic)

    public func set(key: String, value: String) async {
        await dispatch(.configSet(key: key, value: value), key: key, stdin: nil)
    }

    public func unset(key: String) async {
        await dispatch(.configUnset(key: key), key: key, stdin: nil)
    }

    /// Store an API key for an API-key route. The secret string is turned
    /// into stdin bytes HERE and passed straight through the dispatch —
    /// it is never assigned to any property, never appears in the argv or
    /// `lastCommand`, and this method's failure words never include it.
    public func saveKey(route: String, secret: String) async {
        let trimmed = secret.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        await dispatch(.configSetKey(route: route), key: route, stdin: Data(trimmed.utf8))
    }

    public func removeKey(route: String) async {
        await dispatch(.configUnsetKey(route: route), key: route, stdin: nil)
    }

    private func dispatch(_ verb: PlannedVerb, key: String, stdin: Data?) async {
        // In-flight guard: one write at a time; a double-click's second
        // task dispatches nothing.
        if case .writing = writeState { return }
        writeState = .writing(key: key)
        lastCommand = runner.literalCommandLine(for: verb)
        let result = await runner.run(verb, stdin: stdin)
        if let refusal = result.refusal {
            writeState = .refused(refusal)
        } else if let object = result.rawObjects.last,
                  let envelope = ConfigWriteEnvelope(data: object) {
            writeState = .completed(key: key, statusWord: envelope.status ?? "completed")
        } else {
            writeState = .failed(result.envelopeFreeWords)
        }
        // File truth: whatever happened, re-read before anything renders a
        // value (a refused write leaves the old truth on screen — which is
        // exactly what the file still says).
        await load()
    }
}
