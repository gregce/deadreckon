import Foundation

@testable import DeadreckonKit

/// Prefix-scripted CLI fake for the settings stores. Unlike FakeCLI (keyed
/// on the first token), scripts here are keyed on a joined argv PREFIX
/// ("config show", "config set", "supervisor status", …) with the longest
/// match winning — the write-then-re-read flows need "config set" and
/// "config show" to answer differently inside one test. Each prefix holds a
/// FIFO queue so a re-read can be scripted to answer differently across
/// polls. Records every call's argv and stdin payload.
final class SettingsFakeCLI: FleetCLIRunning, @unchecked Sendable {
    private let lock = NSLock()
    private var queues: [String: [Result<CLIRunResult, Error>]] = [:]
    /// The last answer served per prefix: repeated re-reads (the store's
    /// write-then-re-read cycle polls `config show` after every write) keep
    /// getting the latest scripted answer once the queue drains.
    private var lastServed: [String: Result<CLIRunResult, Error>] = [:]
    private(set) var calls: [[String]] = []
    private(set) var stdins: [Data?] = []

    /// Queue one scripted answer for the given argv prefix.
    func script(_ prefix: String, stdout: String, stderr: String = "", exitCode: Int32 = 0) {
        lock.lock()
        queues[prefix, default: []].append(
            .success(CLIRunResult(stdout: stdout, stderr: stderr, exitCode: exitCode)))
        lock.unlock()
    }

    func scriptFailure(_ prefix: String, _ error: Error) {
        lock.lock()
        queues[prefix, default: []].append(.failure(error))
        lock.unlock()
    }

    func calls(withPrefix prefix: String) -> [[String]] {
        lock.lock()
        defer { lock.unlock() }
        return calls.filter { $0.joined(separator: " ").hasPrefix(prefix) }
    }

    /// The stdin payload recorded for the first call matching the prefix.
    func stdin(forCallWithPrefix prefix: String) -> Data? {
        lock.lock()
        defer { lock.unlock() }
        for (index, argv) in calls.enumerated()
        where argv.joined(separator: " ").hasPrefix(prefix) {
            return stdins[index]
        }
        return nil
    }

    func run(arguments: [String], timeout: TimeInterval, stdin: Data?) async throws -> CLIRunResult {
        lock.lock()
        calls.append(arguments)
        stdins.append(stdin)
        let joined = arguments.joined(separator: " ")
        let key = queues.keys
            .filter { joined.hasPrefix($0) }
            .max(by: { $0.count < $1.count })
        var scripted: Result<CLIRunResult, Error>?
        if let key {
            if var queue = queues[key], !queue.isEmpty {
                scripted = queue.removeFirst()
                queues[key] = queue
                lastServed[key] = scripted
            } else {
                scripted = lastServed[key]
            }
        }
        lock.unlock()
        switch scripted {
        case .success(let result): return result
        case .failure(let error): throw error
        case nil:
            return CLIRunResult(stdout: "", stderr: "unscripted: \(joined)", exitCode: 2)
        }
    }

    @discardableResult
    func terminateInFlight(patience: TimeInterval) -> Int { 0 }
    var inFlightCount: Int { 0 }
}

// MARK: - Fixtures from the SHIPPED Rust shapes

