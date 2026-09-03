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

use std::collections::HashMap;

use bevy::prelude::*;
use lunco_usd::ui::viewport::{UsdPreviewId, UsdViewportState};
use lunco_usd_bevy::{CanonicalStages, SdfPath, UsdPrimPath, UsdStageAsset};

/// One variant set on a preview session's selected prim.
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

/// Render-ready variant sets for each preview session's selected prim.
/// Derived, never authoritative.
#[derive(Clone, Default)]
pub struct UsdVariantSessionView {
    pub preview: UsdPreviewId,
    pub doc: lunco_doc::DocumentId,
    pub edit_target: lunco_usd::document::LayerId,
    pub generation: u64,
    pub entity: Option<Entity>,
    /// USD path of the prim the rows belong to — the op's `path`.
    pub prim_path: String,
    pub sets: Vec<UsdVariantSet>,
}

/// Session-keyed variant views. A focused lease is only a paint choice; it is
/// not allowed to overwrite another lease's derived selection.
#[derive(Resource, Default)]
pub struct UsdVariantView {
    sessions: HashMap<UsdPreviewId, UsdVariantSessionView>,
}

impl UsdVariantView {
    pub(crate) fn focused(&self, viewport: &UsdViewportState) -> Option<&UsdVariantSessionView> {
        viewport
            .focused_preview_id()
            .and_then(|preview| self.sessions.get(&preview))
    }
}

/// View-model producer: harvest the selected prim's variant sets into
/// [`UsdVariantView`].
pub fn produce_usd_variant_view(
    selected: Option<Res<lunco_scene_commands::SelectedEntities>>,
    q: Query<&UsdPrimPath>,
    q_parents: Query<&ChildOf>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    viewport: Option<Res<UsdViewportState>>,
    mut view: ResMut<UsdVariantView>,
) {
    let Some(viewport) = viewport else {
        view.sessions.clear();
        return;
    };
    let open: std::collections::HashSet<_> = viewport.sessions().map(|s| s.id()).collect();
    view.sessions.retain(|preview, _| open.contains(preview));

    for session in viewport.sessions() {
        let session_view =
            view.sessions
                .entry(session.id())
                .or_insert_with(|| UsdVariantSessionView {
                    preview: session.id(),
                    doc: session.doc(),
                    edit_target: session.edit_target().clone(),
                    generation: 0,
                    entity: None,
                    prim_path: String::new(),
                    sets: Vec::new(),
                });
        session_view.preview = session.id();
        session_view.doc = session.doc();
        session_view.edit_target = session.edit_target().clone();
        session_view.generation = session.projected_generation();
        session_view.entity = None;
        session_view.sets.clear();
        session_view.prim_path.clear();
        if !session.projection_ready() {
            continue;
        }

        let Some(entity) = crate::ui::selected_entity_in_preview(
            session,
            selected.as_deref(),
            None,
            &q,
            &q_parents,
        ) else {
            continue;
        };
        let Ok(prim) = q.get(entity) else {
            continue;
        };
        session_view.entity = Some(entity);
        session_view.prim_path = prim.path.clone();

        let stage_id = prim.stage_handle.id();
        if canonical.get(stage_id).is_none() {
            if let Some(recipe) = stages
                .get(&prim.stage_handle)
                .and_then(|a| a.recipe.clone())
            {
                canonical.get_or_build(stage_id, &recipe);
            }
        }
        let Some(cs) = canonical.get(stage_id) else {
            continue;
        };
        let Ok(sdf) = SdfPath::new(&prim.path) else {
            continue;
        };

        let stage = cs.stage();
        let selections = match stage
            .prim(sdf.as_str())
            .variant_sets()
            .get_all_variant_selections()
        {
            Ok(s) => s,
            Err(_) => continue,
        };
        if selections.is_empty() {
            continue;
        }

        let options_by_set = lunco_usd_bevy::variants::variant_options_in_stage(stage);
        for (name, selection) in selections {
            let options = options_by_set.get(&name).cloned().unwrap_or_default();
            session_view.sets.push(UsdVariantSet {
                name,
                selection: (!selection.is_empty()).then_some(selection),
                options,
            });
        }
        session_view.sets.sort_by(|a, b| a.name.cmp(&b.name));
    }
}
