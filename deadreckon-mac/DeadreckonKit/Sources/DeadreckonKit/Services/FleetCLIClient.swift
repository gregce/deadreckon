import Foundation

/// The read-only CLI seam FleetStore polls through. Tests inject a fake;
/// the app injects `DeadreckonCLIClient`. Every invocation is one child
/// process of the manifest-pinned binary (trust rule 8: the app never
/// touches DEADRECKON_HOME itself).
public protocol FleetCLIRunning: AnyObject {
    /// Runs `deadreckon <arguments>` to completion. Throws
    /// `FleetCLIError.binaryUnavailable` when no trusted binary exists;
    /// a nonzero exit is an outcome in the result, not an error.
    func run(arguments: [String], timeout: TimeInterval) async throws -> CLIRunResult

    /// SIGTERM every in-flight child (SIGKILL after `patience`); returns how
    /// many children were signaled. Quit-time teardown calls this so no
    /// child is orphaned.
    @discardableResult
    func terminateInFlight(patience: TimeInterval) -> Int

    /// How many children are still running right now. Quit-time teardown
    /// polls this after the SIGTERM sweep so the process outlives its
    /// children (or the SIGKILL escalation) instead of orphaning them.
    var inFlightCount: Int { get }
}

public enum FleetCLIError: Error, LocalizedError, Equatable {
    /// The typed unavailable state: no trusted binary could be located.
    /// Renders as an honest banner, never as fake rows.
    case binaryUnavailable(String)

    public var errorDescription: String? {
        switch self {
        case .binaryUnavailable(let reason): return reason
        }
    }
}

/// Production `FleetCLIRunning`: locates the vendored binary fail-closed via
/// BinaryLocator, runs each invocation as a tracked `CLIRunner` child, and
/// can SIGTERM the whole set at quit.
public final class DeadreckonCLIClient: FleetCLIRunning {
    private let workingDirectory: String
    private let environment: [String: String]
    private let lock = NSLock()
    private var inFlight: [ObjectIdentifier: CLIRunner] = [:]

    public init(workingDirectory: String = NSHomeDirectory(), environment: [String: String] = [:]) {
        self.workingDirectory = workingDirectory
        self.environment = environment
    }

    public func run(arguments: [String], timeout: TimeInterval) async throws -> CLIRunResult {
        let binary: URL
        do {
            binary = try BinaryLocator.locate()
        } catch {
            let locatorError = error as? BinaryLocatorError
            throw FleetCLIError.binaryUnavailable(
                locatorError?.errorDescription ?? String(describing: error))
        }

        let runner = CLIRunner(
            binary: binary, arguments: arguments,
            workingDirectory: workingDirectory, environment: environment)
        try runner.launch()

        lock.lock()
        inFlight[ObjectIdentifier(runner)] = runner
        lock.unlock()
        defer {
            lock.lock()
            inFlight[ObjectIdentifier(runner)] = nil
            lock.unlock()
        }

        // The watchdog turns a hung child into a terminated one; the result
        // then reports whatever exit the child produced. SIGTERM first so
        // the CLI can clean up, SIGKILL only after 2 more seconds.
        let watchdog = Task.detached {
            try? await Task.sleep(nanoseconds: UInt64(timeout * 1_000_000_000))
            guard !Task.isCancelled else { return }
            runner.terminate(patience: 2)
        }
        defer { watchdog.cancel() }

        var stdout: [String] = []
        var stderr: [String] = []
        var exitCode: Int32 = -1
        for await event in runner.events {
            switch event {
            case .stdoutLine(let line): stdout.append(line)
            case .stderrLine(let line): stderr.append(line)
            case .terminated(let code): exitCode = code
            }
        }
        return CLIRunResult(
            stdout: stdout.joined(separator: "\n"),
            stderr: stderr.joined(separator: "\n"),
            exitCode: exitCode)
    }

    @discardableResult
    public func terminateInFlight(patience: TimeInterval) -> Int {
        lock.lock()
        let runners = Array(inFlight.values)
        lock.unlock()
        var signaled = 0
        for runner in runners where runner.isRunning {
            runner.terminate(patience: patience)
            signaled += 1
        }
        return signaled
    }

    public var inFlightCount: Int {
        lock.lock()
        let runners = Array(inFlight.values)
        lock.unlock()
        return runners.filter(\.isRunning).count
    }
}
