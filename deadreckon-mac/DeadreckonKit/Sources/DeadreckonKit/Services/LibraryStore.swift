import Foundation

/// The Library browser engine (SETTINGS-SCREENS-SPEC §R3): a read-only
/// table over `library list --json` (real at 0.8.4). Scope toggle rides the
/// documented `--all` flag; filtering is CLIENT-SIDE over goal / scope /
/// run id — `library search` has no `--json` (live) and a local filter over
/// a weekly-sized list covers the journey, so the binary verb stays CLI
/// until it grows an envelope.
///
/// Default scope is ALL projects: the app's CLI client does not run from a
/// project directory, so "current scope" resolves to nothing from the
/// app's seat — an empty this-project view would be noise, not truth. The
/// toggle stays for operators who launch the app from a project shell.
@MainActor
public final class LibraryStore: ObservableObject {
    public enum State: Equatable {
        case idle
        case loading
        case loaded(LibraryListEnvelope)
        /// The failing surface's own words, verbatim — never fake rows.
        case unavailable(String)
    }

    @Published public private(set) var state: State = .idle
    /// The scope toggle (§R3 header). Changing it re-loads.
    @Published public var allProjects: Bool = true

    private let cli: FleetCLIRunning

    public init(cli: FleetCLIRunning) {
        self.cli = cli
    }

    public var envelope: LibraryListEnvelope? {
        if case .loaded(let envelope) = state { return envelope }
        return nil
    }

    public func load() async {
        if case .loading = state { return }
        state = .loading
        var arguments = ["library", "list"]
        if allProjects { arguments.append("--all") }
        arguments.append("--json")
        do {
            let result = try await cli.run(arguments: arguments, timeout: 60)
            if let envelope = LibraryListEnvelope(data: Data(result.stdout.utf8)) {
                state = .loaded(envelope)
            } else {
                let words = result.stderr.isEmpty ? result.stdout : result.stderr
                state = .unavailable(words.isEmpty
                    ? "exit \(result.exitCode) with no output"
                    : String(words.prefix(600)))
            }
        } catch {
            state = .unavailable(
                (error as? FleetCLIError)?.errorDescription ?? error.localizedDescription)
        }
    }

    /// Pure client-side filter over goal, scope, and run id,
    /// case-insensitive. An empty query returns everything.
    public static func filter(_ artifacts: [LibraryArtifact], query: String)
        -> [LibraryArtifact] {
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return artifacts }
        return artifacts.filter { artifact in
            artifact.manifest.goal.localizedCaseInsensitiveContains(trimmed)
                || artifact.manifest.scope.localizedCaseInsensitiveContains(trimmed)
                || artifact.manifest.runID.localizedCaseInsensitiveContains(trimmed)
        }
    }
}
