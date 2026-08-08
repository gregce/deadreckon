import Foundation

/// Settings > Health / Binaries engine (SETTINGS spec §S5/§S6): the full
/// `doctor --json` document retained verbatim (the raw-report disclosure is
/// the evidence floor), findings rendered in the binary's own order, and
/// repair through the one dispatcher with outcomes from the report's own
/// repairs[] rows.
///
/// Repair capability is a probe, not a hardcoded label: SHIPPED doctor has
/// no per-finding `repairable` flag, so ONE section-level [Repair…] renders,
/// gated on `DoctorReportEnvelope.repairAvailable` (the binary-health
/// repairable booleans / a failed supervisor-service finding).
@MainActor
public final class DoctorStore: ObservableObject {
    public enum State: Equatable {
        case idle
        case loading
        case loaded(DoctorReportEnvelope)
        /// No decodable document: the words verbatim + the CLI escape hatch.
        case unavailable(String)
    }

    public enum RepairState: Equatable {
        case idle
        case running
        /// The post-repair document (its repairs[] rows carry the
        /// attempted/result/detail outcomes).
        case completed(DoctorReportEnvelope)
        case refused(ErrorEnvelope)
        case failed(String)
    }

    @Published public private(set) var state: State = .idle
    @Published public private(set) var repair: RepairState = .idle
    @Published public private(set) var lastChecked: Date?

    private let cli: FleetCLIRunning
    private let runner: MutationRunner
    /// Injectable clock (test determinism).
    public var nowProvider: () -> Date = { Date() }

    public init(cli: FleetCLIRunning) {
        self.cli = cli
        self.runner = MutationRunner(cli: cli)
    }

    public var report: DoctorReportEnvelope? {
        if case .loaded(let report) = state { return report }
        return nil
    }

    public func commandLine(for verb: PlannedVerb) -> String {
        runner.literalCommandLine(for: verb)
    }

    /// One `doctor --json` run (also the manual refresh for every doctor
    /// consumer in Settings).
    public func run() async {
        if case .loading = state { return }
        state = .loading
        do {
            let result = try await cli.run(arguments: ["doctor", "--json"], timeout: 120)
            if let report = DoctorReportEnvelope(data: Data(result.stdout.utf8)) {
                state = .loaded(report)
                lastChecked = nowProvider()
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

    /// `doctor --repair --json`: the binary's three bounded repairs. The
    /// post-repair document lands in `repair` (outcome rows) AND replaces
    /// the findings table (it embeds a fresh report) — file truth re-read
    /// in the same envelope.
    public func runRepair() async {
        if case .running = repair { return }
        repair = .running
        let result = await runner.run(.doctorRepair)
        if let refusal = result.refusal {
            repair = .refused(refusal)
        } else if let object = result.rawObjects.last,
                  let report = DoctorReportEnvelope(data: object) {
            repair = .completed(report)
            state = .loaded(report)
            lastChecked = nowProvider()
        } else {
            repair = .failed(result.envelopeFreeWords)
        }
    }

    public func resetRepair() {
        if case .running = repair { return }
        repair = .idle
    }
}
