import DeadreckonKit
import Foundation
import UserNotifications

/// The UNUserNotificationCenter side of the Kit's `UserNotifying` seam
/// (exemplar pattern: authorization requested lazily before the first
/// delivery, stable notification identifiers so repeats replace rather than
/// stack). Authorization is resolved fresh on every post via
/// getNotificationSettings (which never prompts and is cheap) — a denial is
/// NOT cached for the process lifetime, so enabling deadreckon later in
/// System Settings > Notifications takes effect on the next post without a
/// relaunch (a menubar login item may not relaunch for days). The system
/// prompt itself fires only while the status is notDetermined, so the user
/// is never re-prompted after deciding.
///
/// All derivation lives in the Kit (AttentionDerivation); this adapter only
/// moves an already-derived intent into the framework and a response back
/// out through the pure router.
final class UserNotificationAdapter: NSObject, UserNotifying, UNUserNotificationCenterDelegate {
    /// Set by the app delegate; called on the main queue.
    var onRoute: ((NotificationRoute) -> Void)?

    private let center = UNUserNotificationCenter.current()

    // Only touched on the main queue: coalesces concurrent resolutions so a
    // launch catch-up burst of posts triggers one settings read (and at most
    // one authorization prompt), never a stack of parallel requests.
    private var resolutionInFlight = false
    private var pendingCompletions: [(Bool) -> Void] = []

    override init() {
        super.init()
        center.delegate = self
        registerCategories()
    }

    // MARK: - UserNotifying

    func post(_ intent: NotificationIntent) {
        ensureAuthorized { [weak self] granted in
            guard let self, granted else { return }
            let content = UNMutableNotificationContent()
            content.title = intent.title
            content.body = intent.body
            content.sound = .default
            content.categoryIdentifier = intent.categoryID
            content.userInfo = AttentionDerivation.userInfo(for: intent)
            // deliveryIdentity is stable per reason+subject: a repeat (the
            // reseal row after a rolled-back sealing attempt) replaces the
            // delivered banner instead of stacking.
            let request = UNNotificationRequest(
                identifier: intent.deliveryIdentity, content: content, trigger: nil)
            self.center.add(request, withCompletionHandler: nil)
        }
    }

    // MARK: - Delegate

    /// Show banners even while the app is frontmost: the menubar shell is
    /// "open" almost always, and the return moment is the product.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .sound])
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        defer { completionHandler() }
        // Stringly userInfo -> typed route through the Kit's pure router
        // (tested without this framework).
        var info: [String: String] = [:]
        for (key, value) in response.notification.request.content.userInfo {
            if let key = key as? String, let value = value as? String {
                info[key] = value
            }
        }
        guard let route = AttentionDerivation.route(
            actionIdentifier: response.actionIdentifier, userInfo: info) else { return }
        DispatchQueue.main.async { [weak self] in
            self?.onRoute?(route)
        }
    }

    // MARK: - Internals

    private func registerCategories() {
        let open = UNNotificationAction(
            identifier: NotificationIdentity.openActionID,
            title: "Open",
            options: [.foreground])
        let reviewAtGate = UNNotificationAction(
            identifier: NotificationIdentity.reviewAtGateActionID,
            title: "Review & Approve",
            options: [.foreground])
        let general = UNNotificationCategory(
            identifier: NotificationIdentity.generalCategoryID,
            actions: [open],
            intentIdentifiers: [],
            options: [])
        let verified = UNNotificationCategory(
            identifier: NotificationIdentity.verifiedCategoryID,
            actions: [open, reviewAtGate],
            intentIdentifiers: [],
            options: [])
        center.setNotificationCategories([general, verified])
    }

    /// Resolves the CURRENT authorization on every call (settings reads
    /// never prompt): authorized/provisional delivers, denied stays silent
    /// for this post but is re-checked next time — a grant made in System
    /// Settings works immediately — and notDetermined asks once via the
    /// system prompt (the system itself will not re-prompt after a
    /// decision). Concurrent calls coalesce onto one in-flight resolution.
    /// Completion runs on the main queue.
    private func ensureAuthorized(_ completion: @escaping (Bool) -> Void) {
        guard Thread.isMainThread else {
            DispatchQueue.main.async { [weak self] in self?.ensureAuthorized(completion) }
            return
        }
        pendingCompletions.append(completion)
        guard !resolutionInFlight else { return }
        resolutionInFlight = true

        center.getNotificationSettings { [weak self] settings in
            switch settings.authorizationStatus {
            case .authorized, .provisional:
                self?.finishResolution(granted: true)
            case .denied:
                // Silent for now; re-checked on the next post so a later
                // grant in System Settings needs no relaunch.
                self?.finishResolution(granted: false)
            default:
                guard let self else { return }
                self.center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
                    self.finishResolution(granted: granted)
                }
            }
        }
    }

    private func finishResolution(granted: Bool) {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            let completions = self.pendingCompletions
            self.pendingCompletions = []
            self.resolutionInFlight = false
            completions.forEach { $0(granted) }
        }
    }
}
