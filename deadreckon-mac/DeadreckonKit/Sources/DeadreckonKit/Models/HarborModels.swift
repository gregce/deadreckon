import Foundation

/// Harbor strip read-models: the low-frequency `providers list --json` and
/// `supervisor status --json` surfaces, decoded to exactly the facts the
/// quiet chips render. Unknown keys are ignored; unknown words degrade to
/// `.unknown` per the ForgivingStringEnum contract.

/// `ProbeStatus` (deadreckon-providers registry/mod.rs, snake_case).
public enum ProviderProbeStatus: String, ForgivingStringEnum, CaseIterable {
    case ok
    case failed
    case skipped
    case unknown
}

/// One entry of `providers[]` from `providers list --json`
/// (ProviderProbeResult). The chip subset plus, since APP-4, the probe's own
/// failure words and try lines: the Lay Course provider picker shows failed
/// probes visible-but-disabled with these as the fix hints, verbatim.
public struct ProviderProbeRow: Codable, Equatable, Sendable {
    public let id: String
    public let displayName: String?
    public let status: ProviderProbeStatus
    public let message: String?
    public let tryLines: [String]

    enum CodingKeys: String, CodingKey {
        case id
        case displayName = "display_name"
        case status
        case message
        case tryLines = "try_lines"
    }

    public init(id: String, displayName: String?, status: ProviderProbeStatus,
                message: String? = nil, tryLines: [String] = []) {
        self.id = id
        self.displayName = displayName
        self.status = status
        self.message = message
        self.tryLines = tryLines
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        displayName = try container.decodeIfPresent(String.self, forKey: .displayName)
        status = try container.decode(ProviderProbeStatus.self, forKey: .status)
        message = try container.decodeIfPresent(String.self, forKey: .message)
        tryLines = try container.decodeIfPresent([String].self, forKey: .tryLines) ?? []
    }
}

/// `models --json` (models_command, commands/providers.rs): per-provider
/// model catalogs for the Lay Course model picker. `default` marks the
/// configured default; `recommended` is the registry's own flag.
public struct ModelsEnvelope: Codable, Equatable, Sendable {
    public struct Entry: Codable, Equatable, Sendable {
        public let id: String
        public let recommended: Bool
        public let isDefault: Bool

        enum CodingKeys: String, CodingKey {
            case id, recommended
            case isDefault = "default"
        }

        public init(id: String, recommended: Bool, isDefault: Bool) {
            self.id = id
            self.recommended = recommended
            self.isDefault = isDefault
        }

        public init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            id = try container.decode(String.self, forKey: .id)
            recommended = try container.decodeIfPresent(Bool.self, forKey: .recommended) ?? false
            isDefault = try container.decodeIfPresent(Bool.self, forKey: .isDefault) ?? false
        }
    }

    public struct ProviderModels: Codable, Equatable, Sendable {
        public let provider: String
        public let displayName: String?
        public let models: [Entry]

        enum CodingKeys: String, CodingKey {
            case provider
            case displayName = "display_name"
            case models
        }

        public init(provider: String, displayName: String?, models: [Entry]) {
            self.provider = provider
            self.displayName = displayName
            self.models = models
        }

        public init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            provider = try container.decode(String.self, forKey: .provider)
            displayName = try container.decodeIfPresent(String.self, forKey: .displayName)
            models = try container.decodeIfPresent([Entry].self, forKey: .models) ?? []
        }
    }

    public let providers: [ProviderModels]

    public init(providers: [ProviderModels]) {
        self.providers = providers
    }
}

/// `providers list --json` top-level envelope (`{kind:"providers", ...}`).
public struct ProvidersEnvelope: Codable, Equatable, Sendable {
    public let kind: String
    public let providers: [ProviderProbeRow]
    public let missingProviders: [String]
    public let active: String?

    enum CodingKeys: String, CodingKey {
        case kind
        case providers
        case missingProviders = "missing_providers"
        case active
    }

    public init(kind: String, providers: [ProviderProbeRow], missingProviders: [String], active: String?) {
        self.kind = kind
        self.providers = providers
        self.missingProviders = missingProviders
        self.active = active
    }

    /// Chip fact: how many probed routes passed, out of how many entries the
    /// registry answered for (probed plus missing). Pure counting, no health
    /// invention.
    public var okCount: Int { providers.filter { $0.status == .ok }.count }
    public var totalCount: Int { providers.count + missingProviders.count }
}

/// `doctor --json` (doctor.rs `doctor_json_payload`): the startup-handshake
/// surface. Only finding statuses are counted; detail prose stays in the
/// binary's own words for tooltips.
public struct DoctorEnvelope: Codable, Equatable, Sendable {
    public struct Finding: Codable, Equatable, Sendable {
        public let status: String
        public let subject: String
        public let detail: String

        public init(status: String, subject: String, detail: String) {
            self.status = status
            self.subject = subject
            self.detail = detail
        }
    }

    public let kind: String
    /// The verdict-surface kind word, rendered not interpreted.
    public let status: String
    public let findings: [Finding]

    public init(kind: String, status: String, findings: [Finding]) {
        self.kind = kind
        self.status = status
        self.findings = findings
    }

    public var failedCount: Int { findings.filter { $0.status == "failed" }.count }
    public var warningCount: Int { findings.filter { $0.status == "warning" }.count }
}

/// `ServiceInstallationState` (supervisor_service.rs, snake_case).
public enum SupervisorInstallState: String, ForgivingStringEnum, CaseIterable {
    case unsupported
    case notInstalled = "not_installed"
    case stale
    case current
    case unknown
}

/// `supervisor status --json` (SupervisorServiceStatusReport). Only the
/// chip-relevant subset is decoded; the checkpoint evidence block is APP-3+
/// territory and is deliberately not modeled here.
public struct SupervisorStatusReport: Codable, Equatable, Sendable {
    public let schemaVersion: Int
    public let manager: String
    public let installed: SupervisorInstallState
    public let loaded: Bool?
    public let active: String?

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case manager
        case installed
        case loaded
        case active
    }

    public init(schemaVersion: Int, manager: String, installed: SupervisorInstallState, loaded: Bool?, active: String?) {
        self.schemaVersion = schemaVersion
        self.manager = manager
        self.installed = installed
        self.loaded = loaded
        self.active = active
    }

    /// Mirrors Rust `ServiceManagerRuntime::is_running` exactly:
    /// launchd reports `loaded`, systemd reports `active == "active"`.
    /// Display data for a chip, never a liveness authority.
    public var isRunning: Bool {
        loaded == true || active == "active"
    }
}
