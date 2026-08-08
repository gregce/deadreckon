import Foundation

public enum CLIRunnerEvent: Sendable, Equatable {
    case stdoutLine(String)
    case stderrLine(String)
    case terminated(exitCode: Int32)
}

public enum CLIRunnerError: Error, LocalizedError {
    case alreadyLaunched
    case launchFailed(String)
    case timeout(TimeInterval)

    public var errorDescription: String? {
        switch self {
        case .alreadyLaunched:
            return "This CLI process was already launched."
        case .launchFailed(let reason):
            return "Could not launch the deadreckon CLI: \(reason)"
        case .timeout(let seconds):
            return "The deadreckon CLI did not finish within \(Int(seconds)) seconds and was killed."
        }
    }
}

/// One-shot result for callers that need the exit code (deadreckon uses exit
/// codes 0/1/2/130 as machine signal; a nonzero exit is an outcome to render,
/// not a launch error).
public struct CLIRunResult: Sendable {
    public let stdout: String
    public let stderr: String
    public let exitCode: Int32
}

/// Splits pipe chunks into complete lines, holding the trailing partial line
/// until more data (or EOF flush) arrives.
struct CLILineBuffer {
    private var pending = Data()

    mutating func append(_ chunk: Data) -> [String] {
        pending.append(chunk)
        var lines: [String] = []
        while let newlineIndex = pending.firstIndex(of: 0x0A) {
            let lineData = pending.subdata(in: pending.startIndex..<newlineIndex)
            pending.removeSubrange(pending.startIndex...newlineIndex)
            var line = String(decoding: lineData, as: UTF8.self)
            if line.hasSuffix("\r") { line.removeLast() }
            lines.append(line)
        }
        return lines
    }

    mutating func flush() -> String? {
        guard !pending.isEmpty else { return nil }
        let line = String(decoding: pending, as: UTF8.self)
        pending.removeAll()
        return line
    }
}

/// One child `deadreckon` process per invocation, never a pool. stdout and
/// stderr are parsed line-wise into one AsyncStream; NDJSON callers decode
/// each `.stdoutLine`. The stream ends with exactly one `.terminated` event.
/// The app never writes harness files itself: every mutation is one of these
/// invocations against the manifest-pinned binary.
public final class CLIRunner {
    private let process = Process()
    private let stdoutPipe = Pipe()
    private let stderrPipe = Pipe()
    /// Payload written to the child's stdin after launch (nil keeps the
    /// null device). REDACTION RULE (SETTINGS spec rule 4): these bytes are
    /// the secret path for `config set-key` — they are written to the child
    /// and released, and must never be echoed into any transcript, log,
    /// error description, or display string. No code in this file reads
    /// them back for any purpose but the one write.
    private let stdinPayload: Data?
    private let queue = DispatchQueue(label: "com.itavero.deadreckon.cli-runner")

    private var stdoutBuffer = CLILineBuffer()
    private var stderrBuffer = CLILineBuffer()
    private var continuation: AsyncStream<CLIRunnerEvent>.Continuation?
    private var launched = false
    private var stdoutClosed = false
    private var stderrClosed = false
    private var exitStatus: Int32?
    private var finished = false
    // Keeps the runner alive while the child runs even if the caller drops
    // its reference; released when the event stream finishes.
    private var keepAliveWhileRunning: CLIRunner?

    public let events: AsyncStream<CLIRunnerEvent>

    public init(binary: URL, arguments: [String], workingDirectory: String,
                environment: [String: String], stdin: Data? = nil) {
        var streamContinuation: AsyncStream<CLIRunnerEvent>.Continuation?
        events = AsyncStream { streamContinuation = $0 }
        continuation = streamContinuation
        stdinPayload = stdin

        process.executableURL = binary
        process.arguments = arguments
        process.currentDirectoryURL = URL(fileURLWithPath: workingDirectory)
        process.environment = Self.mergedEnvironment(environment)
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe
        // nil stdin keeps the historical null device; a payload gets a pipe
        // written after launch (feedStdin).
        if stdin == nil {
            process.standardInput = FileHandle.nullDevice
        } else {
            process.standardInput = Pipe()
        }
    }

