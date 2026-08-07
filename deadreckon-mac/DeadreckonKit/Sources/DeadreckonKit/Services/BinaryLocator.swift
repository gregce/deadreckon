import CryptoKit
import Foundation

/// Live getenv wrapper. ProcessInfo caches the environment at first access,
/// which breaks setenv-based tests and dev overrides applied late.
enum ProcessEnvironment {
    static func liveValue(_ name: String) -> String? {
        guard let raw = getenv(name) else { return nil }
        let value = String(cString: raw)
        return value.isEmpty ? nil : value
    }
}

public enum BinaryLocatorError: Error, LocalizedError, Equatable {
    case overrideNotAbsolute(String)
    case overrideNotExecutable(String)
    case unsupportedArchitecture(String)
    case binaryNotBundled(String)
    case manifestMissing
    case manifestUnreadable(String)
    case checksumMismatch(binary: String, expected: String, actual: String)

    public var errorDescription: String? {
        switch self {
        case .overrideNotAbsolute(let path):
            return "DEADRECKON_BIN must be an absolute path, got \"\(path)\"."
        case .overrideNotExecutable(let path):
            return "DEADRECKON_BIN points at \"\(path)\", which is not an executable file."
        case .unsupportedArchitecture(let machine):
            return "This Mac reports an unsupported architecture (\(machine)). deadreckon ships arm64 and x86_64 binaries only."
        case .binaryNotBundled(let name):
            return "The bundled CLI binary \(name) is missing from Resources/bin. Run scripts/vendor-cli.sh and rebuild."
        case .manifestMissing:
            return "Resources/bin/manifest.json is missing from the app bundle, so the CLI binary cannot be verified."
        case .manifestUnreadable(let reason):
            return "Resources/bin/manifest.json could not be read: \(reason)"
        case .checksumMismatch(let binary, let expected, let actual):
            return "Integrity check failed for \(binary): manifest.json expects sha256 \(expected) but the binary hashes to \(actual). Reinstall deadreckon or rerun scripts/vendor-cli.sh."
        }
    }
}

/// Locates the vendored `deadreckon` binary, fail closed.
///
/// Order: DEADRECKON_BIN env override (dev/tests, skips manifest
/// verification because a locally built binary will not match the pinned
/// hashes), then Bundle.main Resources/bin/deadreckon_darwin_{arm64|x86_64}
/// by machine architecture, verified against the committed
/// Resources/bin/manifest.json sha256. Verification failure throws; there is
/// no unverified fallback.
public enum BinaryLocator {
    private static let lock = NSLock()
    // Bundle binaries are verified once per process; re-hashing a large
    // binary on every child spawn is wasted work.
    private static var verifiedBundleBinary: URL?

    public static func locate() throws -> URL {
        if let override = ProcessEnvironment.liveValue("DEADRECKON_BIN") {
            guard override.hasPrefix("/") else {
                throw BinaryLocatorError.overrideNotAbsolute(override)
            }
            guard FileManager.default.isExecutableFile(atPath: override) else {
                throw BinaryLocatorError.overrideNotExecutable(override)
            }
            return URL(fileURLWithPath: override)
        }

        lock.lock()
        defer { lock.unlock() }
        if let verifiedBundleBinary {
            return verifiedBundleBinary
        }

        let archKey = try machineArchKey()
        let name = "deadreckon_darwin_\(archKey)"
        guard let binary = Bundle.main.url(forResource: name, withExtension: nil, subdirectory: "bin") else {
            throw BinaryLocatorError.binaryNotBundled(name)
        }
        guard let manifest = Bundle.main.url(forResource: "manifest", withExtension: "json", subdirectory: "bin") else {
            throw BinaryLocatorError.manifestMissing
        }
        try verify(binary: binary, manifestURL: manifest, archKey: archKey)
        verifiedBundleBinary = binary
        return binary
    }

    struct Manifest: Decodable {
        let sha256: [String: String]
    }

    static func verify(binary: URL, manifestURL: URL, archKey: String) throws {
        let manifest: Manifest
        do {
            let data = try Data(contentsOf: manifestURL)
            manifest = try JSONDecoder().decode(Manifest.self, from: data)
        } catch {
            throw BinaryLocatorError.manifestUnreadable(String(describing: error))
        }
        guard let expected = manifest.sha256[archKey]?.lowercased(), !expected.isEmpty else {
            throw BinaryLocatorError.manifestUnreadable("no sha256 entry for \(archKey)")
        }
        let actual = try sha256Hex(of: binary)
        guard actual == expected else {
            throw BinaryLocatorError.checksumMismatch(
                binary: binary.lastPathComponent, expected: expected, actual: actual)
        }
    }

    /// Hardware architecture of the running machine via utsname.
    static func machineArchKey() throws -> String {
        var uts = utsname()
        uname(&uts)
        let machine = withUnsafeBytes(of: &uts.machine) { raw -> String in
            String(decoding: raw.prefix(while: { $0 != 0 }), as: UTF8.self)
        }
        if machine.hasPrefix("arm64") { return "arm64" }
        if machine == "x86_64" { return "x86_64" }
        throw BinaryLocatorError.unsupportedArchitecture(machine)
    }

    static func sha256Hex(of url: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var hasher = SHA256()
        while true {
            guard let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty else { break }
            hasher.update(data: chunk)
        }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }
}
