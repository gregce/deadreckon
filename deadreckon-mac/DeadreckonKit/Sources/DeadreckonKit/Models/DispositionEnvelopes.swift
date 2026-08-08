import Foundation

// Implementer B (SETTINGS-SCREENS-SPEC §R1/§R2/§R3): disposition + screen
// read-models, each reconciled against the SHIPPED serializer before any
// decoder was written (spec §P rule). Ground truth:
// - rewind: crates/deadreckon/src/main.rs rewind_command — the `--json`
//   success payload predates G1 and is a BESPOKE document (no kind
//   scaffold); refusals are armed to the shared error envelope in the
//   concurrent Rust batch and are PROSE on the vendored 0.8.4 binary.
// - undo: crates/deadreckon/src/commands/undo.rs + main.rs undo_command +
//   chain/mod.rs — the armed G1 scaffold (kind "undo") with an `undo_kind`
//   discriminator. Not spoken by the vendored 0.8.4 binary at all.
// - library list: live-corroborated against 0.8.4 (2026-08-07, real home).
// - try: live-corroborated against 0.8.4 (2026-08-07, scratch home) — a
//   bare proof document, not a kind-scaffold envelope.
// All decoders fail closed: a shape that does not carry its required facts
// returns nil and the surface renders the established envelope-free
// pattern, never a guessed state.

// MARK: - rewind --json (bespoke success payload, shipped shape)

/// The `rewind --json` success document (preview AND apply — `mode` says
/// which). CORRECTED from spec §P7's guess: `files` is a plain array of
/// changed paths — there is NO per-file change word and NO per-file
/// hash-guard state in the shipped payload. The hash guard runs binary-side
/// at apply time only; a drifted file arrives as a refusal quoting
/// "refusing rewind because {path} has unrelated edits", rendered verbatim.
public struct RewindEnvelope: Equatable, Sendable {
    public let runID: String
    /// "preview" | "apply" (rewind_mode_label).
    public let mode: String
    /// target {kind: turn|provider_event|checkpoint, id} — which selector
    /// resolved the checkpoint.
    public let targetKind: String?
    public let targetID: String?
    public let checkpointID: String
    public let previewDir: String?
    /// Changed paths, relative to the run workspace. Paths only (see above).
    public let files: [String]
    public let primaryAction: String?
    public let verdict: VerdictBlock?

    public init?(data: Data) {
        guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let runID = object["run_id"] as? String,
              let mode = object["mode"] as? String,
              let checkpointID = object["checkpoint_id"] as? String else { return nil }
        self.runID = runID
        self.mode = mode
        self.checkpointID = checkpointID
        let target = object["target"] as? [String: Any]
        targetKind = target?["kind"] as? String
        targetID = target?["id"] as? String
        previewDir = object["preview_dir"] as? String
        files = object["files"] as? [String] ?? []
        primaryAction = object["primary_action"] as? String
        if let verdictObject = object["verdict"],
           let verdictData = try? JSONSerialization.data(withJSONObject: verdictObject) {
            verdict = try? DeadreckonJSON.decoder().decode(VerdictBlock.self, from: verdictData)
        } else {
            verdict = nil
        }
    }
}

// MARK: - undo --json (armed G1 scaffold, shipped shape)

/// The armed `undo --json` success envelope: the shared `{kind:"undo", id,
/// status, next_actions, try_lines}` scaffold plus `undo_kind`-discriminated
/// facts (undo.rs / main.rs / chain/mod.rs):
/// - "run-snapshot": restored_turn, snapshot, workspace
/// - "job-delivery": destination, target_ref, reverted_revision,
///   undo_revision, already_undone
/// - "chain": undone_steps, workspace
/// Facts a kind did not emit stay nil, never guessed. `status` is the
/// verdict word ("completed", or "no-op" when already undone).
public struct UndoEnvelope: Equatable, Sendable {
    public let kind: String
    public let id: String?
    public let status: String?
    public let nextActions: [String]
    public let undoKind: String?
    // run-snapshot facts
    public let restoredTurn: Int?
    public let snapshot: String?
    public let workspace: String?
    // job-delivery facts
    public let destination: String?
    public let targetRef: String?
    public let revertedRevision: String?
    public let undoRevision: String?
    public let alreadyUndone: Bool?
    // chain facts
    public let undoneSteps: Int?
    public let verdict: VerdictBlock?

    public init?(data: Data) {
        guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let kind = object["kind"] as? String, kind == "undo" else { return nil }
        self.kind = kind
        id = object["id"] as? String
        status = object["status"] as? String
        nextActions = object["next_actions"] as? [String] ?? []
        undoKind = object["undo_kind"] as? String
        restoredTurn = (object["restored_turn"] as? NSNumber)?.intValue
        snapshot = object["snapshot"] as? String
        workspace = object["workspace"] as? String
        destination = object["destination"] as? String
        targetRef = object["target_ref"] as? String
        revertedRevision = object["reverted_revision"] as? String
        undoRevision = object["undo_revision"] as? String
        alreadyUndone = object["already_undone"] as? Bool
        undoneSteps = (object["undone_steps"] as? NSNumber)?.intValue
        if let verdictObject = object["verdict"],
           let verdictData = try? JSONSerialization.data(withJSONObject: verdictObject) {
            verdict = try? DeadreckonJSON.decoder().decode(VerdictBlock.self, from: verdictData)
        } else {
            verdict = nil
        }
    }
}

