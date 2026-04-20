//! Drains [`lunco_workbench::BrowserActions`] and routes each into the
//! appropriate Modelica subsystem.
//!
//! Sections push abstract intents (`OpenFile`, `OpenModelicaClass`)
//! during render; this system picks them up the same frame and turns
//! them into concrete app behaviour:
//!
//! - **`OpenFile`** — opens the file as a Modelica document via the
//!   existing `package_browser::open_model` path. Same code that runs
//!   when the user clicks a file in the legacy browser, so we get tab
//!   spawning + worker registration + welcome-tab swap-out for free.
//! - **`OpenModelicaClass`** — same as `OpenFile`, *plus* records a
//!   "when this file finishes loading, push this qualified path into
//!   `DrilledInClassNames`" entry on [`PendingDrillIns`]. The package
//!   browser's load-task handler reads it after allocating the
//!   document and applies it before opening the model-view tab, so
//!   the canvas projector lands on the right class on first paint.

use bevy::prelude::*;
use lunco_workbench::{BrowserAction, BrowserActions};
use std::collections::HashMap;

use crate::ui::panels::canvas_diagram::DrilledInClassNames;
use crate::ui::state::ModelLibrary;

/// `file_id (absolute path string) → qualified class path` queued by
/// [`drain_browser_actions`]. The package-browser's load-task handler
/// drains the matching entry the moment a Document is allocated and
/// pushes it into [`DrilledInClassNames`], so the canvas projector
/// drills into the right class without any second click.
///
/// Entries linger if the load fails (the queue would silently grow);
/// in practice that doesn't happen because every load path either
/// completes or the app restarts. A future polish step: GC entries
/// older than ~10 s.
#[derive(Resource, Default)]
pub struct PendingDrillIns {
    by_file_id: HashMap<String, String>,
}

impl PendingDrillIns {
    /// Stash a drill-in to apply when `file_id` finishes loading.
    pub fn queue(&mut self, file_id: String, qualified_path: String) {
        self.by_file_id.insert(file_id, qualified_path);
    }

    /// Pop the queued qualified path for `file_id`, if any.
    pub fn take(&mut self, file_id: &str) -> Option<String> {
        self.by_file_id.remove(file_id)
    }
}

/// Drain the Twin Browser action outbox each frame and dispatch.
///
/// Runs in `Update` after the panel render so actions queued during
/// the egui pass are picked up the same frame they were emitted.
pub fn drain_browser_actions(world: &mut World) {
    // Pull actions out of the world so we can mutate other resources
    // (DocumentRegistry, WorkbenchState, …) freely while iterating.
    let actions: Vec<BrowserAction> = {
        let mut outbox = world.resource_mut::<BrowserActions>();
        outbox.drain()
    };
    if actions.is_empty() {
        return;
    }

    // Resolve `relative_path` → absolute path string against the
    // currently-open Twin's root. Captured once so we don't fight the
    // borrow checker re-borrowing `OpenTwin` per action.
    let twin_root = world
        .resource::<lunco_workbench::OpenTwin>()
        .0
        .as_ref()
        .map(|t| t.root.clone());

    for action in actions {
        match action {
            BrowserAction::OpenFile { relative_path } => {
                let Some(root) = twin_root.as_ref() else {
                    log::warn!(
                        "BrowserAction::OpenFile fired with no OpenTwin: {:?}",
                        relative_path
                    );
                    continue;
                };
                let abs = root.join(&relative_path);
                let id = abs.to_string_lossy().to_string();
                let name = relative_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Untitled")
                    .to_string();
                crate::ui::panels::package_browser::open_model(
                    world,
                    id,
                    name,
                    ModelLibrary::User,
                );
            }
            BrowserAction::OpenModelicaClass {
                relative_path,
                qualified_path,
            } => {
                let Some(root) = twin_root.as_ref() else {
                    log::warn!(
                        "BrowserAction::OpenModelicaClass fired with no OpenTwin: {:?}",
                        relative_path
                    );
                    continue;
                };
                let abs = root.join(&relative_path);
                let id = abs.to_string_lossy().to_string();
                let name = relative_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Untitled")
                    .to_string();
                // Queue the drill-in target *first* so it's already
                // there when the load task completes — even though
                // `open_model` spawns the load asynchronously, the
                // resource write happens synchronously here so there's
                // no race.
                world
                    .resource_mut::<PendingDrillIns>()
                    .queue(id.clone(), qualified_path);
                crate::ui::panels::package_browser::open_model(
                    world,
                    id,
                    name,
                    ModelLibrary::User,
                );
            }
            BrowserAction::OpenLoadedClass {
                doc_id,
                qualified_path,
            } => {
                let doc = lunco_doc::DocumentId::new(doc_id);
                // Set the drill-in target *before* the tab opens so
                // the canvas projector picks it up on first paint.
                world
                    .resource_mut::<DrilledInClassNames>()
                    .set(doc, qualified_path);
                // Ensure the model-view tab exists / is focused for
                // this document. Same call the package-browser load
                // handler makes after a fresh allocation.
                {
                    let mut model_tabs = world
                        .resource_mut::<crate::ui::panels::model_view::ModelTabs>();
                    model_tabs.ensure(doc);
                }
                world
                    .resource_mut::<lunco_workbench::WorkbenchLayout>()
                    .open_instance(
                        crate::ui::panels::model_view::MODEL_VIEW_KIND,
                        doc.raw(),
                    );
                // Force a fresh projection on the next canvas tick —
                // the doc may have been already open at the package
                // (target=None) level, with a cached zero-node scene.
                world
                    .resource_mut::<crate::ui::state::WorkbenchState>()
                    .diagram_dirty = true;
            }
            // `BrowserAction` is `#[non_exhaustive]` upstream; future
            // variants land as warnings here, not silent drops.
            other => {
                log::warn!("unhandled BrowserAction: {:?}", other);
            }
        }
    }
}
