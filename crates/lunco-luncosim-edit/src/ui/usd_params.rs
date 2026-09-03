//! USD **parameter** view-model — bounded sliders for attributes that author a
//! `customData { min, max, unit }` UI hint.
//!
//! The Inspector's other parameter sections read fixed, hand-coded ranges from
//! ECS components. This one is data-driven: any scalar attribute on the selected
//! prim that authors a `customData` range shows up as a slider clamped to it —
//! so an asset (`float primvars:spoke_count = 6 (customData = {double min=3;
//! double max=12})`) declares its own editing bounds and the UI derives the
//! control, per [`feedback_inspector_derives_params_not_hardcoded`].
//!
//! The producer runs on the main thread (the composed stage is `!Send`) and
//! harvests each open preview's selected prim into its session entry in [`UsdParamView`]; the
//! Inspector section (`inspector::usd_parameters_section`) renders them and
//! writes edits back through the same `ApplyUsdOp(SetAttribute)` path.

use bevy::prelude::*;
use std::collections::HashMap;

use lunco_usd::ui::viewport::{UsdPreviewId, UsdViewportState};
use lunco_usd_bevy::{CanonicalStages, SdfPath, UsdPrimPath, UsdRead, UsdStageAsset};

/// One ranged parameter derived from an attribute's `customData`.
#[derive(Clone)]
pub struct UsdParam {
    /// Full attribute name (e.g. `primvars:spoke_count`) — the write-back target.
    pub name: String,
    /// Display label (the leaf after the last `:`).
    pub label: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    /// Optional unit suffix from `customData.unit`.
    pub unit: String,
    /// Value type for the write-back `SetAttribute` (`customData.type`, default
    /// `"float"`).
    pub type_name: String,
}

/// Render-ready ranged parameters for the selected prim. Derived, never
/// authoritative.
#[derive(Clone, Default)]
pub struct UsdParamSessionView {
    pub preview: UsdPreviewId,
    pub doc: lunco_doc::DocumentId,
    pub edit_target: lunco_usd::document::LayerId,
    pub generation: u64,
    pub entity: Option<Entity>,
    pub path: String,
    pub params: Vec<UsdParam>,
}

/// Session-keyed parameter views. The Inspector selects the focused entry for
/// painting, while open previews retain their own derived state.
#[derive(Resource, Default)]
pub struct UsdParamView {
    sessions: HashMap<UsdPreviewId, UsdParamSessionView>,
}

impl UsdParamView {
    pub(crate) fn focused(&self, viewport: &UsdViewportState) -> Option<&UsdParamSessionView> {
        viewport
            .focused_preview_id()
            .and_then(|preview| self.sessions.get(&preview))
    }
}

/// View-model producer: harvest the selected prim's `customData`-ranged
/// attributes into [`UsdParamView`].
pub fn produce_usd_param_view(
    selected: Option<Res<lunco_scene_commands::SelectedEntities>>,
    target: Option<Res<crate::InspectorTarget>>,
    q: Query<&UsdPrimPath>,
    q_parents: Query<&ChildOf>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    viewport: Option<Res<UsdViewportState>>,
    mut view: ResMut<UsdParamView>,
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
                .or_insert_with(|| UsdParamSessionView {
                    preview: session.id(),
                    doc: session.doc(),
                    edit_target: session.edit_target().clone(),
                    generation: 0,
                    entity: None,
                    path: String::new(),
                    params: Vec::new(),
                });
        session_view.preview = session.id();
        session_view.doc = session.doc();
        session_view.edit_target = session.edit_target().clone();
        session_view.generation = session.projected_generation();
        session_view.entity = None;
        session_view.path.clear();
        session_view.params.clear();
        if !session.projection_ready() {
            continue;
        }

        // A drilled prim-backed subpart wins over the primary: Alt+Shift+click
        // a wheel of the selected rover and this session edits the wheel's own
        // attrs. A raw mesh drill falls back to the session's primary prim.
        let Some(entity) = crate::ui::selected_entity_in_preview(
            session,
            selected.as_deref(),
            target.as_deref(),
            &q,
            &q_parents,
        ) else {
            continue;
        };
        let Ok(prim) = q.get(entity) else {
            continue;
        };
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
        let stage_view = cs.view();
        let Ok(sdf) = SdfPath::new(&prim.path) else {
            continue;
        };
        session_view.entity = Some(entity);
        session_view.path = prim.path.clone();

        for attr in stage_view.attr_names(&sdf) {
            // Per-asset authored customData wins; the schema's declared hint is
            // the shared fallback for standard LunCo properties.
            let Some(hint) = stage_view
                .attr_ui_hint(&sdf, &attr)
                .or_else(|| lunco_usd::schema::ui_hint_of(&attr))
            else {
                continue;
            };
            let (Some(min), Some(max)) = (hint.min, hint.max) else {
                continue;
            };
            if max <= min {
                continue;
            }
            let value = stage_view.real(&sdf, &attr).unwrap_or(min).clamp(min, max);
            let unit = hint.unit.unwrap_or_default();
            let type_name = hint
                .type_name
                .or_else(|| {
                    lunco_usd::schema::SchemaRegistry::global()
                        .read()
                        .ok()
                        .and_then(|r| r.property(&attr).map(|p| p.type_name.clone()))
                })
                .unwrap_or_else(|| "float".to_string());
            let label = attr.rsplit(':').next().unwrap_or(&attr).to_string();
            session_view.params.push(UsdParam {
                name: attr,
                label,
                value,
                min,
                max,
                unit,
                type_name,
            });
        }
        session_view.params.sort_by(|a, b| a.label.cmp(&b.label));
    }
}
