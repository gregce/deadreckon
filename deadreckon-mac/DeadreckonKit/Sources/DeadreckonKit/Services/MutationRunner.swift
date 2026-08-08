import Foundation

// APP-4: the single verb dispatcher. Every mutation the app performs goes
// through this one choke point, so trust rules 1, 2, 8, and 9 are enforced
// in exactly one place: argv arrays through the FleetCLIRunning seam (never
// a shell string), --json always present, flags first and a literal `--`
// terminating the flag section before any OPERATOR-TYPED positional (a goal
// or note beginning with "-", or shaped like "--plan=/path", is always text
// to the binary, never a flag), the literal CLI line exposed for the sheet
// to display, success decoded into envelopes and refusal into the typed
// error envelope. It cannot construct a dr-gate invocation by design:
// PlannedVerb is the complete verb surface and has no such case, and no
// case carries a force/override parameter.

// MARK: - The launch request (G2 preview leg inputs)

public struct StartRequest: Equatable, Sendable {
    public var goal: String
    public var provider: String?
    public var model: String?
    public var maxSpendUSD: Double?
    /// The project source directory (`start --from PATH`: copied into
    /// runstate before launch). nil resolves from the client's cwd, which
    /// the sheet states honestly.
    public var projectDirectory: String?

    public init(goal: String, provider: String? = nil, model: String? = nil,
                maxSpendUSD: Double? = nil, projectDirectory: String? = nil) {
        self.goal = goal
        self.provider = provider
        self.model = model
        self.maxSpendUSD = maxSpendUSD
        self.projectDirectory = projectDirectory
    }
}

// MARK: - Finish destination (mapped 1:1 to documented flags)

public enum FinishDestinationChoice: Equatable, Sendable {
    /// Apply to the working tree: `--autostash` / `--cleanup` toggles.
    case apply(autostash: Bool, cleanup: Bool)
    /// Export to a directory: `--dest DIR`.
    case export(path: String)

    public var arguments: [String] {
        switch self {
        case .apply(let autostash, let cleanup):
            var flags: [String] = []
            if autostash { flags.append("--autostash") }
            if cleanup { flags.append("--cleanup") }
            return flags
        case .export(let path):
            // The runner executes argv directly (no shell) and the binary
            // takes the path verbatim, so a typed "~" must expand here or a
            // directory literally named "~" appears under the cwd.
            return ["--dest", (path as NSString).expandingTildeInPath]
        }
    }
}

// MARK: - The complete verb surface

