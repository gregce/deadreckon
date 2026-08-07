import CoreServices
import Foundation

/// DEADRECKON_HOME resolution, mirroring `DeadreckonPaths::discover` in the
/// binary: a non-empty DEADRECKON_HOME env value wins, else `~/.deadreckon`.
/// Read-only path math; nothing here creates or writes anything (trust
/// rule 8), and nothing here ever touches a path containing `gate-keys`
/// (trust rule 3).
public enum DeadreckonHome {
    public static func url() -> URL {
        if let override = ProcessEnvironment.liveValue("DEADRECKON_HOME") {
            return URL(fileURLWithPath: override)
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".deadreckon")
    }

    public static func jobsDirectory() -> URL {
        url().appendingPathComponent("jobs")
    }
}

/// The filesystem wake-up seam FleetStore listens through. FSEvents is a
/// hint only (TAILING.md): a change callback triggers one immediate poll of
/// `list --json`; the poll cadence continues regardless.
public protocol FleetWatching: AnyObject {
    func start(onChange: @escaping () -> Void)
    func stop()
    /// True while a change stream is live. False after a `start` that was a
    /// no-op (the watched directory does not exist yet), so the owner knows
    /// to retry `start` later instead of assuming hints will arrive.
    var isActive: Bool { get }
}

/// FSEventStream over DEADRECKON_HOME/jobs. Watching only the jobs subtree
/// keeps `gate-keys/` (a sibling under home) outside the watched path by
/// construction. If the directory does not exist yet the watcher stays
/// inert (`isActive == false`); polling still covers the fleet, and
/// FleetStore retries `start` after successful polls until the directory
/// appears.
///
/// Thread discipline: `stream` and `onChange` are only ever touched on the
/// private serial `queue`, which is also the FSEvents delivery queue. That
/// makes the callback's read of `onChange` ordered against `start`/`stop`
/// mutation, and guarantees no callback can run after `stop` returns
/// (`deinit` tears down on the same queue, closing the use-after-free
/// window the unretained stream context would otherwise leave open).
public final class JobsDirectoryWatcher: FleetWatching {
    private let directory: URL
    private let latency: CFTimeInterval
    private var stream: FSEventStreamRef?
    private var onChange: (() -> Void)?
    private let queue = DispatchQueue(label: "com.itavero.deadreckon.jobs-watcher")

    public init(directory: URL = DeadreckonHome.jobsDirectory(), latency: CFTimeInterval = 0.5) {
        self.directory = directory
        self.latency = latency
    }

    deinit {
        queue.sync { teardownLocked() }
    }

    public var isActive: Bool {
        queue.sync { stream != nil }
    }

    public func start(onChange: @escaping () -> Void) {
        queue.sync {
            teardownLocked()
            guard FileManager.default.fileExists(atPath: directory.path) else { return }
            self.onChange = onChange

            var context = FSEventStreamContext(
                version: 0,
                info: Unmanaged.passUnretained(self).toOpaque(),
                retain: nil, release: nil, copyDescription: nil)

            let callback: FSEventStreamCallback = { _, info, _, _, _, _ in
                // Runs on `queue` (FSEventStreamSetDispatchQueue below), so
                // this read of onChange is serialized against start/stop.
                guard let info else { return }
                let watcher = Unmanaged<JobsDirectoryWatcher>.fromOpaque(info).takeUnretainedValue()
                watcher.onChange?()
            }

            guard let stream = FSEventStreamCreate(
                kCFAllocatorDefault,
                callback,
                &context,
                [directory.path] as CFArray,
                FSEventStreamEventId(kFSEventStreamEventIdSinceNow),
                latency,
                FSEventStreamCreateFlags(kFSEventStreamCreateFlagNone))
            else { return }

            self.stream = stream
            FSEventStreamSetDispatchQueue(stream, queue)
            FSEventStreamStart(stream)
        }
    }

    public func stop() {
        queue.sync { teardownLocked() }
    }

    /// Must run on `queue`. Invalidating on the delivery queue means no
    /// callback can be in flight when this returns.
    private func teardownLocked() {
        guard let stream else {
            onChange = nil
            return
        }
        FSEventStreamStop(stream)
        FSEventStreamInvalidate(stream)
        FSEventStreamRelease(stream)
        self.stream = nil
        onChange = nil
    }
}
