import Foundation

/// Settings > Service engine (SETTINGS spec §S3): one honest verdict word +
/// evidence from `supervisor status --json`, lifecycle verbs through the
/// one dispatcher, and the resolution discipline — a lifecycle sheet
/// resolves on its envelope, then status is RE-POLLED before the verdict
/// repaints (two sources, fresh).
///
/// The typed display verdict derives from the SHIPPED v4 two-source truth
/// (verdict healthy|degraded|foreign_home|down + service + installed).
/// Older binaries: the healthy path is a v3 report (typed fields absent)
/// and the checkpoint-absent path is a prose refusal on stderr — both
/// degrade honestly (`.unknown` with the words verbatim, or the v3
/// installed/loaded derivation).
@MainActor
public final class ServiceController: ObservableObject {
    /// The §S3 verdict row vocabulary, typed. Words live in the app's
    /// Lexicon; this enum carries the classification only.
    public enum DisplayVerdict: Equatable {
        case running
        case stopped
        case notInstalled
        /// Installed unit is stale for this binary/home (spec "Outdated").
        case outdated
        /// FULL-DRIVE B5a: the manager reports running but the checkpoint
        /// belongs to a different home (or is absent).
        case runningForeignHome
        /// Running with a degradation fact (not enabled / stale checkpoint
        /// on this home); the reason is quoted beside the word.
        case degraded
        case unsupported
        case unknown
    }

    public enum StatusState: Equatable {
        case idle
        case loading
        case loaded(ServiceStatusReport)
        /// No decodable report: the words render verbatim (the pre-v4
        /// checkpoint-absent refusal is exactly this).
        case unavailable(String)
    }

    /// One lifecycle action's state (install / start / stop share it — at
    /// most one runs at a time).
    public enum ActionState: Equatable {
        case idle
        case running(String)
        case completed(SupervisorLifecycleEnvelope)
        case refused(ErrorEnvelope)
        case failed(String)
    }

    @Published public private(set) var status: StatusState = .idle
    @Published public private(set) var action: ActionState = .idle

    private let cli: FleetCLIRunning
    private let runner: MutationRunner

    public init(cli: FleetCLIRunning) {
        self.cli = cli
        self.runner = MutationRunner(cli: cli)
    }

    public func commandLine(for verb: PlannedVerb) -> String {
        runner.literalCommandLine(for: verb)
    }

    // MARK: - Status (read)

    public func refreshStatus() async {
        if case .loading = status { return }
        status = .loading
        do {
            let result = try await cli.run(
                arguments: ["supervisor", "status", "--json"], timeout: 60)
            if let report = ServiceStatusReport(data: Data(result.stdout.utf8)) {
                status = .loaded(report)
            } else {
                let words = result.stderr.isEmpty ? result.stdout : result.stderr
                status = .unavailable(words.isEmpty
                    ? "exit \(result.exitCode) with no output"
                    : String(words.prefix(600)))
            }
        } catch {
            status = .unavailable(
                (error as? FleetCLIError)?.errorDescription ?? error.localizedDescription)
        }
    }

    /// The one honest word over the loaded report. Two sources, never
    /// averaged into a guess: the mapping types on the shipped verdict and
    /// quotes `verdict_reason` verbatim beside any degraded word.
    public static func displayVerdict(for report: ServiceStatusReport) -> DisplayVerdict {
        if report.manager == "unsupported" || report.installed == .unsupported {
            return .unsupported
        }
        switch report.verdict {
        case .healthy:
            return .running
        case .foreignHome:
            return .runningForeignHome
        case .degraded:
            // A stale managed unit is the spec's "Outdated"; every other
            // degradation keeps the word with its reason quoted.
            return report.installed == .stale ? .outdated : .degraded
        case .down:
            switch report.service {
            case .notInstalled: return .notInstalled
            case .stopped, .running: return report.installed == .stale ? .outdated : .stopped
            case .unknown, nil: break
            }
            // v3 fallback: no typed service word — derive from installed +
            // manager runtime, the pre-v4 chip logic.
            fallthrough
        case .unknown, nil:
            if report.installed == .notInstalled { return .notInstalled }
            if report.installed == .stale { return .outdated }
            let managerRunning = report.loaded == true || report.active == "active"
            if report.installed == .current {
                return managerRunning ? .running : .stopped
            }
            return .unknown
        }
    }

    public var displayVerdict: DisplayVerdict {
        switch status {
        case .loaded(let report): return Self.displayVerdict(for: report)
        case .idle, .loading, .unavailable: return .unknown
        }
    }

    // MARK: - Lifecycle (write, then re-poll)

    public func install() async { await dispatch(.supervisorInstall, word: "install") }
    public func start() async { await dispatch(.supervisorStart, word: "start") }
    public func stop() async { await dispatch(.supervisorStop, word: "stop") }

    public func resetAction() {
        if case .running = action { return }
        action = .idle
    }

    private func dispatch(_ verb: PlannedVerb, word: String) async {
        if case .running = action { return }
        action = .running(word)
        let result = await runner.run(verb)
        if let refusal = result.refusal {
            action = .refused(refusal)
        } else if let object = result.rawObjects.last,
                  let envelope = SupervisorLifecycleEnvelope(data: object) {
            action = .completed(envelope)
        } else {
            action = .failed(result.envelopeFreeWords)
        }
        // Resolution discipline (§P6): the envelope resolves the sheet;
        // the verdict row repaints only from a fresh two-source poll.
        await refreshStatus()
    }
}