/// Every state-changing invocation the app can make. There is deliberately
/// no dr-gate case, no sign case, and no case parameter that could express
/// "force past a failed digest" — the enum is the proof that no code path
/// retries a mutation with different authority.
public enum PlannedVerb: Equatable, Sendable {
    case steer(id: String, note: String)
    case kill(id: String, escalate: Bool)
    /// Report-only promote preview (G4). No --yes: nothing may mutate.
    case finishDryRun(id: String, destination: FinishDestinationChoice)
    case finish(id: String, destination: FinishDestinationChoice)
    case extendJob(parentID: String, goal: String, note: String?)
    /// Read-only launch preview (`will_start: false`). No --yes.
    case startPreview(StartRequest)
    /// Execute leg: replay the exact plan file the preview embedded. The
    /// launch plan does NOT carry the source directory (start.rs resolves
    /// source mode from the invocation), so `fromPath` must be repeated
    /// here when the preview used one. `spendAcknowledged` may ONLY be fed
    /// from `SpendAcknowledgement.armsFlag` (the typed-amount match).
    case startExecute(planFilePath: String, spendAcknowledged: Bool, fromPath: String? = nil)
    /// Declare the done contract from plain English: `def-done [--dir D]
    /// [--provider P] [--model M] --yes --json -- <criteria>`. The binary
    /// (never the app) writes .deadreckon/acceptance.yaml in the project;
    /// `--yes` is the operator approval the interactive flow would have
    /// collected, so this case may ONLY be dispatched from an explicit
    /// operator action (the sheet's Draft-contract click IS the approval).
    /// nil directory declares in the client's working directory, exactly as
    /// the preview leg resolves its source (the binary's --dir default).
    case defDoneDeclare(directory: String?, criteria: String,
                        provider: String? = nil, model: String? = nil)
    /// Read back the declared contract: `def-done show --json [--dir D]`.
    /// Read-only (no --yes): a missing contract is a normal exit-0
    /// "default_gate" envelope, never a refusal.
    case defDoneShow(directory: String?)
    /// `config set KEY VALUE --json`. The key is app-authored (the binary's
    /// validated key set); the VALUE is operator-typed, so both ride after
    /// the `--` terminator. Secret keys (providers.<route>.api_key) are
    /// refused by the binary toward set-key — this case cannot carry one
    /// usefully by construction.
    case configSet(key: String, value: String)
    /// `config unset KEY --json`. Removing an absent key is a binary-side
    /// no-op envelope, never a refusal.
    case configUnset(key: String)
    /// `config set-key ROUTE --json` — the secret NEVER rides this enum
    /// (Equatable, displayable): argv carries only the route, and the key
    /// bytes travel exclusively through the dispatch call's stdin parameter
    /// (SETTINGS spec rule 4).
    case configSetKey(route: String)
    /// `config unset-key ROUTE --json` (the shipped removal form).
    case configUnsetKey(route: String)
    /// `supervisor install --json`: writes the managed per-user unit only;
    /// enable/start stays a separate verb.
    case supervisorInstall
    /// `supervisor start --json`: enable + start the installed unit.
    case supervisorStart
    /// `supervisor stop --json`: disable + unload; the unit is retained.
    case supervisorStop
    /// `doctor --repair --json`: the binary's three bounded repairs, with
    /// their outcomes serialized in the report's repairs[] rows. Repairs
    /// gain no authority from the app — this only dispatches and renders.
    case doctorRepair
    /// `rewind RUN --to-checkpoint C --preview --json`: read-only preview —
    /// the binary materializes the checkpoint into a preview directory and
    /// reports the files that would change WITHOUT touching the workspace.
    /// Both ids are file/envelope truth (checkpoint manifests, run
    /// resolution), never operator-typed text.
    case rewindPreview(runID: String, checkpoint: String)
    /// `rewind RUN --to-checkpoint C --apply --json`: the hash-guarded
    /// apply. The guard is the CLI's, not the app's: a drifted file refuses
    /// binary-side ("refusing rewind because {path} has unrelated edits")
    /// and the refusal renders verbatim. Preview-first is enforced by the
    /// RewindCoordinator — this case is only reachable from a previewed
    /// state.
    case rewindApply(runID: String, checkpoint: String)
    /// `undo ID --no-confirm --json`. The shipped binary REQUIRES
    /// `--no-confirm` for a non-interactive Job undo (undo.rs: "non-
    /// interactive Job undo requires --no-confirm"); the sheet's destructive
    /// confirm click IS that confirmation, exactly as the finish sheet's
    /// Approve is `--yes`. The vendored 0.8.4 binary predates `undo --json`
    /// entirely, so the affordance is capability-gated (VerbCapabilityProbe)
    /// and never renders against it.
    case undo(id: String)
    /// `try --json`: the keyless local smoke proof (first-run §R2 "prove
    /// it"). Runs a real tiny run in a throwaway workspace inside
    /// DEADRECKON_HOME — state-changing, so it rides the one dispatcher —
    /// and emits a bare proof document, NOT a kind-scaffold envelope. Its
    /// trust words ("untrusted local smoke diagnostic") render verbatim.
    case tryProof

    public var verbWord: String {
        switch self {
        case .steer: return "steer"
        case .kill: return "kill"
        case .finishDryRun, .finish: return "finish"
        case .extendJob: return "extend"
        case .startPreview, .startExecute: return "start"
        case .defDoneDeclare, .defDoneShow: return "def-done"
        case .configSet, .configUnset, .configSetKey, .configUnsetKey: return "config"
        case .supervisorInstall, .supervisorStart, .supervisorStop: return "supervisor"
        case .doctorRepair: return "doctor"
        case .rewindPreview, .rewindApply: return "rewind"
        case .undo: return "undo"
        case .tryProof: return "try"
        }
    }

