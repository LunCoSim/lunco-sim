//! Routes [`lunco_workbench::BrowserAction::OpenFile`] events with USD
//! extensions (`.usda`, `.usdc`) into the USD document open pipeline, so
//! a click on a `.usda` row in the Twin browser opens the file in the
//! shared USD viewport — same shape as Modelica's
//! `browser_dispatch::drain_browser_actions`, just gated on a different
//! filetype.
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
//! This module just translates browser-panel clicks into calls on the
//! domain-layer load pipeline. The filesystem read, the `OpenFile`
//! command observer, and the registry allocate live in
//! [`crate::commands`] so they also work in headless / sandbox bins
//! that never add `UsdUiPlugin`.

use bevy::prelude::*;
use lunco_workbench::{BrowserAction, BrowserActions};
use lunco_workspace::WorkspaceResource;

use crate::commands::spawn_usd_load;

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

/// Drain Twin-browser `OpenFile` actions whose path looks like USD and
/// hand each off to the domain load pipeline ([`spawn_usd_load`]), which
/// reads the file and allocates the document idempotently.
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

    let twin_root = {
        let ws = world.resource::<WorkspaceResource>();
        ws.active_twin
            .and_then(|id| ws.twin(id))
            .map(|t| t.root.clone())
    };
    let Some(root) = twin_root else {
        for a in &actions {
            bevy::log::warn!(
                "BrowserAction::OpenFile (USD) fired with no active Twin: {:?}",
                a
            );
        }
        return;
    };

    for action in actions {
        let BrowserAction::OpenFile { relative_path } = action else {
            continue;
        };
        let abs = root.join(&relative_path);
        spawn_usd_load(world, abs);
    }
}
