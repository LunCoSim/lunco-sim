//! Which document backs this entity, and where does its look live — the two
//! questions every authoring path has to answer before it can write a USD op.
//!
//! Both helpers used to sit in `ui::inspector` because a panel happened to be the
//! first caller. That made them `ui`-only, while `commands.rs` — declared
//! headless-safe, and the module a `--no-ui` server depends on for
//! `SpawnCommandPlugin` — reached into `crate::ui::inspector` for them anyway. The
//! server build therefore did not compile at all (`cannot find `ui` in `crate``).
//! Neither function is UI: one matches a stage asset to its open document, the other
//! walks `material:binding` on the composed stage. They belong here, where the
//! command layer and the Inspector can share them.

use bevy::prelude::*;
use lunco_doc::DocumentOrigin;
use lunco_usd::registry::UsdDocumentRegistry;
use lunco_usd_bevy::{resolve_bound_shader, SdfPath, UsdPrimPath};

/// The `UsdPreviewSurface` Shader prim bound to `prim`'s geometry, or `None` when it
/// has no material yet.
///
/// Walks `material:binding` → the Material's `outputs:surface` connection → the
/// Shader, on the LIVE canonical stage (building it from the asset's recipe if it has
/// not been built yet). Shared, because the two places that edit a look — the
/// Inspector panel and the `SetObjectProperty` command — must agree on WHERE the look
/// lives, or one of them will scribble `inputs:*` somewhere no other DCC reads it
/// back from.
pub(crate) fn bound_shader_prim(world: &mut World, prim: &UsdPrimPath) -> Option<String> {
    let id = prim.stage_handle.id();
    let mesh_sdf = SdfPath::new(&prim.path).ok()?;

    let recipe = world
        .get_resource::<Assets<lunco_usd_bevy::UsdStageAsset>>()
        .and_then(|stages| stages.get(&prim.stage_handle))
        .and_then(|a| a.recipe.clone());
    if let Some(mut canonical) = world.get_non_send_mut::<lunco_usd_bevy::CanonicalStages>() {
        if canonical.get(id).is_none() {
            if let Some(r) = recipe.as_ref() {
                canonical.get_or_build(id, r);
            }
        }
    }
    let canonical = world.get_non_send::<lunco_usd_bevy::CanonicalStages>()?;
    let view = canonical.get(id)?.view();
    resolve_bound_shader(&view, &mesh_sdf).map(|p| p.to_string())
}

/// Resolve the editable USD document backing `entity`'s stage — the same
/// asset↔document match `apply_usd_path_attribute_change` needs, factored out so a
/// caller authoring a *sequence* of ops (the mount snap) resolves the doc once and
/// dispatches every op to it.
///
/// Falls back to the viewport's active doc, which is a GUI notion — a headless server
/// has no viewport, so there the registry match is the only answer (and the honest
/// one: with no open viewport there is no "active" document to mean).
pub(crate) fn resolve_doc_for_entity(world: &World, entity: Entity) -> Option<lunco_doc::DocumentId> {
    let prim = world.get::<UsdPrimPath>(entity)?;
    let asset_server = world.get_resource::<AssetServer>()?;
    let asset_path = asset_server.get_path(prim.stage_handle.id())?;
    let path_str = asset_path.path().to_string_lossy().to_string();

    let doc_id = world.get_resource::<UsdDocumentRegistry>().and_then(|reg| {
        reg.ids().find(|id| {
            reg.host(*id).is_some_and(|h| match h.document().origin() {
                DocumentOrigin::File { path, .. } => path.to_string_lossy().ends_with(&path_str),
                _ => false,
            })
        })
    });

    #[cfg(feature = "ui")]
    let doc_id = doc_id.or_else(|| {
        world
            .get_resource::<lunco_usd::ui::viewport::UsdViewportState>()
            .and_then(|v| v.active_doc())
    });

    doc_id
}