    /// The argv array. Flags come first and a literal `--` terminates the
    /// flag section before every OPERATOR-TYPED positional (goal, note), so
    /// pasted text like "--plan=/path/evil.json" or "- fix login" reaches
    /// clap as text, never as a flag. IDs decoded from the binary's own
    /// envelopes are not operator text and stay in place. Never joined into
    /// a shell string for execution (display uses `literalCommandLine`).
    public var arguments: [String] {
        switch self {
        case .steer(let id, let note):
            return ["steer", "--json", "--", id, note]
        case .kill(let id, let escalate):
            var argv = ["kill", id]
            if escalate { argv.append("--escalate") }
            argv.append("--json")
            return argv
        case .finishDryRun(let id, let destination):
            return ["finish", id, "--dry-run"] + destination.arguments + ["--json"]
        case .finish(let id, let destination):
            return ["finish", id] + destination.arguments + ["--yes", "--json"]
        case .extendJob(let parentID, let goal, let note):
            var argv = ["extend"]
            if let note {
                // Single =-token so a note beginning with "-" survives clap
                // value parsing.
                argv.append("--note=\(note)")
            }
            argv.append(contentsOf: ["--yes", "--json", "--", parentID, goal])
            return argv
        case .startPreview(let request):
            var argv = ["start"]
            if let provider = request.provider {
                argv.append(contentsOf: ["--provider", provider])
            }
            if let model = request.model {
                argv.append(contentsOf: ["--model", model])
            }
            if let cap = request.maxSpendUSD {
                argv.append(contentsOf: ["--max-spend", Self.usd(cap)])
            }
            if let from = request.projectDirectory {
                argv.append(contentsOf: ["--from", from])
            }
            argv.append(contentsOf: ["--json", "--", request.goal])
            return argv
        case .startExecute(let planFilePath, let spendAcknowledged, let fromPath):
            var argv = ["start", "--plan", planFilePath, "--yes"]
            if spendAcknowledged {
                argv.append("--i-know-its-a-lot")
            }
            if let fromPath {
                argv.append(contentsOf: ["--from", fromPath])
            }
            argv.append("--json")
            return argv
        case .defDoneDeclare(let directory, let criteria, let provider, let model):
            var argv = ["def-done"]
            if let directory {
                argv.append(contentsOf: ["--dir", directory])
            }
            if let provider {
                argv.append(contentsOf: ["--provider", provider])
            }
            if let model {
                argv.append(contentsOf: ["--model", model])
            }
            // The criteria is operator-typed plain English: `--` first, so
            // pasted text shaped like a flag reaches clap as literal text.
            argv.append(contentsOf: ["--yes", "--json", "--", criteria])
            return argv
        case .defDoneShow(let directory):
            // "show" is the app-authored subcommand word, not operator text.
            var argv = ["def-done", "show", "--json"]
            if let directory {
                argv.append(contentsOf: ["--dir", directory])
            }
            return argv
        case .configSet(let key, let value):
            // The value is operator-typed: `--` first so "-5" or a pasted
            // "--flag" reaches clap as literal text.
            return ["config", "set", "--json", "--", key, value]
        case .configUnset(let key):
            return ["config", "unset", "--json", "--", key]
        case .configSetKey(let route):
            // argv carries ONLY the route; the secret rides stdin.
            return ["config", "set-key", "--json", "--", route]
        case .configUnsetKey(let route):
            return ["config", "unset-key", "--json", "--", route]
        case .supervisorInstall:
            return ["supervisor", "install", "--json"]
        case .supervisorStart:
            return ["supervisor", "start", "--json"]
        case .supervisorStop:
            return ["supervisor", "stop", "--json"]
        case .doctorRepair:
            return ["doctor", "--repair", "--json"]
        case .rewindPreview(let runID, let checkpoint):
            return ["rewind", runID, "--to-checkpoint", checkpoint, "--preview", "--json"]
        case .rewindApply(let runID, let checkpoint):
            return ["rewind", runID, "--to-checkpoint", checkpoint, "--apply", "--json"]
        case .undo(let id):
            return ["undo", id, "--no-confirm", "--json"]
        case .tryProof:
            return ["try", "--json"]
        }
    }