    /// Write the stdin payload to the launched child and close the pipe so
    /// the child sees EOF (`config set-key` reads to EOF). A child that
    /// exited before the write is tolerated: the write error is swallowed
    /// WITHOUT capturing the payload into any error path (redaction rule),
    /// and F_SETNOSIGPIPE keeps a closed pipe from signalling the app.
    private static func feedStdin(_ payload: Data, into pipe: Pipe) {
        let handle = pipe.fileHandleForWriting
        _ = fcntl(handle.fileDescriptor, F_SETNOSIGPIPE, 1)
        DispatchQueue.global(qos: .userInitiated).async {
            defer { try? handle.close() }
            do {
                try handle.write(contentsOf: payload)
            } catch {
                // Deliberately silent: the only context this error could
                // carry is the secret payload's destination; the child's
                // own exit/refusal is the observable outcome.
            }
        }
    }

    /// `launched` is confined to `queue` (written in launch, read here and
    /// in terminate) so nothing depends on unsynchronized cross-thread
    /// visibility once the project moves to strict concurrency.
    public var isRunning: Bool {
        queue.sync { launched } && process.isRunning
    }

    public func launch() throws {
        try queue.sync {
            guard !launched else { throw CLIRunnerError.alreadyLaunched }
            stdoutPipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
                let data = handle.availableData
                self?.queue.async { self?.consume(data, stderr: false) }
            }
            stderrPipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
                let data = handle.availableData
                self?.queue.async { self?.consume(data, stderr: true) }
            }
            process.terminationHandler = { [weak self] process in
                let status = process.terminationStatus
                self?.queue.async { self?.processExited(status) }
            }
            do {
                try process.run()
            } catch {
                stdoutPipe.fileHandleForReading.readabilityHandler = nil
                stderrPipe.fileHandleForReading.readabilityHandler = nil
                process.terminationHandler = nil
                throw CLIRunnerError.launchFailed(error.localizedDescription)
            }
            launched = true
            keepAliveWhileRunning = self
            if let payload = stdinPayload, let pipe = process.standardInput as? Pipe {
                Self.feedStdin(payload, into: pipe)
            }
        }
    }

    /// SIGTERM first so the CLI can run its own graceful teardown; escalate
    /// to SIGKILL only after `patience`. The SIGTERM is delivered
    /// synchronously so a caller that exits right after (app quit) still gets
    /// the signal out; only the SIGKILL escalation is deferred.
    public func terminate(patience: TimeInterval = 5) {
        queue.sync { [self] in
            guard launched, process.isRunning else { return }
            let pid = process.processIdentifier
            process.terminate()
            queue.asyncAfter(deadline: .now() + patience) { [weak self] in
                guard let self, self.process.isRunning else { return }
                kill(pid, SIGKILL)
            }
        }
    }

    // MARK: - One-shot

    /// Run to completion and return stdout (JSON modes). Kills the child and
    /// throws on timeout.
    public static func run(binary: URL, arguments: [String], workingDirectory: String, environment: [String: String], timeout: TimeInterval, stdin: Data? = nil) async throws -> String {
        try await runDetailed(binary: binary, arguments: arguments, workingDirectory: workingDirectory, environment: environment, timeout: timeout, stdin: stdin).stdout
    }

    public static func runDetailed(binary: URL, arguments: [String], workingDirectory: String, environment: [String: String], timeout: TimeInterval, stdin: Data? = nil) async throws -> CLIRunResult {
        try await withCheckedThrowingContinuation { continuation in
            let process = Process()
            let stdoutPipe = Pipe()
            let stderrPipe = Pipe()
            process.executableURL = binary
            process.arguments = arguments
            process.currentDirectoryURL = URL(fileURLWithPath: workingDirectory)
            process.environment = mergedEnvironment(environment)
            process.standardOutput = stdoutPipe
            process.standardError = stderrPipe
            let stdinPipe: Pipe?
            if stdin == nil {
                stdinPipe = nil
                process.standardInput = FileHandle.nullDevice
            } else {
                let pipe = Pipe()
                stdinPipe = pipe
                process.standardInput = pipe
            }

            let lock = NSLock()
            var resumed = false
            func resumeOnce(_ result: Result<CLIRunResult, Error>) {
                lock.lock()
                let shouldResume = !resumed
                resumed = true
                lock.unlock()
                if shouldResume { continuation.resume(with: result) }
            }

            var stdoutData = Data()
            var stderrData = Data()
            let group = DispatchGroup()
            group.enter()
            group.enter()
            group.enter()
            DispatchQueue.global().async {
                stdoutData = (try? stdoutPipe.fileHandleForReading.readToEnd()) ?? Data()
                group.leave()
            }
            DispatchQueue.global().async {
                stderrData = (try? stderrPipe.fileHandleForReading.readToEnd()) ?? Data()
                group.leave()
            }
            process.terminationHandler = { _ in group.leave() }

            do {
                try process.run()
            } catch {
                process.terminationHandler = nil
                // Unblock the reader tasks; the child never existed.
                try? stdoutPipe.fileHandleForWriting.close()
                try? stderrPipe.fileHandleForWriting.close()
                resumeOnce(.failure(CLIRunnerError.launchFailed(error.localizedDescription)))
                return
            }
            if let payload = stdin, let pipe = stdinPipe {
                feedStdin(payload, into: pipe)
            }

            let pid = process.processIdentifier
            var timedOut = false
            let killOnTimeout = DispatchWorkItem {
                lock.lock()
                timedOut = true
                lock.unlock()
                if process.isRunning { kill(pid, SIGKILL) }
                // Report the timeout shortly even if an orphaned grandchild
                // still holds the pipes open past the kill, and close our
                // read ends so the two reader threads unblock instead of
                // leaking from the global pool on every timeout.
                DispatchQueue.global().asyncAfter(deadline: .now() + 0.2) {
                    try? stdoutPipe.fileHandleForReading.close()
                    try? stderrPipe.fileHandleForReading.close()
                    resumeOnce(.failure(CLIRunnerError.timeout(timeout)))
                }
            }
            DispatchQueue.global().asyncAfter(deadline: .now() + timeout, execute: killOnTimeout)

            group.notify(queue: DispatchQueue.global()) {
                killOnTimeout.cancel()
                lock.lock()
                let sawTimeout = timedOut
                lock.unlock()
                if sawTimeout {
                    resumeOnce(.failure(CLIRunnerError.timeout(timeout)))
                    return
                }
                let stdout = String(decoding: stdoutData, as: UTF8.self)
                let stderr = String(decoding: stderrData, as: UTF8.self)
                resumeOnce(.success(CLIRunResult(stdout: stdout, stderr: stderr, exitCode: process.terminationStatus)))
            }
        }
    }

    // MARK: - Private

    static func mergedEnvironment(_ overrides: [String: String]) -> [String: String] {
        ProcessInfo.processInfo.environment.merging(overrides) { _, override in override }
    }

    private func consume(_ data: Data, stderr: Bool) {
        if data.isEmpty {
            if stderr {
                stderrClosed = true
                stderrPipe.fileHandleForReading.readabilityHandler = nil
            } else {
                stdoutClosed = true
                stdoutPipe.fileHandleForReading.readabilityHandler = nil
            }
            finishIfComplete()
            return
        }
        let lines = stderr ? stderrBuffer.append(data) : stdoutBuffer.append(data)
        for line in lines { emit(line, stderr: stderr) }
    }

    private func emit(_ line: String, stderr: Bool) {
        guard !finished, !line.isEmpty else { return }
        continuation?.yield(stderr ? .stderrLine(line) : .stdoutLine(line))
    }

    private func processExited(_ status: Int32) {
        exitStatus = status
        // If an orphaned grandchild inherited our pipes, EOF may never come;
        // force-finish shortly after the child itself is gone.
        queue.asyncAfter(deadline: .now() + 1.0) { [weak self] in
            self?.forceFinish()
        }
        finishIfComplete()
    }

    private func finishIfComplete() {
        guard !finished, let status = exitStatus, stdoutClosed, stderrClosed else { return }
        finishStream(status)
    }

    private func forceFinish() {
        guard !finished, let status = exitStatus else { return }
        stdoutClosed = true
        stderrClosed = true
        stdoutPipe.fileHandleForReading.readabilityHandler = nil
        stderrPipe.fileHandleForReading.readabilityHandler = nil
        finishStream(status)
    }

    private func finishStream(_ status: Int32) {
        if let line = stdoutBuffer.flush() { emit(line, stderr: false) }
        if let line = stderrBuffer.flush() { emit(line, stderr: true) }
        finished = true
        continuation?.yield(.terminated(exitCode: status))
        continuation?.finish()
        continuation = nil
        keepAliveWhileRunning = nil
    }
}
