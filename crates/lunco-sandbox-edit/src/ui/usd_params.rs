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
//! harvests the selected prim's ranged attributes into [`UsdParamView`]; the
//! Inspector section (`inspector::usd_parameters_section`) renders them and
//! writes edits back through the same `ApplyUsdOp(SetAttribute)` path.

use bevy::prelude::*;
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
#[derive(Resource, Default)]
pub struct UsdParamView {
    pub entity: Option<Entity>,
    pub params: Vec<UsdParam>,
}

/// View-model producer: harvest the selected prim's `customData`-ranged
/// attributes into [`UsdParamView`].
pub fn produce_usd_param_view(
    selected: Option<Res<crate::SelectedEntities>>,
    target: Option<Res<crate::InspectorTarget>>,
    q: Query<&UsdPrimPath>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    mut view: ResMut<UsdParamView>,
) {
    // A DRILLED prim-backed subpart wins over the primary: Alt+Shift+click a
    // wheel of the selected rover and the section edits the WHEEL's own attrs
    // (`lunco:wheel:*`), addressed at its own prim path. A part that carries no
    // `UsdPrimPath` (raw mesh drill for material editing) falls back to the
    // primary's params.
    let entity = target
        .as_deref()
        .and_then(|t| t.part)
        .filter(|p| q.get(*p).is_ok())
        .or_else(|| selected.as_deref().and_then(|s| s.primary()));
    view.entity = entity;
    view.params.clear();

    let Some(entity) = entity else {
        return;
    };
    let Ok(prim) = q.get(entity) else {
        return;
    };
    let stage_id = prim.stage_handle.id();
    if canonical.get(stage_id).is_none() {
        if let Some(recipe) = stages.get(&prim.stage_handle).and_then(|a| a.recipe.clone()) {
            canonical.get_or_build(stage_id, &recipe);
        }
    }
    let Some(cs) = canonical.get(stage_id) else {
        return;
    };
    let stage_view = cs.view();
    let Ok(sdf) = SdfPath::new(&prim.path) else {
        return;
    };

    for attr in stage_view.attr_names(&sdf) {
        // Per-asset authored `customData` wins; the SCHEMA's declared hint is
        // the fallback — so an attribute whose schema authors bounds (all of
        // `LunCoWheelAPI`/`LunCoSuspensionAPI`, the physxVehicle attrs we
        // read) gets a slider on EVERY asset with zero per-asset authoring.
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
        // Write-back type: the hint's `type` field, else the schema's declared
        // type for the attribute, else the historical `float` guess.
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
        view.params.push(UsdParam {
            name: attr,
            label,
            value,
            min,
            max,
            unit,
            type_name,
        });
    }
    view.params.sort_by(|a, b| a.label.cmp(&b.label));
}
