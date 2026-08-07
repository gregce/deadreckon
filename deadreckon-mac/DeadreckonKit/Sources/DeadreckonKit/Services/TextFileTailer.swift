import Foundation

/// Plain-text tail for `supervisor.out` / `supervisor.err` (design P5
/// drawer terminals). These are NOT blessed JSONL ledgers: no schema, no
/// append-only promise, no corruption verdicts. The tailer reads appended
/// bytes; a file that shrinks (rotation, truncation) resets honestly to the
/// top rather than declaring anything. One owner polls it, like JSONLTailer.
public final class TextFileTailer {
    public enum PollResult: Equatable, Sendable {
        /// Nothing new (including: file does not exist yet).
        case none
        /// Newly appended text (may end mid-line; the drawer renders raw).
        case appended(String)
        /// The file shrank beneath the offset: whatever was rendered no
        /// longer reflects the file. The payload is the full fresh read.
        case reset(String)
    }

    public let url: URL
    public private(set) var offset: UInt64 = 0

    public init(url: URL) {
        self.url = url
    }

    public func poll() -> PollResult {
        guard let size = fileSize() else {
            if offset == 0 { return .none }
            offset = 0
            return .reset("")
        }
        if size < offset {
            offset = 0
            guard size > 0, let data = read(from: 0) else { return .reset("") }
            offset = UInt64(data.count)
            return .reset(String(decoding: data, as: UTF8.self))
        }
        guard size > offset, let data = read(from: offset) else { return .none }
        offset += UInt64(data.count)
        return .appended(String(decoding: data, as: UTF8.self))
    }

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
