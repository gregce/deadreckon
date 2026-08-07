import Foundation

/// The docs/TAILING.md reader algorithm, exactly.
///
/// Keeps a byte offset per file; on every `poll()`:
/// 1. stat the file. Missing: nothing yet (offset 0) or, for
///    acceptance-progress only, a restart. Length below the offset: restart
///    for acceptance-progress, corruption for everything else.
/// 2. Open, seek to the offset, read to EOF, advance the offset by the bytes
///    read.
/// 3. Prepend bytes retained from the previous poll, split at the LAST
///    newline. Everything before it is complete lines; everything after it is
///    a torn append, retained unparsed for the next poll.
/// 4. Parse each complete line as one JSON object. A parse failure is
///    corruption (report and stop trusting the file), except for
///    acceptance-progress where ANY parse anomaly means the sign-time rewrite
///    landed under the offset: reset to 0, discard retained rows, re-read.
/// 5. For job-events, verify `sequence` continues exactly last + 1. Any gap
///    is corruption; render "unknown", never a guessed state.
///
/// Corruption is sticky: once a strict file is corrupt, every later poll
/// reports the same corruption. Not thread-safe; one owner polls it.
/// Polling with a short interval is the supported mechanism; FSEvents is a
/// wake-up hint only, the read path above must still run as written.
public final class JSONLTailer {
    public enum Mode: Equatable, Sendable {
        /// Append-only blessed files: events.jsonl, spend.jsonl, traces.jsonl,
        /// flight-events.jsonl, notify.jsonl.
        case standard
        /// job-events.jsonl: standard plus strict `sequence` 1..N validation.
        case jobEvents
        /// proofs/acceptance-progress.jsonl: restarts on each gate attempt
        /// and is rewritten once at sign time; any anomaly means restart,
        /// never corruption.
        case acceptanceProgress
    }

    public enum PollResult: Equatable, Sendable {
        /// Nothing new (including: file does not exist yet).
        case none
        /// Newly completed lines, in file order, each one valid JSON.
        case lines([String])
        /// acceptance-progress only: the file restarted (new gate attempt or
        /// sign-time rewrite). Discard every previously retained row; the
        /// payload is the full re-read from the top (possibly empty while the
        /// rewrite is still in flight).
        case restarted([String])
        /// Strict files only. Report it and stop trusting the file; sticky.
        case corrupt(String)
    }

    public let url: URL
    public let mode: Mode
    public private(set) var offset: UInt64 = 0
    /// jobEvents mode: the last verified sequence number (0 before any row).
    public private(set) var lastSequence: UInt64 = 0

    private var retained = Data()
    private var corruptReason: String?

    /// True while the last poll left an unterminated final line retained
    /// (TAILING.md torn append: an in-flight append or crash residue being
    /// retried, never parsed and never corruption). Surfaces as the drawer's
    /// torn-tail badge.
    public var hasRetainedTail: Bool { !retained.isEmpty }

    public init(url: URL, mode: Mode) {
        self.url = url
        self.mode = mode
    }

    public func poll() -> PollResult {
        if let corruptReason {
            return .corrupt(corruptReason)
        }

        guard let size = fileSize() else {
            // Missing file: nothing yet, keep offset 0 and retry later. A
            // file that vanishes beneath an advanced offset is an anomaly:
            // restart for acceptance-progress, corruption for append-only
            // files (they are never rotated, renamed, or removed).
            if offset == 0 && retained.isEmpty {
                return .none
            }
            if mode == .acceptanceProgress {
                resetForRestart()
                return .restarted([])
            }
            return fail("file disappeared beneath a retained offset")
        }

        if size < offset {
            if mode == .acceptanceProgress {
                return restartAndReread()
            }
            return fail("file shrank below the read offset; append-only violated")
        }

        guard size > offset, let chunk = read(from: offset) else {
            return .none
        }
        offset += UInt64(chunk.count)

        var buffer = retained
        buffer.append(chunk)
        let (completeLines, tail) = Self.splitAtLastNewline(buffer)
        retained = tail

        switch parse(completeLines) {
        case .success(let lines):
            return lines.isEmpty ? .none : .lines(lines)
        case .failure(let error):
            if mode == .acceptanceProgress {
                return restartAndReread()
            }
            return fail(error.reason)
        }
    }

