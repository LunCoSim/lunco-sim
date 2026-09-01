//! Routes [`lunco_workbench::BrowserAction::OpenFile`] events with USD
//! extensions (`.usda`, `.usdc`) into the USD document open pipeline.
//!
//! A browser click means **open and preview this source**, never **replace the
//! running scene**. A Twin contains reusable vehicle, material, and support
//! layers as well as scene roots; treating every layer as a `LoadScene` tore
//! down the current world when a user merely inspected a referenced rover.
//! Loading a world is an explicit Scenarios action.
//!
//! ## File partitioning
//!
//! [`BrowserActions::take_where`] only removes the actions whose path
//! has a `.usda` / `.usdc` extension, leaving Modelica's `.mo` opens
//! for the Modelica drain to handle in the same frame. Two crates,
//! one shared outbox, no ordering coupling.
//!
//! ## UI-only
//!
//! This module just translates browser-panel clicks into the document-load
//! pipeline. The filesystem read and registry allocation live in
//! [`crate::commands`] so they also work in headless / sandbox bins that never
//! add `UsdUiPlugin`.

use bevy::prelude::*;
use lunco_workbench::{BrowserAction, BrowserActions};
use lunco_workspace::WorkspaceResource;

/// Lower-cased extensions this dispatch recognises as USD files.
/// `.usdc` (binary) is included so users get a *parser failure*
/// message instead of having the click silently misrouted to another
/// domain — the openusd 0.2.0 text reader will fail on binary input
/// and [`crate::ui::viewport`] surfaces the warning.
const USD_EXTENSIONS: &[&str] = &["usda", "usdc"];

fn is_usd_open_file(action: &BrowserAction) -> bool {
    match action {
        BrowserAction::OpenFile { relative_path } => relative_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|ext| {
                let lower = ext.to_ascii_lowercase();
                USD_EXTENSIONS.iter().any(|e| *e == lower)
            })
            .unwrap_or(false),
        _ => false,
    }
}

/// Resolve a browser file selection to an on-disk document.
///
/// Browser sections may already know an absolute path. That form is already
/// resolved by the emitting section and must not be re-anchored on the active
/// Twin; relative paths remain scoped to the active Twin.
fn browser_document_path(
    root: Option<&std::path::Path>,
    selected: &std::path::Path,
) -> Option<std::path::PathBuf> {
    if selected.is_absolute() {
        return Some(selected.to_path_buf());
    }
    let root = root?;
    let absolute = root.join(selected);
    if absolute.strip_prefix(root).is_ok() {
        Some(absolute)
    } else {
        None
    }
}

/// Drain Twin-browser `OpenFile` actions whose path looks like USD and hand
/// each off to the document pipeline ([`crate::commands::spawn_usd_load`]).
/// This deliberately does not trigger [`crate::LoadScene`].
pub fn drain_browser_actions_for_usd(world: &mut World) {
    let actions: Vec<BrowserAction> = {
        // Bail gracefully when the workbench's outbox isn't present
        // (headless / lifecycle tests add `UsdUiPlugin` without the
        // workbench plugin). `resource_mut` would panic.
        let Some(mut outbox) = world.get_resource_mut::<BrowserActions>() else {
            return;
        };
        outbox.take_where(is_usd_open_file)
    };
    if actions.is_empty() {
        return;
    }

    let (active_twin, twin_roots) = world
        .get_resource::<WorkspaceResource>()
        .map(|ws| {
            (
                ws.active_twin
                    .and_then(|id| ws.twin(id))
                    .map(|t| t.root.clone()),
                ws.twins()
                    .map(|(_, twin)| twin.root.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap_or_default();
    for action in actions {
        let BrowserAction::OpenFile { relative_path } = action else {
            continue;
        };
        let Some(abs) = browser_document_path(active_twin.as_deref(), &relative_path) else {
            bevy::log::warn!(
                "BrowserAction::OpenFile (USD) needs an active Twin for relative path: {:?}",
                relative_path
            );
            continue;
        };
        let owner_root = twin_roots
            .iter()
            .filter(|root| abs.strip_prefix(root).is_ok())
            .max_by_key(|root| root.components().count())
            .cloned();
        crate::commands::spawn_usd_load(world, abs, true, owner_root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_usd_selection_stays_within_the_active_twin() {
        let root = std::env::temp_dir()
            .join("lunco-browser-dispatch")
            .join("twin");
        let rover = root.join("sim").join("rovers").join("lunokhod2.usda");
        let traverse = root.join("sim").join("scenes").join("traverse.usda");
        let outside = root
            .parent()
            .expect("test root has a parent")
            .join("solar_system.usda");

        assert_eq!(
            browser_document_path(
                Some(&root),
                std::path::Path::new("sim/rovers/lunokhod2.usda"),
            ),
            Some(rover)
        );
        assert_eq!(
            browser_document_path(Some(&root), &traverse),
            Some(traverse)
        );
        assert_eq!(
            browser_document_path(Some(&root), &outside),
            Some(outside),
            "an already-resolved absolute browser path must not be re-anchored on the active Twin"
        );
    }
}