// MARK: - library list --json (real at 0.8.4, live-corroborated)

/// One promoted artifact: `{manifest{...}, path, materialized_count}`.
/// `payload_files` / `payload_bytes` are integers and ABSENT on
/// schema_version-1 manifests (older promotes) — absent stays nil, and the
/// row simply omits the size facts.
public struct LibraryArtifact: Equatable, Sendable, Identifiable {
    public struct Manifest: Equatable, Sendable {
        public let runID: String
        public let scope: String
        public let goal: String
        public let promotedAtRaw: String?
        public let promotedAt: Date?
        public let sourceWorkingDir: String?
        public let provenanceHash: String?
        public let payloadFiles: Int?
        public let payloadBytes: Int?
    }

    public let manifest: Manifest
    /// The artifact directory under `<home>/library/<scope>/<run>`.
    public let path: String
    public let materializedCount: Int?

    public var id: String { path }

    init?(object: [String: Any]) {
        guard let manifestObject = object["manifest"] as? [String: Any],
              let runID = manifestObject["run_id"] as? String,
              let scope = manifestObject["scope"] as? String,
              let goal = manifestObject["goal"] as? String,
              let path = object["path"] as? String else { return nil }
        let promotedAtRaw = manifestObject["promoted_at"] as? String
        manifest = Manifest(
            runID: runID,
            scope: scope,
            goal: goal,
            promotedAtRaw: promotedAtRaw,
            promotedAt: promotedAtRaw.flatMap(DeadreckonJSON.date(from:)),
            sourceWorkingDir: manifestObject["source_working_dir"] as? String,
            provenanceHash: manifestObject["provenance_hash"] as? String,
            payloadFiles: (manifestObject["payload_files"] as? NSNumber)?.intValue,
            payloadBytes: (manifestObject["payload_bytes"] as? NSNumber)?.intValue)
        self.path = path
        materializedCount = (object["materialized_count"] as? NSNumber)?.intValue
    }
}

/// `library list --json` (live shape): kind "library_list", id
/// "current-scope" | "all-scopes", status, artifacts[], next_actions,
/// try_lines. An artifact row that fails to decode costs exactly that row
/// (counted, never guessed), mirroring the fleet's quarantine discipline.
public struct LibraryListEnvelope: Equatable, Sendable {
    public let kind: String
    public let id: String?
    public let status: String?
    public let artifacts: [LibraryArtifact]
    public let unreadableCount: Int
    public let nextActions: [String]
    public let tryLines: [String]

    public init?(data: Data) {
        guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let kind = object["kind"] as? String, kind == "library_list",
              let rows = object["artifacts"] as? [Any] else { return nil }
        self.kind = kind
        id = object["id"] as? String
        status = object["status"] as? String
        var decoded: [LibraryArtifact] = []
        var unreadable = 0
        for row in rows {
            if let rowObject = row as? [String: Any],
               let artifact = LibraryArtifact(object: rowObject) {
                decoded.append(artifact)
            } else {
                unreadable += 1
            }
        }
        artifacts = decoded
        unreadableCount = unreadable
        nextActions = object["next_actions"] as? [String] ?? []
        tryLines = object["try_lines"] as? [String] ?? []
    }
}

// MARK: - try --json (real at 0.8.4, live-corroborated)

/// The keyless smoke proof's bare document: `{run_id, trust,
/// trusted_job_receipt, gate, proof, story, lineage, next}`. `trust`
/// ("untrusted local smoke diagnostic") and `gate` ("local smoke gate
/// evidence only; not a trusted Job receipt") are the binary's own trust
/// words and ALWAYS render verbatim — the proof row never claims more than
/// the binary claimed.
public struct TryProofEnvelope: Equatable, Sendable {
    public let runID: String
    public let trust: String
    public let trustedJobReceipt: Bool
    public let gate: String?
    public let proofPath: String?
    public let storyPath: String?
    public let lineage: String?
    public let next: String?

    public init?(data: Data) {
        guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let runID = object["run_id"] as? String,
              let trust = object["trust"] as? String else { return nil }
        self.runID = runID
        self.trust = trust
        trustedJobReceipt = object["trusted_job_receipt"] as? Bool ?? false
        gate = object["gate"] as? String
        proofPath = object["proof"] as? String
        storyPath = object["story"] as? String
        lineage = object["lineage"] as? String
        next = object["next"] as? String
    }
}
