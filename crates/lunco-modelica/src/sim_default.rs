//! Default-simulation-class resolution + the per-document run-target
//! override registry.
//!
//! Egui-free; lifted out of `ui::panels::model_view::context` so the
//! headless/server build (and core read-side helpers like
//! `state::detected_name_for`) can resolve "which class does a
//! simulation surface default to" without the egui `ui` module.

use bevy::prelude::*;
use std::collections::HashMap;
use lunco_doc::DocumentId;
use crate::state::ModelicaDocumentRegistry;
use crate::model_tabs_types::TabRenderContext;
use crate::model_tabs::ModelTabs;

/// Read-only resource access abstracted over the three contexts that resolve
/// run-target precedence: the headless / world side (`&World`) and the two
/// capability-narrowed paint contexts (`PanelCtx`, `BrowserCtx`). The
/// precedence resolvers below are written once against this trait instead of
/// as hand-synced `_world` / `_ctx` / `_bctx` copies — otherwise the
/// divergence hazard is structural: a precedence change applied to one copy
/// and not the others silently makes surfaces disagree on which class to run.
pub trait ResourceRead {
    /// Read a resource, `None` if absent. O(1) hash lookup, never a scan.
    fn read_resource<T: Resource>(&self) -> Option<&T>;
}

impl ResourceRead for World {
    fn read_resource<T: Resource>(&self) -> Option<&T> {
        self.get_resource::<T>()
    }
}

#[cfg(feature = "ui")]
impl ResourceRead for lunco_workbench::PanelCtx<'_> {
    fn read_resource<T: Resource>(&self) -> Option<&T> {
        self.resource::<T>()
    }
}

#[cfg(feature = "ui")]
impl ResourceRead for lunco_workbench::BrowserCtx<'_, '_> {
    fn read_resource<T: Resource>(&self) -> Option<&T> {
        self.resource::<T>()
    }
}

/// Drilled-in class for `doc`, honouring the active tab's render context when
/// it targets the same doc, else the per-doc drill pin. Generic over the
/// resource-read context — see [`ResourceRead`].
pub fn drilled_class_for_doc_in<R: ResourceRead>(ctx: &R, doc: DocumentId) -> Option<String> {
    if let Some(tc) = ctx.read_resource::<TabRenderContext>() {
        if let Some(tab_id) = tc.tab_id {
            if let Some(tabs) = ctx.read_resource::<ModelTabs>() {
                if let Some(state) = tabs.get(tab_id) {
                    if state.doc == doc {
                        return state.drilled_class.clone();
                    }
                }
            }
        }
    }
    ctx.read_resource::<ModelTabs>()
        .and_then(|t| t.drilled_class_for_doc(doc))
}

/// The class a doc's simulation surfaces default to, in precedence order:
///   1. run-target override (the Experiments / Fast Run class dropdown) — but
///      ONLY when it still names a real candidate; a stale override left over
///      from before a source edit must not silently run a missing class. It is
///      deliberately separate from the drill pin so choosing a run target
///      never moves the canvas view.
///   2. drilled-in class — the UI drill-in pin; the user is looking at a
///      leaf model and expects *that* to run, not the enclosing package.
///   3. tier-ranked simulation root — `simulation_candidates()[0]`, where an
///      `experiment(...)`-annotated, non-partial class sorts first. This is
///      NOT arbitrary `HashMap` order: a package whose only annotated model
///      is `RoverThermalSystem` must not default to e.g. `LunarEnvironment`.
/// Returns `None` when the doc has no host or no simulatable candidate.
///
/// Single source of truth for "which class does a simulation surface default
/// to" — the Fast Run popup, the Experiments Setup form, and (for the
/// non-ambiguous path) `dispatch_experiment` all route through here so the
/// precedence can't drift between surfaces. Generic over the resource-read
/// context — see [`ResourceRead`]. Callers that need to disambiguate multiple
/// candidates (e.g. open a picker modal) layer that on top.
pub fn default_simulation_class_in<R: ResourceRead>(ctx: &R, doc: DocumentId) -> Option<String> {
    let candidates = ctx
        .read_resource::<ModelicaDocumentRegistry>()
        .and_then(|r| r.host(doc))
        .map(|h| h.document().index().simulation_candidates())
        .unwrap_or_default();
    let override_cls = ctx
        .read_resource::<RunTargetOverrides>()
        .and_then(|o| o.0.get(&doc).cloned())
        .filter(|c| candidates.iter().any(|x| x == c));
    if let Some(c) = override_cls {
        return Some(c);
    }
    let drilled = drilled_class_for_doc_in(ctx, doc);
    crate::sim_target::default_class(drilled.as_deref(), &candidates)
}

/// `&World` resolver for the drilled-in class — see [`drilled_class_for_doc_in`].
pub fn drilled_class_for_doc(world: &World, doc: DocumentId) -> Option<String> {
    drilled_class_for_doc_in(world, doc)
}

/// `&World` resolver for the default simulation class — see
/// [`default_simulation_class_in`].
pub fn default_simulation_class(world: &World, doc: DocumentId) -> Option<String> {
    default_simulation_class_in(world, doc)
}

/// `PanelCtx` sibling of [`drilled_class_for_doc`], reading resources through
/// the capability-narrowed panel context so ported panels can resolve the
/// drilled class during paint without `&World`.
#[cfg(feature = "ui")]
pub fn drilled_class_for_doc_ctx(
    ctx: &lunco_workbench::PanelCtx,
    doc: DocumentId,
) -> Option<String> {
    drilled_class_for_doc_in(ctx, doc)
}

/// `PanelCtx` sibling of [`default_simulation_class`].
#[cfg(feature = "ui")]
pub fn default_simulation_class_ctx(
    ctx: &lunco_workbench::PanelCtx,
    doc: DocumentId,
) -> Option<String> {
    default_simulation_class_in(ctx, doc)
}

/// Per-document explicit run target, set by the class dropdowns on the
/// Experiments Setup form and the Fast Run modal. Read by
/// [`default_simulation_class`] with top precedence. Kept separate from the
/// drill pin ([`ModelTabs`] `drilled_class`) so picking what to *run* does
/// not change what the canvas *shows*.
#[derive(Resource, Default)]
pub struct RunTargetOverrides(pub HashMap<DocumentId, String>);

/// Pin `class` as the explicit run target for `doc`. Every run surface
/// (Experiments Setup, Fast Run modal, `dispatch_experiment`) re-resolves to
/// it via [`default_simulation_class`]; the canvas drill view is untouched.
pub fn set_run_target_for_doc(world: &mut World, doc: DocumentId, class: &str) {
    if let Some(mut targets) = world.get_resource_mut::<RunTargetOverrides>() {
        targets.0.insert(doc, class.to_string());
    }
}
