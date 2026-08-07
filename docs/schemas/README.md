# Checked JSON Schemas

These files are generated from the Rust types that DeadReckon writes. Do not edit the schema JSON by hand.

The protocol ledger schemas are:

- `ledger-item.schema.json`: the tagged union of every canonical ledger item
- `run-event.schema.json`: `events.jsonl` records
- `spend-record.schema.json`: `spend.jsonl` records
- `trace-record.schema.json`: `traces.jsonl` records
- `flight-event.schema.json`: provider flight-event records
- `narrative-snapshot-ref.schema.json`: references to local narrative snapshots
- `notify-event.schema.json`: `notify.jsonl` records (operator-attention signals and delivery attempts)

`projections/run-view.schema.json` describes the JSON emitted by `deadreckon report --json`. Other named artifacts accepted by `deadreckon show --raw` are not Keel protocol ledgers and keep their existing formats.

Mixed-version caveat for `provenance.jsonl` (no checked schema): since M1, `extend --note` appends a kind-discriminated `{kind: "operator_sendback", note, parent_job_id, new_job_id, at}` row to the PARENT run's `provenance.jsonl` alongside the historical kind-less turn rows (`parent_job_id` carries the parent run id). Binaries and readers older than M1 that parse the file strictly as turn-shaped `ProvenanceRecord` rows will error on any parent extended with `--note` by a newer binary; readers must skip or type-switch on the presence of `kind`.

External observers may tail the JSONL ledgers these schemas describe directly; the per-file guarantees (append-only, torn-tail handling, strict sequencing on `job-events.jsonl`) are the supported contract in [`docs/TAILING.md`](../TAILING.md).

## Regenerating schemas

Set `DEADRECKON_UPDATE_SCHEMAS=1` only when an intentional Rust type change should update the checked files. The tests fail on schema drift and print the relevant regeneration command.

Regenerate the protocol schemas with:

```sh
DEADRECKON_UPDATE_SCHEMAS=1 cargo test -p deadreckon-protocol
```

Regenerate the report projection schema with:

```sh
DEADRECKON_UPDATE_SCHEMAS=1 cargo test -p deadreckon report_json_validates_against_generated_schema
```

Review the generated diff before committing it.
