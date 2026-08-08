import Foundation

/// First-run completeness (SETTINGS-SCREENS-SPEC §R2), as a pure
/// derivation: the setup panel replaces the empty state exactly when the
/// fleet is empty AND setup is incomplete — any of: doctor reports
/// `config_present == false`, zero configured agent routes, or the service
/// is positively not installed.
///
/// Unknown facts never claim incompleteness: a probe that has not answered
/// (or failed) is not evidence that setup is missing, so an operator with a
/// broken doctor never gets nagged by a setup panel — the standard empty
/// state (with its own degraded surfaces) renders instead. Only a
/// positively-known gap summons the panel.
public enum SetupDerivation {
    public struct Inputs: Equatable, Sendable {
        /// doctor --json `config_present`; nil while unknown/unavailable.
        public var configPresent: Bool?
        /// Number of agent routes the providers probe reported (probe rows
        /// present, regardless of status); nil while unknown/unavailable.
        public var agentRouteCount: Int?
        /// True exactly when the service status positively reported
        /// "not installed"; nil while unknown/unavailable. Stopped and
        /// degraded states are remediation for Settings, not first-run.
        public var serviceNotInstalled: Bool?

        public init(configPresent: Bool? = nil, agentRouteCount: Int? = nil,
                    serviceNotInstalled: Bool? = nil) {
            self.configPresent = configPresent
            self.agentRouteCount = agentRouteCount
            self.serviceNotInstalled = serviceNotInstalled
        }
    }

    /// True when any KNOWN fact says setup is missing.
    public static func isIncomplete(_ inputs: Inputs) -> Bool {
        if inputs.configPresent == false { return true }
        if inputs.agentRouteCount == 0 { return true }
        if inputs.serviceNotInstalled == true { return true }
        return false
    }

    /// True once every probe has answered (whatever it said): the panel-or-
    /// empty-state decision waits for this so the operator never sees a
    /// flash of the wrong surface.
    public static func isResolved(_ inputs: Inputs) -> Bool {
        inputs.configPresent != nil && inputs.agentRouteCount != nil
            && inputs.serviceNotInstalled != nil
    }
}
