//! Shared path helpers for document-backed editor authoring.

use lunco_usd::document::UsdDocument;
use lunco_usd_bevy::SdfPath;

/// Join a parent prim path and a child name, including the stage root (`/`).
pub(crate) fn join_prim(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// Whether a prim is authored in either editable document layer.
pub(crate) fn prim_exists(host: &lunco_doc::DocumentHost<UsdDocument>, path: &str) -> bool {
    let Ok(sdf) = SdfPath::new(path) else {
        return false;
    };
    host.document().data().spec(&sdf).is_some()
        || host.document().runtime_data().spec(&sdf).is_some()
}
