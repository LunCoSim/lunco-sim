//! CQ-111: shared CSV-export helpers used by both the graphs panel
//! (`export_graph_to_csv`) and the experiments panel
//! (`export_experiment_csv`). The two callers build their CSV bodies
//! differently (graphs forward-fills a merged multi-series time axis;
//! experiments uses a single shared time axis), so only the field
//! escaping and the save-dialog/write/error-console boilerplate are
//! shared here.

use bevy::prelude::*;

/// Append `s` as a single CSV field to `out`, RFC-4180 quoting when it
/// contains a comma, double-quote, or newline (doubling embedded quotes).
pub fn push_csv_field(out: &mut String, s: &str) {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        out.push('"');
        for c in s.chars() {
            if c == '"' {
                out.push('"');
            }
            out.push(c);
        }
        out.push('"');
    } else {
        out.push_str(s);
    }
}

/// Prompt for a save location and write `bytes` there.
///
/// Returns the chosen [`lunco_storage::StorageHandle`] on success so the
/// caller can emit its own (panel-specific) success message. Returns
/// `None` when the user cancels the dialog or the write fails; in the
/// failure case the error is already logged to the console here (the
/// message is identical across callers).
pub fn save_csv_via_dialog(
    world: &mut World,
    suggested_name: &str,
    bytes: &[u8],
) -> Option<lunco_storage::StorageHandle> {
    use lunco_storage::Storage as _;

    let storage = lunco_storage::FileStorage::new();
    let hint = lunco_workbench::picker::SaveHint {
        suggested_name: Some(suggested_name.to_string()),
        start_dir: None,
        filters: vec![lunco_workbench::picker::OpenFilter::new("CSV", &["csv"])],
    };
    let handle = lunco_workbench::picker::pick_save_blocking(&hint)?; // user cancelled

    match futures_lite::future::block_on(storage.write(&handle, bytes)) {
        Ok(()) => Some(handle),
        Err(e) => {
            if let Some(mut console) =
                world.get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
            {
                console.error(format!("CSV export: write failed: {e}"));
            }
            None
        }
    }
}
