//! Pure persistence policy for ledger items.

use serde_json::Value;

use crate::{LedgerFile, LedgerItem, RunEventKind};

/// Return the existing ledger file for a persisted item.
pub const fn ledger_file_for(item: &LedgerItem) -> Option<LedgerFile> {
    LedgerFile::for_item(item)
}

/// Whether today's persistence policy writes this item.
pub const fn is_persisted(item: &LedgerItem) -> bool {
    ledger_file_for(item).is_some()
}

/// Apply the safety transformations required before an item reaches disk.
pub fn redact_for_persistence(mut item: LedgerItem) -> LedgerItem {
    match &mut item {
        LedgerItem::Event(event) => {
            if let RunEventKind::ToolCallStarted { args, .. } = &mut event.event {
                redact_gate_nonce_values(args);
            }
        }
        LedgerItem::Trace(trace) => redact_gate_nonce_values(&mut trace.detail),
        LedgerItem::Spend(_)
        | LedgerItem::Flight(_)
        | LedgerItem::NarrativeSnapshotRef(_)
        | LedgerItem::Unknown => {}
    }
    item
}

fn redact_gate_nonce_values(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "gate_nonce" | "gate-nonce" | "gate/nonce") {
                    *value = Value::String("[REDACTED]".to_string());
                } else {
                    redact_gate_nonce_values(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_gate_nonce_values(value);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}
