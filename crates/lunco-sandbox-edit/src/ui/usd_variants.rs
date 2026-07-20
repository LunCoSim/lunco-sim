//! USD **variant** view-model — the variant sets on the selected prim, the
//! options each offers, and which one is currently composed.
//!
//! A variantSet is how one asset ships several configurations: a rover's
//! `drivetrain` (raycast | physical), a scenario scene's `terrain` (which real
//! lunar site it composes with). Selecting one is a first-class journaled op
//! ([`UsdOp::SetVariantSelection`](lunco_usd::document::UsdOp)) — networked,
//! undoable, and replayed from the journal like every other edit — so a picker
//! here is a real authoring control, not a debug toggle.
//!
//! Splitting the read into a producer matches the rest of the Inspector
//! (`usd_params`, `usd_mount`): the composed stage is `!Send`, so it is read on
//! the main thread into a cloneable resource, and the section only paints.
//!
//! ## Where the two halves come from
//!
//! - **Current selection** — [`Prim::variant_sets`] →
//!   `get_all_variant_selections`, which is the *composed* answer and therefore
//!   correct across reference arcs (a wrapper scene that references another and
//!   pins a selection reports the pinned one).
//! - **Available options** — composition can only ever show ONE selection at a
//!   time, so the options cannot be read off the composed stage at all. They
//!   come from the authored layers via
//!   [`lunco_usd_bevy::variants::variant_options_in_stage`], which also
//!   documents why they are keyed by set NAME rather than by prim path.

use bevy::prelude::*;
use lunco_usd_bevy::{CanonicalStages, SdfPath, UsdPrimPath, UsdStageAsset};

/// One variant set on the selected prim.
#[derive(Clone)]
pub struct UsdVariantSet {
    /// Set name, e.g. `terrain` or `drivetrain` — the `variant_set` field of
    /// the op this row dispatches.
    pub name: String,
    /// Currently composed selection, if any resolves.
    pub selection: Option<String>,
    /// Selectable variant names, sorted and deduplicated.
    pub options: Vec<String>,
}

/// Render-ready variant sets for the selected prim. Derived, never
/// authoritative.
#[derive(Resource, Default)]
pub struct UsdVariantView {
    pub entity: Option<Entity>,
    /// USD path of the prim the rows belong to — the op's `path`.
    pub prim_path: String,
    pub sets: Vec<UsdVariantSet>,
}

/// View-model producer: harvest the selected prim's variant sets into
/// [`UsdVariantView`].
pub fn produce_usd_variant_view(
    selected: Option<Res<crate::SelectedEntities>>,
    q: Query<&UsdPrimPath>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    mut view: ResMut<UsdVariantView>,
) {
    let entity = selected.as_deref().and_then(|s| s.primary());
    view.entity = entity;
    view.sets.clear();
    view.prim_path.clear();

    let Some(entity) = entity else {
        return;
    };
    let Ok(prim) = q.get(entity) else {
        return;
    };
    view.prim_path = prim.path.clone();

    let stage_id = prim.stage_handle.id();
    if canonical.get(stage_id).is_none() {
        if let Some(recipe) = stages.get(&prim.stage_handle).and_then(|a| a.recipe.clone()) {
            canonical.get_or_build(stage_id, &recipe);
        }
    }
    let Some(cs) = canonical.get(stage_id) else {
        return;
    };
    let Ok(sdf) = SdfPath::new(&prim.path) else {
        return;
    };

    let stage = cs.stage();
    // Composed selections: authoritative, and reference-arc correct.
    let selections = match stage.prim(sdf.as_str()).variant_sets().get_all_variant_selections() {
        Ok(s) => s,
        Err(_) => return,
    };
    if selections.is_empty() {
        return;
    }

    let options_by_set = lunco_usd_bevy::variants::variant_options_in_stage(stage);
    for (name, selection) in selections {
        let mut options = options_by_set.get(&name).cloned().unwrap_or_default();
        // The composed selection is selectable even if no layer spelled it out
        // (a fallback, or a variant whose block authors nothing) — otherwise
        // the picker could show a state it cannot return to.
        if !selection.is_empty() && !options.iter().any(|o| *o == selection) {
            options.push(selection.clone());
            options.sort();
        }
        view.sets.push(UsdVariantSet {
            name,
            selection: (!selection.is_empty()).then_some(selection),
            options,
        });
    }
    view.sets.sort_by(|a, b| a.name.cmp(&b.name));
}

