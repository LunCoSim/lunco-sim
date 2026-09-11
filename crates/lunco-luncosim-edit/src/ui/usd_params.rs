//! USD **parameter** view-model — schema-hinted fields for attributes that author a
//! `customData { min, max, unit }` UI hint.
//!
//! The Inspector's other parameter sections read fixed, hand-coded ranges from
//! ECS components. This one is data-driven: any scalar attribute on the selected
//! prim that authors a `customData` range shows up as an editor field with the
//! declared bounds —
//! so an asset (`float primvars:spoke_count = 6 (customData = {double min=3;
//! double max=12})`) declares its own editing bounds and the UI derives the
//! control, per [`feedback_inspector_derives_params_not_hardcoded`].
//!
//! The producer runs on the main thread (the composed stage is `!Send`) and
//! harvests each open preview's selected prim into its session entry in
//! [`UsdParamView`]. The Inspector section (`inspector::usd_parameters_section`)
//! keeps edits as a session-local draft and submits them through the typed USD
//! proposal/review path used by the Assembly Editor and AI callers.

use bevy::prelude::*;
use std::collections::HashMap;

use lunco_usd_bevy::SdfPath;
use lunco_usd_bevy_core::{CanonicalStages, UsdRead, UsdStageAsset};
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_usd_ui::viewport::{UsdPreviewId, UsdViewportState};

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
    /// Whether the value has an authored opinion in any contributing layer.
    pub authored: bool,
    /// Whether the composed value is finite and within its declared bounds.
    /// Invalid values stay visible and are not submitted until corrected.
    pub valid: bool,
    /// Whether the composed value is a finite number that can be corrected in
    /// the Inspector even when it is outside the declared bounds.
    pub numeric: bool,
    /// Why the composed value cannot be edited, when [`Self::valid`] is false.
    pub diagnostic: Option<String>,
}

/// Render-ready ranged parameters for the selected prim. Derived, never
/// authoritative.
#[derive(Clone, Default)]
pub struct UsdParamSessionView {
    pub preview: UsdPreviewId,
    pub doc: lunco_doc::DocumentId,
    pub edit_target: lunco_usd_core::document::LayerId,
    pub generation: u64,
    pub entity: Option<Entity>,
    pub path: String,
    pub kind: Option<String>,
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

/// Draft values for one selected prim. Drafts are editor state only: they are
/// never projected into the stage until the Inspector submits one compound
/// USD change set.
pub(crate) struct UsdParamDraft {
    pub generation: u64,
    pub values: HashMap<String, f64>,
    pub scope: lunco_usd_core::edit_session::UsdEditScope,
}

impl Default for UsdParamDraft {
    fn default() -> Self {
        Self {
            generation: 0,
            values: HashMap::new(),
            scope: lunco_usd_core::edit_session::UsdEditScope::Assembly,
        }
    }
}

/// Session/path keyed parameter drafts. The composed stage remains the only
/// authoritative value source while a user edits several fields together.
#[derive(Resource, Default)]
pub(crate) struct UsdParamDrafts {
    pub entries: HashMap<(UsdPreviewId, String), UsdParamDraft>,
}

/// Classify a composed numeric value without changing it. The Inspector may
/// show a value outside its declared range as invalid, but must not hide the
/// source problem by clamping it to a valid-looking number.
fn classify_real(raw: Option<f64>, min: f64, max: f64) -> (f64, bool, bool, Option<String>) {
    match raw {
        Some(value) if value.is_finite() && value >= min && value <= max => {
            (value, true, true, None)
        }
        Some(value) if value.is_finite() => (
            value,
            false,
            true,
            Some(format!("outside declared range [{min}, {max}]")),
        ),
        Some(_) => (min, false, false, Some("value is not finite".to_string())),
        None => (
            min,
            false,
            false,
            Some("composed value is not numeric".to_string()),
        ),
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
                    kind: None,
                    params: Vec::new(),
                });
        session_view.preview = session.id();
        session_view.doc = session.doc();
        session_view.edit_target = session.edit_target().clone();
        session_view.generation = session.projected_generation();
        session_view.entity = None;
        session_view.path.clear();
        session_view.kind = None;
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
        session_view.kind = stage_view.kind(&sdf);

        for attr in stage_view.attr_names(&sdf) {
            // Per-asset authored customData wins; the schema's declared hint is
            // the shared fallback for standard LunCo properties.
            let Some(hint) = stage_view
                .attr_ui_hint(&sdf, &attr)
                .or_else(|| lunco_usd_core::schema::ui_hint_of(&attr))
            else {
                continue;
            };
            let (Some(min), Some(max)) = (hint.min, hint.max) else {
                continue;
            };
            if !min.is_finite() || !max.is_finite() || max <= min {
                continue;
            }
            let (value, valid, numeric, diagnostic) =
                classify_real(stage_view.real(&sdf, &attr), min, max);
            let unit = hint.unit.unwrap_or_default();
            let type_name = hint
                .type_name
                .or_else(|| {
                    lunco_usd_core::schema::SchemaRegistry::global()
                        .read()
                        .ok()
                        .and_then(|r| r.property(&attr).map(|p| p.type_name.clone()))
                })
                .unwrap_or_else(|| "float".to_string());
            let label = attr.rsplit(':').next().unwrap_or(&attr).to_string();
            session_view.params.push(UsdParam {
                name: attr.clone(),
                label,
                value,
                min,
                max,
                unit,
                type_name,
                authored: stage_view.has_authored_attribute(&sdf, &attr),
                valid,
                numeric,
                diagnostic,
            });
        }
        session_view.params.sort_by(|a, b| a.label.cmp(&b.label));
    }
}

#[cfg(test)]
mod tests {
    use super::classify_real;

    #[test]
    fn invalid_composed_values_are_reported_without_clamping() {
        let (value, valid, numeric, diagnostic) = classify_real(Some(15.0), 0.0, 10.0);

        assert_eq!(value, 15.0);
        assert!(!valid);
        assert!(numeric);
        assert_eq!(
            diagnostic.as_deref(),
            Some("outside declared range [0, 10]")
        );
    }

    #[test]
    fn missing_composed_values_are_not_editable() {
        let (value, valid, numeric, diagnostic) = classify_real(None, 0.0, 10.0);

        assert_eq!(value, 0.0);
        assert!(!valid);
        assert!(!numeric);
        assert_eq!(diagnostic.as_deref(), Some("composed value is not numeric"));
    }
}
