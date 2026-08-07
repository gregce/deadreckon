import XCTest

@testable import DeadreckonKit

final class BinaryLocatorTests: XCTestCase {
    private var directory: URL!

    override func setUpWithError() throws {
        directory = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("binary-locator-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        unsetenv("DEADRECKON_BIN")
        try? FileManager.default.removeItem(at: directory)
    }

    private func writeExecutable(named name: String, contents: String) throws -> URL {
        let url = directory.appendingPathComponent(name)
        try Data(contents.utf8).write(to: url)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
        return url
    }

    // MARK: - DEADRECKON_BIN override

    func testOverrideWinsAndSkipsManifestVerification() throws {
        let binary = try writeExecutable(named: "deadreckon", contents: "#!/bin/sh\nexit 0\n")
        setenv("DEADRECKON_BIN", binary.path, 1)
        let located = try BinaryLocator.locate()
        XCTAssertEqual(located.path, binary.path)
    }

    func testRelativeOverrideIsRefused() {
        setenv("DEADRECKON_BIN", "relative/deadreckon", 1)
        XCTAssertThrowsError(try BinaryLocator.locate()) { error in
            XCTAssertEqual(
                error as? BinaryLocatorError, .overrideNotAbsolute("relative/deadreckon"))
        }
    }

    func testNonExecutableOverrideIsRefused() throws {
        let url = directory.appendingPathComponent("not-executable")
        try Data("data".utf8).write(to: url)
        setenv("DEADRECKON_BIN", url.path, 1)
        XCTAssertThrowsError(try BinaryLocator.locate()) { error in
            XCTAssertEqual(error as? BinaryLocatorError, .overrideNotExecutable(url.path))
        }
    }

    // MARK: - Manifest verification (fail closed)

    func testVerifyAcceptsMatchingSha256() throws {
        let binary = try writeExecutable(named: "deadreckon_darwin_arm64", contents: "binary-bytes")
        let expected = try BinaryLocator.sha256Hex(of: binary)
        let manifest = directory.appendingPathComponent("manifest.json")
        try Data(
            """
            {"cliVersion": "v0.8.4", "gitCommit": "21696a1", "sha256": {"arm64": "\(expected)"}}
            """.utf8
        ).write(to: manifest)
        XCTAssertNoThrow(
            try BinaryLocator.verify(binary: binary, manifestURL: manifest, archKey: "arm64"))
    }

    func testVerifyRejectsMismatchedSha256() throws {
        let binary = try writeExecutable(named: "deadreckon_darwin_arm64", contents: "binary-bytes")
        let manifest = directory.appendingPathComponent("manifest.json")
        try Data(
            #"{"cliVersion": "v0.8.4", "gitCommit": "21696a1", "sha256": {"arm64": "deadbeef"}}"#
                .utf8
        ).write(to: manifest)
        XCTAssertThrowsError(
            try BinaryLocator.verify(binary: binary, manifestURL: manifest, archKey: "arm64")
        ) { error in
            guard case .checksumMismatch = error as? BinaryLocatorError else {
                return XCTFail("expected checksumMismatch, got \(error)")
            }
        }
    }

    func testVerifyRejectsMissingArchEntry() throws {
        // The committed placeholder manifest has an empty sha256 map; a build
        // without vendored binaries must fail closed, never run unverified.
        let binary = try writeExecutable(named: "deadreckon_darwin_arm64", contents: "binary-bytes")
        let manifest = directory.appendingPathComponent("manifest.json")
        try Data(
            #"{"cliVersion": "unvendored", "gitCommit": "unvendored", "sha256": {}}"#.utf8
        ).write(to: manifest)
        XCTAssertThrowsError(
            try BinaryLocator.verify(binary: binary, manifestURL: manifest, archKey: "arm64"))
    }
}
