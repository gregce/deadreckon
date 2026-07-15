# deadreckon — Pennant Rider (contracts as descriptor data)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-07-15-1658-deadreckon-pennant-goal.md`.
It supersedes nothing in prior riders — their invariants still apply, and the
Semaphore rider (`2026-07-11-1119-…-semaphore-rider.md`) is a hard
prerequisite: Pennant constructs the `ProviderContract` value Semaphore's
shared machinery consumes, from descriptor TOML instead of code. This rider
adds: the **`[contract]` descriptor section**, the **JSON-path extraction
dialect**, generic-driver honoring of contracts, and **fixture-grounded
onboarding** of cli:pi, cli:copilot, and (where the binaries offer structured
modes) cli:gemini and cli:opencode.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `~/.deadreckon`.
Grounding: the descriptors in `crates/deadreckon-providers/descriptors/` and
the INSTALLED binaries (`pi`, `copilot`, `gemini`, `opencode`) — never their
source repos, never assumed versions.

## Posture (decided — do not redesign)

- **Maturity stays stable** (lands under a `Pennant` CHANGELOG section).
- **Additive and optional.** A descriptor without `[contract]` behaves byte-identically to today (depth-tested). A malformed `[contract]` is a descriptor-load warning surfaced through the existing registry-warning path; the provider still works contract-less.
- **Declarative only.** Contract fields are literal args and JSON pointers. No templating beyond the existing `{prompt}`/`{conversation_id}` placeholders, no conditionals, no scripting. If a provider's contract cannot be expressed declaratively, it gets a bespoke mirror in a future slice — not a cleverer schema here.
- **Fixtures from real binaries** (Semaphore's claude rule, fleet-wide): each onboarded provider's fixtures are recorded from the actual installed binary and checked in. A provider whose installed binary offers no structured mode gets NO `[contract]` — an honest gap documented in the descriptor as a comment and in AS-BUILT, never a guessed contract.
- **Capability doctrine inherited**: probe → disable-with-caveat; tolerant parse; `provider.contract.degraded` on unparseable output; session file provider-scoped; resume never crosses runs. All from Semaphore — reused, not reimplemented.
- **Tokens land; dollars stay subscription/$0.** Any provider-reported cost goes to trace detail (claude precedent).
- **No `PipelineState` schema changes. No new crates. No `git push`. No V1 invention. Edits stay inside `/Users/gdc/deadreckon`.**

## Data model

### `[contract]` descriptor section (optional; all fields optional unless marked)

```toml
[contract]
# How to ask for structure. Replaces/augments exec_template args when present.
stream_args = ["--mode", "json", "--print"]        # REQUIRED if section present
dialect = "json-lines"                              # "json-lines" | "json-document"

# Where facts live. JSON Pointers (RFC 6901) evaluated per line (json-lines)
# or against the single document (json-document). Absent pointer = capability absent.
conversation_id_path = "/session_id"
usage_input_path  = "/usage/input_tokens"
usage_output_path = "/usage/output_tokens"
cost_path = "/total_cost_usd"                       # -> trace detail only
answer_path = "/result"
error_flag_path = "/is_error"                        # truthy => provider error
error_message_path = "/error/message"

# Event lines that should stream to the flight ledger live (json-lines only):
# a line matching ANY selector is appended verbatim under the [ingest] schema.
flight_event_paths = ["/type"]                       # selector: pointer must exist

# Resume, only if the binary supports it. {conversation_id} substituted.
resume_args = ["--session", "{conversation_id}"]

# Probe: contract activates only when --help output contains this substring.
probe_substring = "--mode"
```

Rules:
- `dialect = "json-lines"`: pointers are tried per parsed line; the LAST line
  where a pointer resolves wins for terminal facts (answer/usage/cost/error);
  `conversation_id_path` takes the FIRST resolution.
- `dialect = "json-document"`: whole stdout parsed once; pointers evaluated
  against it. `flight_event_paths` is invalid here (load warning).
- Pointer resolution to a missing path is a per-capability caveat, not an error.

### ProviderContract (Semaphore's machinery input)

```rust
pub(crate) struct ProviderContract { /* built by Semaphore for codex/claude in code */ }
impl ProviderContract {
    pub(crate) fn from_descriptor(section: &ContractSection) -> Result<Self, ContractError>;
}
```

Codex/claude keep their bespoke mirrors (their dialects are richer event
streams); Pennant's `from_descriptor` covers the declarative subset. One
machinery, two construction paths — depth-tested that both flow through the
same session/parse/flight/degrade code.

## Per-provider onboarding rules (P6–P9)

| Provider | Starting point | Onboarding rule |
|---|---|---|
| cli:pi | `--mode json --print` already in exec_template | Record fixtures; write `[contract]` for id/usage/answer/error; add `resume_args` ONLY if the installed binary documents a resume/session flag; flight selectors for tool events. |
| cli:copilot | `--output-format json` already in exec_template | Same; note `--stream off` is already set — dialect is likely `json-document`. |
| cli:gemini | plain `-p` | Probe installed binary for a structured output mode; onboard if present (fixtures), else document the gap and leave descriptor contract-less. |
| cli:opencode | plain `run` | Same rule as gemini. |

Each onboarded provider's phase commits: recorded fixtures, the descriptor
`[contract]`, and a fake-binary CI fixture replaying the recorded shape.
The exec_template/`[contract]` interaction must leave NON-contract invocations
(version probe, install hint) untouched.

## Phases (eleven)

Each phase: named depth test(s) first (red) → implement → `make verify` green
→ conventional-commit → CHANGELOG line naming the SHA.

### P1 — ContractSection parsing + validation
- Registry parses `[contract]`; malformed sections warn and drop; `json-document` + `flight_event_paths` rejected at load.

Depth tests:
- `contract_section_parses_full_and_minimal_forms`
- `malformed_contract_warns_and_provider_stays_usable`
- `document_dialect_rejects_flight_selectors`

### P2 — JSON-pointer extraction engine
Depth tests:
- `pointers_extract_from_json_lines_last_wins`
- `conversation_id_takes_first_resolution`
- `document_dialect_extracts_from_single_json`
- `missing_pointer_is_capability_caveat_not_error`

### P3 — ProviderContract::from_descriptor
- Bridges the section into Semaphore's machinery struct; probe_substring gates activation.

Depth tests:
- `descriptor_contract_flows_through_semaphore_machinery`
- `probe_substring_miss_disables_contract_with_caveat`

### P4 — Generic driver honors contracts
- `cli_generic.rs`: stream_args applied, extraction wired, degraded fallback preserved; contract-less descriptors byte-identical.

Depth tests:
- `contractless_descriptor_behavior_is_byte_identical`
- `contract_provider_reports_real_usage_and_answer`
- `unparseable_output_degrades_with_caveat_generic`

### P5 — Session + resume for descriptor contracts
- `provider-session.json` reuse; `resume_args` substitution; providers without resume_args stay fresh-per-turn silently (no caveat spam).

Depth tests:
- `descriptor_resume_substitutes_conversation_id`
- `no_resume_args_means_fresh_turns_without_caveat`

### P6 — Onboard cli:pi
- Real fixtures; `[contract]`; raw-JSON-as-content wart dies (answer extracted).

Depth tests:
- `pi_fixture_yields_usage_answer_and_session`
- `pi_response_content_is_answer_not_json_blob`

### P7 — Onboard cli:copilot
Depth tests:
- `copilot_fixture_yields_usage_and_answer`
- `copilot_document_dialect_parses_single_json`

### P8 — Probe-and-onboard cli:gemini
- If the installed binary offers a structured mode: fixtures + contract + tests; else: a documented-gap commit (descriptor comment + AS-BUILT note) and the depth test asserts the honest contract-less behavior.

Depth tests:
- `gemini_contract_or_documented_gap`  (name refined at implementation to match the outcome)

### P9 — Probe-and-onboard cli:opencode
Depth tests:
- `opencode_contract_or_documented_gap`

### P10 — Flight + friendliness
- `flight_event_paths` live ingestion (dedupe against `[ingest]` post-hoc import); `show`/`report` token rendering covers descriptor providers; `deadreckon providers` output marks which routes have contracts.

Depth tests:
- `descriptor_flight_events_stream_live_and_dedupe`
- `providers_listing_marks_contract_bearing_routes`

### P11 — Architecture doc + CHANGELOG (doc only)
- Insert `## 55. Pennant: Descriptor-Declared Contracts` into AS-BUILT (section schema, pointer dialect, onboarding table incl. any documented gaps); cross-reference §50.
- CHANGELOG:
  ```
  ## Pennant (stable) — contracts as descriptor data — <date>
  - CLI provider wire contracts are descriptor TOML: [contract] declares
    stream args, JSON paths, and resume; pi and copilot (whose JSON already
    flowed unparsed) gain real usage, extracted answers, sessions, and live
    flight events; gemini/opencode onboarded per what their binaries offer.
    A new agent CLI's contract is a TOML edit plus recorded fixtures.
  ```
- V1-CANDIDATES: contract hot-reload, operator-supplied contract overrides in config.toml, richer event-mirror escalation path.

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| malformed [contract] at load | warning names the field; `try: deadreckon providers check <name>` |
| resume failed twice | inherited from Semaphore (`show --raw provider-session`) |
| probe_substring miss | caveat: "installed <binary> predates its contract; upgrade to enable token accounting" |

## Out of scope (explicitly → V1-CANDIDATES)

- Bespoke event mirrors for any generic provider (escalation path documented, not built).
- Contract sections for cli:codex/cli:claude-code (their mirrors are richer; unifying is V1).
- Operator config-level contract overrides; contract hot-reload.
- Steering/app-server anything (Rudder).
- Guessed contracts for binaries that offer no structured mode.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1: serde/serde_json/toml (in tree; JSON Pointer via `serde_json::Value::pointer` — no new crate). Tier 2: none. Tier 3 (blocked): jsonpath/jq crates (RFC 6901 pointers suffice by design), linking any provider's source.

## Engineering invariants (do not violate)

- **Contract-less behavior is byte-identical** — the P4 test is the compatibility contract.
- **Declarative or bespoke, never clever**: the schema gains no logic; providers that outgrow it get mirrors.
- **Fixtures from real binaries only**; each onboarding commit names the binary version probed.
- **Documented gaps are first-class outcomes** — an honest "no structured mode" beats a fabricated contract; the P8/P9 tests pin whichever truth holds.
- **One machinery** — descriptor contracts and bespoke mirrors share Semaphore's session/parse/flight/degrade path (depth-tested).
- **One depth test before each phase.**

## Process invariants

- Phased local commits only. No `git push`.
- Each phase: depth tests green + CHANGELOG SHA line.
- P6–P9 each record the probed binary's version string in the commit message.
- If a provider's real output defies the declarative schema, stop, log the V1 escalation (bespoke mirror), ship the documented gap — do not extend the schema mid-slice.
