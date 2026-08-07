import Foundation

/// One user notification the app intends to post, derived purely from a
/// typed `operator_attention` row (APP-5, operator decision 6). Every fact
/// here comes from the record's own fields plus the owning job of the tailed
/// run root; nothing is derived from the time of observation, so the same
/// record always yields the same identity — across polls, tails rebuilt from
/// offset 0, and app relaunches.
public struct NotificationIntent: Equatable, Sendable, Identifiable {
    /// Unique per RECORD: `attention|<reason>|<job or ->|<run or ->|<at ms>`.
    /// The seen-set dedupes on this, so re-observation cannot re-fire.
    public let recordIdentity: String
    /// Stable per reason+subject: repeated notifications for the same fact
    /// (e.g. the reseal row after a rolled-back sealing attempt, TAILING.md)
    /// REPLACE the delivered banner rather than stacking (the exemplar's
    /// stable-notification-ID pattern).
    public let deliveryIdentity: String
    public let reason: OperatorAttentionReason
    /// App-authored short label per reason (same provenance rule as the
    /// GlossaryText phase words: one honest label, nothing predictive).
    public let title: String
    /// The record's `summary`, verbatim (trust rule 2).
    public let body: String
    /// The record's job_id, else the job whose run root carried the row
    /// (paused_at_cap rows have no job_id; the tailed root names the owner).
    public let jobID: String?
    public let runID: String?
    public let scope: String?
    public let at: Date
    /// Notification category: verified_awaiting_promote gets the extra
    /// "Review at Gate" action; everything else gets Open only.
    public let categoryID: String

    public var id: String { recordIdentity }
}

/// Where a notification response routes (pure derivation, tested without
/// the UserNotifications framework).
public enum NotificationRoute: Equatable, Sendable {
    /// Open the window onto the job's workbench.
    case openJob(jobID: String)
    /// Open the window onto the promote sheet (the APP-4 WriteSurfaceRouter
    /// owns the navigation; the sheet's own gate stays authoritative).
    case reviewAtGate(jobID: String)
}

/// Stable identifiers shared between the Kit derivation and the app's
/// UNUserNotificationCenter adapter.
public enum NotificationIdentity {
    public static let openActionID = "DEADRECKON_OPEN"
    public static let reviewAtGateActionID = "DEADRECKON_REVIEW_AT_GATE"
    public static let generalCategoryID = "deadreckon.attention.general"
    public static let verifiedCategoryID = "deadreckon.attention.verified"
    public static let jobIDUserInfoKey = "jobID"
    public static let reasonUserInfoKey = "reason"
    /// The framework's default action identifier (a banner tap), mirrored
    /// here so the pure router never imports UserNotifications.
    public static let defaultActionID = "com.apple.UNNotificationDefaultActionIdentifier"
}

/// Pure derivation from a decoded `operator_attention` row to a
/// NotificationIntent, and from a notification response back to a route.
public enum AttentionDerivation {
    /// Nil for `.unknown` reasons: an unrecognized vocabulary word cannot be
    /// labeled honestly, so it is observed (and marked seen) but never posted.
    public static func intent(from row: OperatorAttentionRow, owningJobID: String?) -> NotificationIntent? {
        guard row.reason != .unknown else { return nil }
        let atMillis = Int64((row.at.timeIntervalSince1970 * 1000).rounded())
        let recordIdentity = [
            "attention",
            row.reason.rawValue,
            row.jobID ?? "-",
            row.runID ?? "-",
            String(atMillis),
        ].joined(separator: "|")
        let subject = row.jobID ?? row.runID ?? row.scope ?? "fleet"
        let deliveryIdentity = ["attention", row.reason.rawValue, subject].joined(separator: "|")
        return NotificationIntent(
            recordIdentity: recordIdentity,
            deliveryIdentity: deliveryIdentity,
            reason: row.reason,
            title: title(for: row.reason),
            body: row.summary,
            jobID: row.jobID ?? owningJobID,
            runID: row.runID,
            scope: row.scope,
            at: row.at,
            categoryID: row.reason == .verifiedAwaitingPromote
                ? NotificationIdentity.verifiedCategoryID
                : NotificationIdentity.generalCategoryID)
    }

    /// App-authored reason labels. One word set per reason, no readiness or
    /// prediction language (operator decision 8); the summary itself stays
    /// the binary's verbatim words.
    public static func title(for reason: OperatorAttentionReason) -> String {
        switch reason {
        case .verifiedAwaitingPromote: return "Verified, awaiting your promote"
        case .pausedAtCap: return "Paused at cap"
        case .blocked: return "Job blocked"
        case .failed: return "Job failed"
        case .cancelled: return "Job cancelled"
        case .waitingInput: return "Needs your review"
        case .unknown: return GlossaryText.unknownState
        }
    }

