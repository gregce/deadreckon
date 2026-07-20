# Checked JSON Schemas

These files are generated from the Rust types that DeadReckon writes. Do not edit the schema JSON by hand.

The protocol ledger schemas are:

- `ledger-item.schema.json`: the tagged union of every canonical ledger item
- `run-event.schema.json`: `events.jsonl` records
- `spend-record.schema.json`: `spend.jsonl` records
- `trace-record.schema.json`: `traces.jsonl` records
- `flight-event.schema.json`: provider flight-event records
- `narrative-snapshot-ref.schema.json`: references to local narrative snapshots

`projections/run-view.schema.json` describes the JSON emitted by `deadreckon report --json`. Other named artifacts accepted by `deadreckon show --raw` are not Keel protocol ledgers and keep their existing formats.

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
