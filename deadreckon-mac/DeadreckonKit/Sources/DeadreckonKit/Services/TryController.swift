import Foundation

/// The first-run "prove it" engine (SETTINGS-SCREENS-SPEC §R2 row 5):
/// dispatches `try --json` — a REAL tiny run in a throwaway workspace, no
/// credentials needed, minutes not seconds — through the one dispatcher and
/// renders only what the proof document says. The binary's trust words
/// ("untrusted local smoke diagnostic" / "local smoke gate evidence only;
/// not a trusted Job receipt") always render verbatim: the proof row never
/// claims more than the binary claimed.
@MainActor
public final class TryController: ObservableObject {
    public enum State: Equatable {
        case idle
        /// Running since `startedAt`; the row shows elapsed time because
        /// this legitimately takes minutes.
        case running(startedAt: Date)
        case proof(TryProofEnvelope)
        case refused(ErrorEnvelope)
        /// No decodable proof document: the words verbatim + exit code.
        case failed(String)
    }

    @Published public private(set) var state: State = .idle

    /// Injectable clock (tests).
    public var nowProvider: () -> Date = { Date() }

    private let runner: MutationRunner

    public init(cli: FleetCLIRunning) {
        runner = MutationRunner(cli: cli)
    }

    public var commandLine: String {
        runner.literalCommandLine(for: .tryProof)
    }

    public func run() async {
        // In-flight guard: one proof at a time; a double-click's second
        // task dispatches nothing.
        if case .running = state { return }
        state = .running(startedAt: nowProvider())
        let result = await runner.run(.tryProof)
        if let refusal = result.refusal {
            state = .refused(refusal)
        } else if let object = result.rawObjects.last,
                  let proof = TryProofEnvelope(data: object) {
            state = .proof(proof)
        } else {
            state = .failed(result.envelopeFreeWords)
        }
    }
}