/// Envelope fixtures composed byte-shape-faithful to the landed Rust
/// emitters (main.rs config_*_command, supervisor_service.rs
/// emit_supervisor_lifecycle_success / build_service_status_report,
/// doctor.rs doctor_json_payload). The doctor fixture's binary_health block
/// mirrors a live 0.8.4 probe (2026-08-07, scratch home).
enum SettingsFixtures {
    static let configShow = """
    {
      "kind": "config", "id": "show", "status": "completed",
      "next_actions": ["deadreckon config provider", "deadreckon providers list", "deadreckon doctor"],
      "try_lines": [],
      "action": "show",
      "config_path": "/Users/op/.deadreckon/config.toml",
      "config_exists": true,
      "settings": {
        "defaults.provider": { "value": "cli:claude-code", "source": "set" },
        "defaults.model": { "value": null, "source": "default" },
        "defaults.max_spend": { "value": 15, "source": "set" },
        "defaults.cli_max_wall_seconds": { "value": 36000.0, "source": "default" },
        "defaults.sandbox": { "value": "auto", "source": "default" },
        "defaults.prevent_sleep": { "value": null, "source": "default" }
      },
      "providers": {
        "anthropic": { "kind": "api", "api_key": "configured", "model": "claude-sonnet-4-5" },
        "cli:claude-code": { "model": "opus" }
      },
      "fallback": [],
      "provider_overrides_dir": "/Users/op/.deadreckon/providers.d",
      "provider_override_files": [],
      "file": {
        "defaults": { "provider": "cli:claude-code", "max_spend": 15 },
        "providers": { "anthropic": { "api_key": "configured" } }
      },
      "primary_action": "deadreckon config provider",
      "verdict": {
        "kind": "completed",
        "label": "completed config show",
        "subject": "show",
        "recommended_command": "deadreckon config provider",
        "explanation": "DeadReckon read the effective configuration from /Users/op/.deadreckon/config.toml.",
        "evidence": [
          ["config", "/Users/op/.deadreckon/config.toml"],
          ["set keys", "defaults.provider, defaults.max_spend"],
          ["providers.anthropic", "api_key configured"]
        ]
      }
    }
    """

    /// The same document after `config set defaults.max_spend 25` landed.
    static let configShowAfterSet = configShow
        .replacingOccurrences(of: "\"value\": 15", with: "\"value\": 25")
        .replacingOccurrences(of: "\"max_spend\": 15", with: "\"max_spend\": 25")

    static let configSetCompleted = """
    {
      "kind": "config", "id": "defaults.max_spend", "status": "completed",
      "next_actions": ["deadreckon config show", "deadreckon doctor"],
      "try_lines": [],
      "action": "set",
      "key": "defaults.max_spend",
      "value": 25,
      "previous": 15,
      "config_path": "/Users/op/.deadreckon/config.toml",
      "primary_action": "deadreckon config show",
      "verdict": {
        "kind": "completed",
        "label": "completed config defaults.max_spend",
        "subject": "defaults.max_spend",
        "recommended_command": "deadreckon config show",
        "explanation": "Reading the same key back is the safest verification command.",
        "evidence": [["config", "/Users/op/.deadreckon/config.toml"]]
      }
    }
    """

    /// SHIPPED set-key acknowledgment: facts are exactly
    /// {provider, stored, keychain_or_file} — no key material, anywhere.
    static let configSetKeyCompleted = """
    {
      "kind": "config", "id": "anthropic", "status": "completed",
      "next_actions": ["deadreckon providers check anthropic", "deadreckon config show"],
      "try_lines": [],
      "action": "set-key",
      "provider": "anthropic",
      "stored": true,
      "keychain_or_file": "file",
      "config_path": "/Users/op/.deadreckon/config.toml",
      "primary_action": "deadreckon providers check anthropic",
      "verdict": {
        "kind": "completed",
        "label": "completed config set-key anthropic",
        "subject": "set-key anthropic",
        "recommended_command": "deadreckon providers check anthropic",
        "explanation": "The key never rode the command line and every surface reports it only as configured.",
        "evidence": [
          ["provider", "anthropic"],
          ["storage", "config file"],
          ["api_key", "configured"]
        ]
      }
    }
    """

    static let configSetRefusal = """
    {"kind": "error", "code": 1, "verb": "config",
     "message": "config key defaults.max_spend expects a number greater than zero; got `banana`",
     "try_lines": ["deadreckon config set defaults.max_spend <value>"]}
    """

    /// The exact clap prose an older binary (vendored 0.8.4) prints for
    /// `config show` — the capability probe's degraded case, live-observed.
    static let configShowOlderBinaryProse = """
    error: unrecognized subcommand 'show'

    Usage: deadreckon config <COMMAND>

    For more information, try '--help'.
    """

