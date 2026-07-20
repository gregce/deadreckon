use std::path::Path;

use deadreckon_protocol::{
    EventLine, FlightLine, LedgerFile, LedgerItem, SpendLine, TraceLine, ledger_file_for,
    redact_for_persistence,
};

use crate::error::{DeadreckonError, Result};
use crate::state::append_json_line;

/// Apply protocol policy and resolve the existing relative ledger path.
pub fn prepare_ledger_item(item: LedgerItem) -> Result<(LedgerFile, LedgerItem)> {
    let item = redact_for_persistence(item);
    let file = ledger_file_for(&item).ok_or_else(|| {
        DeadreckonError::InvalidInput("unknown ledger items are not persisted".to_string())
    })?;
    Ok((file, item))
}

/// Append one bare protocol line after routing it through persistence policy.
pub fn append_ledger_item(run_root: &Path, item: LedgerItem) -> Result<()> {
    let (file, item) = prepare_ledger_item(item)?;
    let path = run_root.join(file.relative_path());
    match item {
        LedgerItem::Event(record) => append_json_line(&path, &EventLine(record)),
        LedgerItem::Spend(record) => append_json_line(&path, &SpendLine(record)),
        LedgerItem::Trace(record) => append_json_line(&path, &TraceLine(record)),
        LedgerItem::Flight(record) => append_json_line(&path, &FlightLine(record)),
        LedgerItem::NarrativeSnapshotRef(_) => Err(DeadreckonError::InvalidInput(
            "narrative snapshot bodies must use the narrative writer after policy routing"
                .to_string(),
        )),
        LedgerItem::Unknown => Err(DeadreckonError::InvalidInput(
            "unknown ledger items are not persisted".to_string(),
        )),
    }
}
