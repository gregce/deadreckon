import Foundation

/// Gate Queue derivation: pure functions from `list --json` rollup rows to
/// the Quarterdeck taxonomy (design doc C1, composed per section 6.2).
///
/// Operator decision 8 is law here: every ranking input is a durable,
/// file-backed fact already present on the rollup row (phase, outcome,
/// receipt validity, stop reason, recency). No forward-looking estimate,
/// readiness score, or invented status exists anywhere in this file, and
/// nothing is re-derived from raw files that the rollup already joins.

// MARK: - Quarantine

/// A `jobs[]` element that failed to decode as a FleetRow. It renders as an
/// honest unknown row; it never poisons its siblings and never crashes.
public struct QuarantinedRow: Equatable, Hashable, Sendable {
    public let jobID: String?
    public let goal: String?
    public let reason: String
    /// The element's index within the decoded `jobs[]` array: a stable
    /// identity within one decode for rows that salvaged no job_id (two rows
    /// failing with the same reason must not collide in SwiftUI ForEach).
    public let ordinal: Int

    public init(jobID: String?, goal: String?, reason: String, ordinal: Int = 0) {
        self.jobID = jobID
        self.goal = goal
        self.reason = reason
        self.ordinal = ordinal
    }
}

// MARK: - Sections

/// The Quarterdeck taxonomy, plus one honest bucket for rows the app cannot
/// classify (unknown phase or undecodable row): quarantined, not guessed.
public enum QueueSection: String, CaseIterable, Sendable {
    case atTheGate
    case needsReview
    case approaching
    case underway
    case wrecked
    case unknown

    public var title: String {
        switch self {
        case .atTheGate: return "AT THE GATE"
        case .needsReview: return "NEEDS REVIEW"
        case .approaching: return "APPROACHING THE GATE"
        case .underway: return "UNDERWAY"
        case .wrecked: return "WRECKED / BLOCKED"
        case .unknown: return "UNKNOWN STATE"
        }
    }

    public var subtitle: String? {
        switch self {
        case .atTheGate: return "awaiting your decision"
        case .needsReview: return "stopped for your judgment; no proof claimed"
        case .unknown: return "rows the app could not classify; the CLI remains authoritative"
        default: return nil
        }
    }
}

// MARK: - Queue items

/// One ranked row of the Gate Queue.
public struct QueueItem: Equatable, Hashable, Sendable, Identifiable {
    public enum Kind: Equatable, Sendable {
        case job(FleetRow)
        case quarantined(QuarantinedRow)
    }

    public let kind: Kind
    public let section: QueueSection
    /// True for a waiting row whose stop reason is decision-shaped
    /// (operator input required, spend cap, wall-clock cap): the loop parked
    /// itself on purpose and only the operator can move it.
    public let needsDecision: Bool

    public init(kind: Kind, section: QueueSection, needsDecision: Bool) {
        self.kind = kind
        self.section = section
        self.needsDecision = needsDecision
    }

    public var id: String {
        switch kind {
        case .job(let row): return row.jobID
        case .quarantined(let row): return row.jobID ?? "quarantined-\(row.ordinal)"
        }
    }

    public var row: FleetRow? {
        if case .job(let row) = kind { return row }
        return nil
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(id)
        hasher.combine(section)
    }
}

// MARK: - The queue

public struct GateQueue: Equatable, Sendable {
    public let atTheGate: [QueueItem]
    public let needsReview: [QueueItem]
    public let approaching: [QueueItem]
    public let underway: [QueueItem]
    public let wrecked: [QueueItem]
    public let unknown: [QueueItem]

    public init(
        atTheGate: [QueueItem], needsReview: [QueueItem], approaching: [QueueItem],
        underway: [QueueItem], wrecked: [QueueItem], unknown: [QueueItem]
    ) {
        self.atTheGate = atTheGate
        self.needsReview = needsReview
        self.approaching = approaching
        self.underway = underway
        self.wrecked = wrecked
        self.unknown = unknown
    }

    public static let empty = GateQueue(
        atTheGate: [], needsReview: [], approaching: [], underway: [], wrecked: [], unknown: [])

    public func items(in section: QueueSection) -> [QueueItem] {
        switch section {
        case .atTheGate: return atTheGate
        case .needsReview: return needsReview
        case .approaching: return approaching
        case .underway: return underway
        case .wrecked: return wrecked
        case .unknown: return unknown
        }
    }