    static let serviceStatusHealthy = """
    {
      "schema_version": 4,
      "manager": "launchd",
      "installed": "current",
      "loaded": true,
      "enabled": "enabled",
      "active": null,
      "service": "running",
      "home_checkpoint": "present",
      "verdict": "healthy",
      "verdict_reason": null,
      "checkpoint": {
        "schema_version": 2,
        "generation": 12,
        "instance_id": "supervisor-4f1c",
        "boot_id": "macos:7A2C9D",
        "pid": 79697,
        "process_start_identity": "79697:5122",
        "started_at": "2026-08-07T01:00:00Z",
        "binary": "/Users/op/.local/share/deadreckon/bin/deadreckon",
        "deadreckon_home": "/Users/op/.deadreckon",
        "bundle_build_id": "deadreckon-bundle-build-id-sha256:c4ce",
        "binary_sha256": "sha256:6155c2"
      },
      "current_boot_id": "macos:7A2C9D",
      "boot_identity_source": "macos_sysctl",
      "test_override": false
    }
    """

    static func serviceStatus(service: String, homeCheckpoint: String, verdict: String,
                              installed: String, reason: String?) -> String {
        let reasonJSON = reason.map { "\"\($0)\"" } ?? "null"
        return """
        {
          "schema_version": 4,
          "manager": "launchd",
          "installed": "\(installed)",
          "loaded": \(service == "running" ? "true" : "false"),
          "enabled": "enabled",
          "active": null,
          "service": "\(service)",
          "home_checkpoint": "\(homeCheckpoint)",
          "verdict": "\(verdict)",
          "verdict_reason": \(reasonJSON),
          "checkpoint": null,
          "current_boot_id": "macos:7A2C9D",
          "boot_identity_source": "macos_sysctl",
          "test_override": false
        }
        """
    }

    /// The live-observed pre-v4 refusal (prose on stderr, exit 1).
    static let serviceStatusProseRefusal = """
    error: invalid input: service manager reports the supervisor running, but its instance checkpoint is absent
      hint: try: deadreckon doctor
    """

    static func supervisorLifecycle(action: String, result: String,
                                    serviceState: String) -> String {
        """
        {
          "kind": "supervisor", "id": "\(action)", "status": "completed",
          "next_actions": ["deadreckon supervisor status --json"],
          "try_lines": [],
          "action": "\(action)",
          "result": "\(result)",
          "service_state": "\(serviceState)",
          "unit_path": "/Users/op/Library/LaunchAgents/com.deadreckon.supervisor.plist",
          "deadreckon_home": "/Users/op/.deadreckon",
          "binary": "/Users/op/.local/share/deadreckon/bin/deadreckon",
          "primary_action": "deadreckon supervisor status --json",
          "verdict": {
            "kind": "completed",
            "label": "completed supervisor \(action)",
            "subject": "\(action)",
            "recommended_command": "deadreckon supervisor status --json",
            "explanation": "The service manager now owns the supervisor process.",
            "evidence": [
              ["unit", "/Users/op/Library/LaunchAgents/com.deadreckon.supervisor.plist"],
              ["service", "\(serviceState)"]
            ]
          }
        }
        """
    }

    static let supervisorUnmanagedRefusal = """
    {"kind": "error", "code": 1, "verb": "supervisor",
     "message": "an unmanaged unit occupies /Users/op/Library/LaunchAgents/com.deadreckon.supervisor.plist; DeadReckon will not overwrite a unit it does not manage",
     "try_lines": ["deadreckon supervisor status"]}
    """

