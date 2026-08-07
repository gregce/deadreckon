import Foundation

/// The persisted seen-set behind notification dedupe: record identities that
/// have already been processed (posted, filtered, or too old to fire). Backed
/// by UserDefaults with a bounded LRU so the key can never grow without
/// limit; surviving relaunch is the point — launch-time catch-up fires only
/// identities this store has never seen.
public final class NotificationSeenStore {
    public static let defaultKey = "attention.seen"
    public static let defaultCapacity = 512

    private let defaults: UserDefaults
    private let key: String
    private let capacity: Int
    /// Ordered oldest-first; membership mirrored in a set for O(1) contains.
    private var order: [String]
    private var members: Set<String>

    public init(defaults: UserDefaults = .standard,
                key: String = NotificationSeenStore.defaultKey,
                capacity: Int = NotificationSeenStore.defaultCapacity) {
        self.defaults = defaults
        self.key = key
        self.capacity = max(1, capacity)
        let loaded = defaults.stringArray(forKey: key) ?? []
        // Deduplicate defensively while preserving the last occurrence's
        // recency (a corrupted default costs staleness, never a crash).
        var seen: Set<String> = []
        var ordered: [String] = []
        for identity in loaded.reversed() where !seen.contains(identity) {
            seen.insert(identity)
            ordered.append(identity)
        }
        self.order = ordered.reversed()
        self.members = seen
        trimIfNeeded()
    }

    public var count: Int { order.count }

    public func contains(_ identity: String) -> Bool {
        members.contains(identity)
    }

    /// Marks an identity seen, refreshing its LRU position. Persists
    /// synchronously; the array is small by construction.
    public func markSeen(_ identity: String) {
        if members.contains(identity) {
            if order.last != identity, let index = order.lastIndex(of: identity) {
                order.remove(at: index)
                order.append(identity)
                persist()
            }
            return
        }
        members.insert(identity)
        order.append(identity)
        trimIfNeeded()
        persist()
    }

    private func trimIfNeeded() {
        while order.count > capacity {
            let evicted = order.removeFirst()
            members.remove(evicted)
        }
    }

    private func persist() {
        defaults.set(order, forKey: key)
    }
}