    /// The userInfo payload the adapter attaches to a posted notification.
    public static func userInfo(for intent: NotificationIntent) -> [String: String] {
        var info: [String: String] = [NotificationIdentity.reasonUserInfoKey: intent.reason.rawValue]
        if let jobID = intent.jobID {
            info[NotificationIdentity.jobIDUserInfoKey] = jobID
        }
        return info
    }

    /// Routes a notification response. A banner tap (default action) and the
    /// explicit Open action open the job; Review at Gate opens the promote
    /// sheet. Nil when there is no job to route to (the honest no-op) or the
    /// action is a dismissal.
    public static func route(actionIdentifier: String, userInfo: [String: String]) -> NotificationRoute? {
        guard let jobID = userInfo[NotificationIdentity.jobIDUserInfoKey], !jobID.isEmpty else {
            return nil
        }
        switch actionIdentifier {
        case NotificationIdentity.reviewAtGateActionID:
            return .reviewAtGate(jobID: jobID)
        case NotificationIdentity.openActionID, NotificationIdentity.defaultActionID:
            return .openJob(jobID: jobID)
        default:
            return nil
        }
    }
}

/// Per-reason notification preferences (Settings, APP-5). Missing defaults
/// read as enabled; `.unknown` is never notifiable regardless.
public struct AttentionPreferences: Equatable, Sendable {
    public static let masterKey = "notify.master.enabled"
    public static func reasonKey(_ reason: OperatorAttentionReason) -> String {
        "notify.reason.\(reason.rawValue).enabled"
    }

    /// The six notifiable reasons, in Settings display order. `.unknown`
    /// deliberately absent.
    public static let notifiableReasons: [OperatorAttentionReason] = [
        .verifiedAwaitingPromote, .pausedAtCap, .waitingInput, .blocked, .failed, .cancelled,
    ]

    public var masterEnabled: Bool
    public var enabledReasons: Set<OperatorAttentionReason>

    public init(masterEnabled: Bool = true,
                enabledReasons: Set<OperatorAttentionReason> = Set(Self.notifiableReasons)) {
        self.masterEnabled = masterEnabled
        self.enabledReasons = enabledReasons
    }

    public func allows(_ reason: OperatorAttentionReason) -> Bool {
        guard masterEnabled, reason != .unknown else { return false }
        return enabledReasons.contains(reason)
    }

    public static func load(from defaults: UserDefaults) -> AttentionPreferences {
        let master = defaults.object(forKey: masterKey) as? Bool ?? true
        var enabled: Set<OperatorAttentionReason> = []
        for reason in notifiableReasons {
            if defaults.object(forKey: reasonKey(reason)) as? Bool ?? true {
                enabled.insert(reason)
            }
        }
        return AttentionPreferences(masterEnabled: master, enabledReasons: enabled)
    }

    public func save(to defaults: UserDefaults) {
        defaults.set(masterEnabled, forKey: Self.masterKey)
        for reason in Self.notifiableReasons {
            defaults.set(enabledReasons.contains(reason), forKey: Self.reasonKey(reason))
        }
    }
}

/// The bounded-tail plan (design "bounded: tail only rows in non-terminal or
/// recently-terminal states, poll-fallback for the rest"). Pure and tested.
public enum AttentionTailPlan {
    public struct Plan: Equatable, Sendable {
        /// Rows that get a live notify.jsonl tail this tick, newest first.
        public let active: [FleetRow]
        /// Rows covered by the slow full-scan sweep instead (the seen-set
        /// makes the re-read safe).
        public let fallback: [FleetRow]
    }

    /// A terminal row stays tail-worthy this long after its last update:
    /// the supervisor's terminal classification row can land shortly after
    /// the phase flip, and the tail must still be listening.
    public static let recentTerminalWindow: TimeInterval = 900

    public static func select(rows: [FleetRow], limit: Int, now: Date,
                              recentTerminalWindow: TimeInterval = recentTerminalWindow) -> Plan {
        let eligible = rows.filter { row in
            row.projection.phase != .terminal
                || now.timeIntervalSince(row.updatedAt) <= recentTerminalWindow
        }
        let ranked = eligible.sorted { $0.updatedAt > $1.updatedAt }
        let active = Array(ranked.prefix(max(0, limit)))
        let activeIDs = Set(active.map(\.jobID))
        let fallback = rows.filter { !activeIDs.contains($0.jobID) }
        return Plan(active: active, fallback: fallback)
    }
}