    static func doctorReport(repairs: String? = nil,
                             repairableReceipt: Bool = false,
                             supervisorFindingStatus: String = "passed") -> String {
        let repairsRow = repairs.map { "\"repairs\": \($0)," } ?? ""
        return """
        {
          "kind": "doctor",
          "id": "/Users/op/project",
          "status": "blocked",
          "next_actions": ["deadreckon init"],
          "try_lines": [],
          \(repairsRow)
          "paths": { "home": "/Users/op/.deadreckon", "config": "/Users/op/.deadreckon/config.toml" },
          "source": "/Users/op/project",
          "home": "/Users/op/.deadreckon",
          "config_path": "/Users/op/.deadreckon/config.toml",
          "config_present": false,
          "sandboxes": [
            { "backend": "sandbox-exec", "available": true, "path": "/usr/bin/sandbox-exec", "note": "" },
            { "backend": "docker", "available": false, "path": null, "note": "docker CLI not found" },
            { "backend": "none", "available": true, "path": null, "note": "available but unsafe; use only when explicitly requested" }
          ],
          "seams": { "status": "ok", "resolution": ["no seams configured"] },
          "binary_health": {
            "current_path": "/Applications/deadreckon.app/Contents/Resources/bin/deadreckon_darwin_arm64",
            "current_version": "0.8.4",
            "path_selected": "/opt/homebrew/bin/deadreckon",
            "installations": [
              {
                "canonical_path": "/Users/op/.local/share/deadreckon/bin/deadreckon",
                "channel": "shell",
                "locations": ["/Users/op/.local/share/deadreckon/bin/deadreckon"],
                "roles": ["known-install-location", "path"],
                "sha256": "sha256:6155c20eb488",
                "update_command": "deadreckon update",
                "version": "0.8.4"
              },
              {
                "canonical_path": "/opt/homebrew/Cellar/deadreckon/0.8.1/bin/deadreckon",
                "channel": "brew",
                "locations": ["/opt/homebrew/bin/deadreckon"],
                "roles": ["known-install-location", "path-selected"],
                "sha256": "sha256:aa11bb22",
                "update_command": "brew upgrade deadreckon",
                "version": "0.8.1"
              }
            ],
            "conflicts": [
              "/opt/homebrew/Cellar/deadreckon/0.8.1/bin/deadreckon is version 0.8.1; the running binary is version 0.8.4",
              "PATH selects /opt/homebrew/bin/deadreckon, but this process is running /Applications/deadreckon.app/Contents/Resources/bin/deadreckon_darwin_arm64"
            ],
            "advisories": [],
            "gate_helper_compatible": true,
            "gate_helper_path": "/Applications/deadreckon.app/Contents/Resources/bin/dr-gate",
            "gate_protocol_version": 1,
            "bundle_build_id": "deadreckon-bundle-build-id-sha256:c4ce7186",
            "repairable_receipt": \(repairableReceipt),
            "repairable_active_installation": false
          },
          "findings": [
            { "status": "passed", "subject": "source", "detail": "/Users/op/project" },
            { "status": "failed", "subject": "config", "detail": "/Users/op/.deadreckon/config.toml missing", "action": "deadreckon init" },
            { "status": "passed", "subject": "sandbox sandbox-exec", "detail": "found at /usr/bin/sandbox-exec" },
            { "status": "\(supervisorFindingStatus)", "subject": "supervisor service", "detail": "the installed DeadReckon supervisor service points at a different binary or state directory", "action": "deadreckon supervisor status" }
          ],
          "primary_action": "deadreckon init",
          "verdict": {
            "kind": "blocked",
            "label": "blocked doctor",
            "subject": "/Users/op/project",
            "recommended_command": "deadreckon init",
            "explanation": "Doctor checked 4 setup areas and found 1 blocking setup issue(s).",
            "evidence": [["checks", "4 setup checks; 1 blocking issue(s); 0 optional warning(s)"]]
          }
        }
        """
    }

    static let doctorRepairs = """
    [
      { "attempted": "repair active installation", "result": "passed", "detail": "active shell installation reconciled" },
      { "attempted": "repair install receipt", "result": "passed", "detail": "receipt rewritten for the current binary" },
      { "attempted": "repair supervisor service", "result": "failed", "detail": "left unchanged: unit is unmanaged" }
    ]
    """
}
