//! Drains [`lunco_workbench::BrowserActions`] and routes each into the
//! appropriate Modelica subsystem.
//!
//! Sections push abstract intents (`OpenFile`, `OpenModelicaClass`)
//! during render; this system picks them up the same frame and turns
//! them into [`ClassRef`](crate::class_ref::ClassRef) values dispatched
//! to [`crate::ui::panels::package_browser::open_class`]. The drill-in
//! target rides directly on the `ClassRef`, so there is no out-of-band
//! queue to synchronise with the async source load — `ensure_for(doc,
//! drilled)` writes the tab state up front and the load task fills in
//! the source when it lands.

use bevy::prelude::*;
use lunco_workbench::{BrowserAction, BrowserActions};

/// Drain the Twin Browser action outbox each frame and dispatch.
///
/// Runs in `Update` after the panel render so actions queued during
/// the egui pass are picked up the same frame they were emitted.
pub fn drain_browser_actions(world: &mut World) {
    // Pull actions out of the world so we can mutate other resources
    // (DocumentRegistry, WorkbenchState, …) freely while iterating.
    // Take only actions this crate owns. `OpenFile` is partitioned by
    // file extension so domain crates (USD, future SysML, …) can
    // dispatch their own filetypes via sibling systems without each
    // one having to know about every other domain's extensions.
    let actions: Vec<BrowserAction> = {
        let mut outbox = world.resource_mut::<BrowserActions>();
        outbox.take_where(|a| match a {
            BrowserAction::OpenFile { relative_path } => relative_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("mo"))
                .unwrap_or(false),
            BrowserAction::OpenModelicaClass { .. }
            | BrowserAction::OpenLoadedClass { .. }
            | BrowserAction::CloseDoc { .. } => true,
            _ => false,
        })
    };
    if actions.is_empty() {
        return;
    }

    // Resolve `relative_path` → absolute path against the currently-
    // active Twin's root. Captured once so we don't fight the borrow
    // checker re-borrowing `WorkspaceResource` per action.
    let twin_root = {
        let ws = world.resource::<lunco_workspace::WorkspaceResource>();
        ws.active_twin
            .and_then(|id| ws.twin(id))
            .map(|t| t.root.clone())
    };

    for action in actions {
        match action {
            BrowserAction::OpenFile { relative_path } => {
                let Some(root) = twin_root.as_ref() else {
                    log::warn!(
                        "BrowserAction::OpenFile fired with no active Twin: {:?}",
                        relative_path
                    );
                    continue;
                };
                let abs = root.join(&relative_path);
                // Open in a ModelView tab — same path as Modelica
                // classes. For non-`.mo` content the parser produces
                // no classes, so Canvas mode shows empty; the user
                // toggles the 📝 Text mode in the tab toolbar to see
                // raw source. (A future kind-aware default would
                // pre-select Text for non-`.mo`; tracked separately.)
                let class = crate::class_ref::ClassRef::user_file(abs, Vec::<String>::new());
                crate::ui::panels::package_browser::open_class(world, class, false);
            }
            BrowserAction::OpenModelicaClass {
                relative_path,
                qualified_path,
            } => {
                let Some(root) = twin_root.as_ref() else {
                    log::warn!(
                        "BrowserAction::OpenModelicaClass fired with no active Twin: {:?}",
                        relative_path
                    );
                    continue;
                };
                let abs = root.join(&relative_path);
                let qualified_parts: Vec<String> =
                    qualified_path.split('.').map(String::from).collect();
                let class = crate::class_ref::ClassRef::user_file(abs, qualified_parts);
                crate::ui::panels::package_browser::open_class(world, class, false);
            }
            BrowserAction::OpenLoadedClass {
                doc_id,
                qualified_path,
            } => {
                let doc = lunco_doc::DocumentId::new(doc_id);
                // Resolve the doc's "default class" (first
                // non-package top-level class). A click on that
                // class is conceptually the same view as the file
                // tab allocated with `drilled_class = None` by the
                // OpenFile / new-doc paths. `ensure_preview_for_with_default`
                // dedups them so both paths converge on one tab.
                let default_class: Option<String> = world
                    .get_resource::<crate::state::ModelicaDocumentRegistry>()
                    .and_then(|r| r.host(doc))
                    .and_then(|h| {
                        h.document()
                            .index()
                            .classes
                            .values()
                            .find(|c| !matches!(c.kind, crate::index::ClassKind::Package))
                            .map(|c| c.name.clone())
                    });
                let (tab_id, evict) = {
                    let mut model_tabs = world.resource_mut::<crate::model_tabs::ModelTabs>();
                    model_tabs.ensure_preview_for_with_default(
                        doc,
                        Some(qualified_path),
                        default_class.as_deref(),
                    )
                };
                // ensure_preview_for never rebinds TabIds; an evicted
                // previous preview is closed here. Layout mutation goes
                // through CloseTab/OpenTab triggers because WorkbenchLayout
                // is removed from the World for the duration of rendering.
                if let Some(old_id) = evict {
                    world.commands().trigger(lunco_workbench::CloseTab {
                        kind: crate::ui::MODEL_VIEW_KIND,
                        instance: old_id,
                    });
                    world
                        .resource_mut::<crate::model_tabs::ModelTabs>()
                        .close_tab(old_id);
                    if let Some(mut state) = world
                        .get_resource_mut::<crate::ui::panels::canvas_diagram::CanvasDiagramState>(
                    ) {
                        state.drop_tab(old_id);
                    }
                }
                world.commands().trigger(lunco_workbench::OpenTab {
                    kind: crate::ui::MODEL_VIEW_KIND,
                    instance: tab_id,
                });
                // Make this doc the active workspace doc so the
                // canvas (which reads `WorkspaceResource::active_document`
                // to decide what to render) follows the click. Without
                // this, clicking a class in the entity tree only sets
                // `DrilledInClassNames[doc]` — but if the canvas was
                // rendering a different doc, the new drill target is
                // never observed and the diagram looks frozen.
                world
                    .resource_mut::<lunco_workspace::WorkspaceResource>()
                    .active_document = Some(doc);
            }
            BrowserAction::CloseDoc { doc } => {
                use crate::model_tabs::ModelTabs;
                use crate::ui::MODEL_VIEW_KIND;
                // Close every tab bound to the doc — dock layout via
                // CloseTab triggers, ModelTabs state via
                // `close_all_for_doc`, canvas via `drop_tab` — then
                // the document itself. `CloseDocument`'s observers
                // handle registry removal and package-tree cleanup;
                // on wasm the resulting `DocumentClosed` also clears
                // the localStorage autosave entry, so a restored
                // draft stops resurrecting on reload.
                let tab_ids = world.resource_mut::<ModelTabs>().close_all_for_doc(doc);
                for tab in tab_ids {
                    world.commands().trigger(lunco_workbench::CloseTab {
                        kind: MODEL_VIEW_KIND,
                        instance: tab,
                    });
                    if let Some(mut state) = world
                        .get_resource_mut::<crate::ui::panels::canvas_diagram::CanvasDiagramState>(
                    ) {
                        state.drop_tab(tab);
                    }
                }
                world
                    .commands()
                    .trigger(lunco_doc_bevy::CloseDocument { doc });
            }
            // `BrowserAction` is `#[non_exhaustive]` upstream; future
            // variants land as warnings here, not silent drops.
            other => {
                log::warn!("unhandled BrowserAction: {:?}", other);
            }
        }
    }
}