    public var allItems: [QueueItem] {
        QueueSection.allCases.flatMap { items(in: $0) }
    }

    public var nonEmptySections: [QueueSection] {
        QueueSection.allCases.filter { !items(in: $0).isEmpty }
    }

    public var isEmpty: Bool { allItems.isEmpty }
    public var jobCount: Int { allItems.count }

    /// Rows that demand the operator now: at the gate, terminal needs-review
    /// rows (the judge stopped for a human by definition), plus
    /// decision-shaped waiting rows. Feeds the menubar badge and the header
    /// counts.
    public var decisionCount: Int {
        atTheGate.count + needsReview.count + underway.filter(\.needsDecision).count
    }

    /// Rows with a live execution phase (running or verifying). Queued and
    /// waiting rows are not "live" and never light the live indicator.
    public var liveCount: Int {
        allItems.filter { item in
            guard let row = item.row else { return false }
            switch row.projection.phase {
            case .running, .verifyingChecks, .verifyingMeaning: return true
            default: return false
            }
        }.count
    }

    /// The C1 "menubar mirror" line, from counts only (P8: chips not prose).
    /// Deviation from the C1 mock, on purpose: APPROACHING is reported as its
    /// own count rather than folded into "underway", so a job at the
    /// verifying stage is visible in the ten-second glance.
    public var summaryLine: String {
        var parts: [String] = []
        if !atTheGate.isEmpty { parts.append("\(atTheGate.count) at the gate") }
        if !needsReview.isEmpty { parts.append("\(needsReview.count) need review") }
        if !approaching.isEmpty { parts.append("\(approaching.count) approaching") }
        if !underway.isEmpty { parts.append("\(underway.count) underway") }
        if !wrecked.isEmpty { parts.append("\(wrecked.count) wrecked") }
        if !unknown.isEmpty { parts.append("\(unknown.count) unknown") }
        return parts.isEmpty ? "no jobs" : parts.joined(separator: " \u{00B7} ")
    }
}

// MARK: - Derivation

public enum QueueDerivation {
    /// Stop reasons that park a waiting Job on the operator's desk. Durable
    /// protocol words, not an app heuristic: the loop recorded that it cannot
    /// proceed without an operator decision (raise the cap, feed input, or
    /// kill).
    public static let decisionShapedStopReasons: Set<StopReason> = [
        .operatorInputRequired, .spendCap, .wallCap,
    ]

    public static func derive(rows: [FleetRow], quarantined: [QuarantinedRow] = []) -> GateQueue {
        var atTheGate: [FleetRow] = []
        var needsReview: [FleetRow] = []
        var approaching: [FleetRow] = []
        var underway: [FleetRow] = []
        var wrecked: [FleetRow] = []
        var unknownRows: [FleetRow] = []

        for row in rows {
            switch classify(row) {
            case .atTheGate: atTheGate.append(row)
            case .needsReview: needsReview.append(row)
            case .approaching: approaching.append(row)
            case .underway: underway.append(row)
            case .wrecked: wrecked.append(row)
            case .unknown: unknownRows.append(row)
            }
        }

        return GateQueue(
            atTheGate: atTheGate
                .sorted(by: newestFirst)
                .map { item($0, in: .atTheGate) },
            needsReview: needsReview
                .sorted(by: newestFirst)
                // A needs_review outcome is an operator decision by
                // definition: the judge stopped for a human.
                .map { QueueItem(kind: .job($0), section: .needsReview, needsDecision: true) },
            approaching: rankApproaching(approaching)
                .map { item($0, in: .approaching) },
            underway: rankUnderway(underway)
                .map { item($0, in: .underway) },
            wrecked: wrecked
                .sorted(by: newestFirst)
                .map { item($0, in: .wrecked) },
            unknown: unknownRows
                .sorted(by: newestFirst)
                .map { item($0, in: .unknown) }
                + quarantined.map {
                    QueueItem(kind: .quarantined($0), section: .unknown, needsDecision: false)
                }
        )
    }