    // MARK: - Restart (acceptance-progress only)

    /// The sign-time rewrite reuses the same inode and may leave the file the
    /// same length or longer than the offset, landing the next read mid-line
    /// inside new content. Reset to 0, discard retained rows, re-read from
    /// the top. If the fresh read itself fails to parse (rewrite still in
    /// flight), stay reset so the next poll retries from the top.
    private func restartAndReread() -> PollResult {
        resetForRestart()
        guard let size = fileSize(), size > 0, let chunk = read(from: 0) else {
            return .restarted([])
        }
        offset = UInt64(chunk.count)
        let (completeLines, tail) = Self.splitAtLastNewline(chunk)
        retained = tail
        switch parse(completeLines) {
        case .success(let lines):
            return .restarted(lines)
        case .failure:
            resetForRestart()
            return .restarted([])
        }
    }

    private func resetForRestart() {
        offset = 0
        retained.removeAll()
        // Sequence tracking does not apply to acceptance-progress, but keep
        // the reset total so a hypothetical mode change cannot leak state.
        lastSequence = 0
    }

    private func fail(_ reason: String) -> PollResult {
        corruptReason = reason
        return .corrupt(reason)
    }

    // MARK: - Parsing

    struct TailError: Error {
        let reason: String
    }

    private func parse(_ lineData: [Data]) -> Swift.Result<[String], TailError> {
        var lines: [String] = []
        for data in lineData {
            let text = String(decoding: data, as: UTF8.self)
            // Writers never emit blank lines; tolerate a stray trailing one
            // rather than calling whitespace corruption. This is a deliberate
            // softening of TAILING.md's literal "any parse failure is
            // corruption" wording, matching deadreckon's own readers: the
            // Rust `spend_rollup` (crates/deadreckon/src/commands/
            // inspection.rs) filters `!line.trim().is_empty()` before parsing,
            // so a blank line can never diverge from the harness in practice.
            if text.trimmingCharacters(in: .whitespaces).isEmpty { continue }
            let object: Any
            do {
                object = try JSONSerialization.jsonObject(with: data)
            } catch {
                return .failure(TailError(reason: "complete line is not valid JSON: \(text.prefix(120))"))
            }
            guard let dictionary = object as? [String: Any] else {
                return .failure(TailError(reason: "complete line is not a JSON object: \(text.prefix(120))"))
            }
            if mode == .jobEvents {
                guard let sequence = (dictionary["sequence"] as? NSNumber)?.uint64Value else {
                    return .failure(TailError(reason: "job event row is missing its sequence field"))
                }
                guard sequence == lastSequence + 1 else {
                    return .failure(TailError(
                        reason: "job event sequence gap: expected \(lastSequence + 1), read \(sequence)"))
                }
                lastSequence = sequence
            }
            lines.append(text)
        }
        return .success(lines)
    }

    /// Split at the LAST newline: everything before it is complete lines,
    /// everything after it is a torn append to retain unparsed.
    static func splitAtLastNewline(_ buffer: Data) -> (completeLines: [Data], tail: Data) {
        guard let lastNewline = buffer.lastIndex(of: 0x0A) else {
            return ([], buffer)
        }
        let complete = buffer[buffer.startIndex..<lastNewline]
        let tail = Data(buffer[buffer.index(after: lastNewline)...])
        let lines = complete
            .split(separator: 0x0A, omittingEmptySubsequences: false)
            .map { Data($0) }
        return (lines, tail)
    }

    // MARK: - File IO

    private func fileSize() -> UInt64? {
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path),
            let size = attributes[.size] as? NSNumber
        else {
            return nil
        }
        return size.uint64Value
    }

    private func read(from start: UInt64) -> Data? {
        guard let handle = try? FileHandle(forReadingFrom: url) else { return nil }
        defer { try? handle.close() }
        do {
            try handle.seek(toOffset: start)
            return try handle.readToEnd() ?? Data()
        } catch {
            return nil
        }
    }
}
