import Foundation
import ServiceManagement

/// Thin wrapper over SMAppService.mainApp for the launch-at-login setting
/// (the exemplar pattern, verbatim shape). Errors surface to the caller so
/// Settings can render the failure honestly instead of flipping a lying
/// toggle.
enum LaunchAtLogin {
    static var isEnabled: Bool {
        SMAppService.mainApp.status == .enabled
    }

    static func setEnabled(_ enabled: Bool) throws {
        if enabled {
            try SMAppService.mainApp.register()
        } else {
            try SMAppService.mainApp.unregister()
        }
    }
}