    /// Section membership from durable facts only:
    /// - AT THE GATE: terminal + outcome verified + receipt proof `valid`
    ///   (trust rule 6: VERIFIED renders only from the shared classifier).
    ///   An invalid or absent proof keeps a "verified" outcome OUT of this
    ///   section, unconditionally (trust rule 5).
    /// - NEEDS REVIEW: terminal + outcome needs_review (judge uncertain or
    ///   asked for revision). An operator decision by definition, ranked
    ///   below AT THE GATE, painted amber, never buried in the wrecks and
    ///   never granted a VERIFIED chip (design C1: "ranked lower and painted
    ///   amber, never hidden").
    /// - APPROACHING: verifying_checks | verifying_meaning.
    /// - UNDERWAY: running | queued | waiting.
    /// - WRECKED: every other terminal row (failed, blocked, cancelled,
    ///   budget/deadline/retry exhausted, and verified-with-invalid-proof).
    ///   The row's own glossary words say which.
    /// - unknown phase: quarantined, never guessed into a section.
    public static func classify(_ row: FleetRow) -> QueueSection {
        switch row.projection.phase {
        case .terminal:
            if row.projection.outcome == .verified, row.receipt?.verified == .valid {
                return .atTheGate
            }
            if row.projection.outcome == .needsReview {
                return .needsReview
            }
            return .wrecked
        case .verifyingChecks, .verifyingMeaning:
            return .approaching
        case .running, .queued, .waiting:
            return .underway
        case .unknown:
            return .unknown
        }
    }

    public static func isDecisionShaped(_ row: FleetRow) -> Bool {
        guard row.projection.phase == .waiting else { return false }
        guard let reason = row.projection.stopReason else { return false }
        return decisionShapedStopReasons.contains(reason)
    }

    private static func item(_ row: FleetRow, in section: QueueSection) -> QueueItem {
        QueueItem(kind: .job(row), section: section, needsDecision: isDecisionShaped(row))
    }

    private static func newestFirst(_ a: FleetRow, _ b: FleetRow) -> Bool {
        if a.updatedAt != b.updatedAt { return a.updatedAt > b.updatedAt }
        return a.jobID < b.jobID
    }

    /// Approaching ranks by durable pipeline position: verifying_meaning is
    /// past the deterministic checks and nearer the gate than
    /// verifying_checks. Recency breaks ties. Gate counts are display only
    /// and never rank (a failed attempt has no marker and no counts).
    private static func rankApproaching(_ rows: [FleetRow]) -> [FleetRow] {
        rows.sorted { a, b in
            let aMeaning = a.projection.phase == .verifyingMeaning
            let bMeaning = b.projection.phase == .verifyingMeaning
            if aMeaning != bMeaning { return aMeaning }
            return newestFirst(a, b)
        }
    }

    /// Underway ranks decision-readiness first (a decision-shaped waiting
    /// row outranks everything else in the section), then recency.
    private static func rankUnderway(_ rows: [FleetRow]) -> [FleetRow] {
        rows.sorted { a, b in
            let aDecision = isDecisionShaped(a)
            let bDecision = isDecisionShaped(b)
            if aDecision != bDecision { return aDecision }
            return newestFirst(a, b)
        }
    }
}

// MARK: - Menubar state

/// The menubar states, mirroring the exemplar's enum pattern
/// (error / live / idle family). Precedence is fixed: a missing binary
/// outranks everything, a pending decision outranks a degraded fleet, and a
/// degraded fleet outranks plain live activity (design 2.4.1: the icon is
/// badged on needs-decision, stale-lease, and supervisor-down).
public enum MenuBarFleetState: Equatable, Sendable {
    /// The binary is missing or the fleet scan failed: honest error glyph.
    case unavailable
    /// The gate/needs-review sections or a decision-shaped waiting row are
    /// non-empty: badge with the count.
    case attention(Int)
    /// No decision waits, but the fleet is degraded: confirmed-stale leases
    /// (debounced in FleetStore, never one raw poll) or the Watchkeeper is
    /// stopped. Amber attention so the operator returns.
    case degraded(staleLeases: Int, supervisorDown: Bool)
    /// Something is running or verifying: live variant with the count.
    case live(Int)
    /// Fleet known and quiet.
    case idle
    /// Nothing fetched yet this session.
    case loading
}