    /// Long-running verbs get long timeouts; the watchdog SIGTERMs a hung
    /// child and the result reports its real exit.
    public var timeout: TimeInterval {
        switch self {
        case .steer, .kill, .extendJob: return 60
        case .finishDryRun, .finish: return 600
        // Declare drafts through the configured provider inside the
        // binary's own cumulative authoring budget.
        case .startPreview, .startExecute, .defDoneDeclare: return 300
        case .defDoneShow: return 60
        // Config writes are one TOML round-trip.
        case .configSet, .configUnset, .configSetKey, .configUnsetKey: return 60
        // Supervisor lifecycle shells out to launchctl/systemctl.
        case .supervisorInstall, .supervisorStart, .supervisorStop: return 120
        // Repair may reinstall the service and rewrite the receipt.
        case .doctorRepair: return 300
        // Rewind materializes a full checkpoint tree into the preview dir.
        case .rewindPreview, .rewindApply: return 300
        // Undo is one snapshot restore or one exact revert commit.
        case .undo: return 120
        // The smoke proof runs a real tiny run; the row shows elapsed time.
        case .tryProof: return 600
        }
    }

    static func usd(_ value: Double) -> String {
        if value == value.rounded(), abs(value) < 1_000_000 {
            return String(Int(value))
        }
        return String(format: "%.2f", value)
    }
}

// MARK: - The runner

public final class MutationRunner {
    private let cli: FleetCLIRunning

    public init(cli: FleetCLIRunning) {
        self.cli = cli
    }

    /// The literal CLI line a sheet displays for the verb it is about to run
    /// (design 2.4.3). Display-only: execution always uses the argv array.
    public func literalCommandLine(for verb: PlannedVerb) -> String {
        Self.commandLine(arguments: verb.arguments)
    }

    public static func commandLine(arguments: [String]) -> String {
        (["deadreckon"] + arguments.map(shellQuote)).joined(separator: " ")
    }

    /// Run the verb to completion and classify its machine output. A locator
    /// failure (no trusted binary) or launch failure comes back as an
    /// envelope-free result carrying the failure's own words.
    ///
    /// `stdin` is the secret-over-stdin path (`configSetKey` only): the
    /// bytes go to the child through the CLI seam and are never retained,
    /// logged, or echoed by this runner — the verb's argv (and therefore
    /// every command well and transcript) carries only the route.
    public func run(_ verb: PlannedVerb, stdin: Data? = nil) async -> MutationResult {
        do {
            let result = try await cli.run(
                arguments: verb.arguments, timeout: verb.timeout, stdin: stdin)
            return MutationResult.classify(
                stdout: result.stdout, stderr: result.stderr, exitCode: result.exitCode)
        } catch {
            let words = (error as? FleetCLIError)?.errorDescription
                ?? error.localizedDescription
            return MutationResult(
                envelopes: [], refusal: nil, rawObjects: [],
                exitCode: -1, stdout: "", stderr: words)
        }
    }

    /// Write the preview's embedded launch-plan payload to an app-owned
    /// scratch file for the execute leg. NEVER under DEADRECKON_HOME
    /// (single-writer discipline, trust rule 8): the binary is the only
    /// writer of harness state, and it reads this plan wherever it lives.
    public static func writeLaunchPlanFile(_ data: Data) throws -> URL {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("deadreckon-mac-plans", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appendingPathComponent("launch-plan-\(UUID().uuidString).json")
        try data.write(to: url, options: [.atomic])
        return url
    }

    /// Launch-time housekeeping: remove stale launch-plan scratch files from
    /// earlier sessions. The age guard keeps a concurrently running second
    /// instance's fresh plan file safe.
    public static func sweepLaunchPlanFiles(olderThan age: TimeInterval = 3600,
                                            now: Date = Date()) {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("deadreckon-mac-plans", isDirectory: true)
        guard let names = try? FileManager.default.contentsOfDirectory(atPath: directory.path) else {
            return
        }
        for name in names where name.hasPrefix("launch-plan-") {
            let url = directory.appendingPathComponent(name)
            let modified = (try? FileManager.default.attributesOfItem(atPath: url.path))?[
                .modificationDate] as? Date
            if let modified, now.timeIntervalSince(modified) < age { continue }
            try? FileManager.default.removeItem(at: url)
        }
    }

    /// POSIX single-quote quoting for display; alphanumeric-safe words stay
    /// bare so the line reads like what an operator would type.
    static func shellQuote(_ argument: String) -> String {
        let safe = CharacterSet(
            charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._/@%+:=,-")
        if !argument.isEmpty, argument.unicodeScalars.allSatisfy({ safe.contains($0) }) {
            return argument
        }
        return "'" + argument.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
}