extension QueueDerivation {
    /// `staleLeaseCount` must be a debounced count (FleetStore's
    /// confirmed-stale set), and `supervisorDown` is true only for a
    /// positively-reported stopped Watchkeeper (an unknown chip never badges).
    public static func menuBarState(
        _ fleet: FleetStore.FleetState,
        staleLeaseCount: Int = 0,
        supervisorDown: Bool = false
    ) -> MenuBarFleetState {
        switch fleet {
        case .loading:
            return .loading
        case .unavailable:
            return .unavailable
        case .loaded(let queue):
            if queue.decisionCount > 0 { return .attention(queue.decisionCount) }
            if staleLeaseCount > 0 || supervisorDown {
                return .degraded(staleLeases: staleLeaseCount, supervisorDown: supervisorDown)
            }
            if queue.liveCount > 0 { return .live(queue.liveCount) }
            return .idle
        }
    }
}

// MARK: - Lenient list decoding

/// The decoded fleet surface: intact rows, quarantined rows, and the legacy
/// run count. One malformed `jobs[]` element must cost exactly one row.
public struct FleetDecodeResult: Equatable, Sendable {
    public let rows: [FleetRow]
    public let quarantined: [QuarantinedRow]
    public let runs: [RunRow]

    public init(rows: [FleetRow], quarantined: [QuarantinedRow], runs: [RunRow]) {
        self.rows = rows
        self.quarantined = quarantined
        self.runs = runs
    }
}

public enum FleetDecodeError: Error, LocalizedError, Equatable {
    case notAnObject
    case wrongKind(String)

    public var errorDescription: String? {
        switch self {
        case .notAnObject:
            return "list --json did not return a JSON object."
        case .wrongKind(let kind):
            return "list --json returned kind \"\(kind)\", expected \"list\"."
        }
    }
}

public enum FleetDecoder {
    /// Decodes a `list --json` envelope row-by-row so one malformed row
    /// quarantines itself instead of failing the whole fleet.
    /// JSONSerialization does the splitting because re-encoding through
    /// JSONValue would launder integers into doubles.
    public static func decodeList(_ data: Data) throws -> FleetDecodeResult {
        guard let top = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw FleetDecodeError.notAnObject
        }
        if let kind = top["kind"] as? String, kind != "list" {
            throw FleetDecodeError.wrongKind(kind)
        }

        var rows: [FleetRow] = []
        var quarantined: [QuarantinedRow] = []
        let decoder = DeadreckonJSON.decoder()

        for (index, element) in ((top["jobs"] as? [Any]) ?? []).enumerated() {
            guard JSONSerialization.isValidJSONObject([element]) else {
                quarantined.append(QuarantinedRow(
                    jobID: nil, goal: nil, reason: "jobs[] element is not a JSON value",
                    ordinal: index))
                continue
            }
            do {
                let rowData = try JSONSerialization.data(withJSONObject: element, options: [.fragmentsAllowed])
                rows.append(try decoder.decode(FleetRow.self, from: rowData))
            } catch {
                let object = element as? [String: Any]
                quarantined.append(QuarantinedRow(
                    jobID: object?["job_id"] as? String,
                    goal: object?["goal"] as? String,
                    reason: shortDecodeReason(error),
                    ordinal: index))
            }
        }

        var runs: [RunRow] = []
        for element in (top["runs"] as? [Any]) ?? [] {
            guard JSONSerialization.isValidJSONObject([element]),
                let runData = try? JSONSerialization.data(withJSONObject: element, options: [.fragmentsAllowed]),
                let run = try? decoder.decode(RunRow.self, from: runData)
            else { continue }  // legacy rows are a count, not a queue input
            runs.append(run)
        }

        return FleetDecodeResult(rows: rows, quarantined: quarantined, runs: runs)
    }

    private static func shortDecodeReason(_ error: Error) -> String {
        if let decoding = error as? DecodingError {
            switch decoding {
            case .keyNotFound(let key, _):
                return "row is missing \"\(key.stringValue)\""
            case .typeMismatch(_, let context), .valueNotFound(_, let context), .dataCorrupted(let context):
                let path = context.codingPath.map(\.stringValue).joined(separator: ".")
                return path.isEmpty ? context.debugDescription : "\(path): \(context.debugDescription)"
            @unknown default:
                break
            }
        }
        return String(describing: error)
    }
}
