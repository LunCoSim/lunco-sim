//! API query providers for the rendered USD viewport.
//!
//! These queries describe an application presentation surface. They are
//! installed by the viewport UI package, not by the render/session runtime,
//! so headless preview consumers do not inherit the API query registry.

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::{ApiQueryError, ApiQueryResult};
use lunco_api_core::{ApiErrorCode, ApiValue, api_value};
use lunco_usd_bevy_stage::UsdStageAsset;
use lunco_usd_viewport_core::{UsdInspectionSettings, UsdViewportState};

/// Read-only query for the exact USD presentation state currently open in the
/// Assembly Editor.
///
/// Unlike document inspection, this query is session-scoped: it reports every
/// explicit preview lease and its independent presentation views, plus the
/// focused preview/view pair. It never infers a document from a tab title, a
/// filesystem name, or the live simulation scene.
pub(crate) struct InspectUsdViewportProvider;

impl ApiQueryProvider for InspectUsdViewportProvider {
    fn name(&self) -> &'static str {
        "InspectUsdViewport"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let Some(viewport) = world.get_resource::<UsdViewportState>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "InspectUsdViewport requires UsdViewportPlugin",
            ));
        };

        let mut sessions: Vec<_> = viewport.sessions().collect();
        sessions.sort_by_key(|session| session.id().0);
        let previews: Vec<ApiValue> = sessions
            .into_iter()
            .map(|session| {
                let (stage_asset_path, recipe_layers) = world
                    .get_resource::<Assets<UsdStageAsset>>()
                    .and_then(|assets| assets.get(session.stage_handle()))
                    .map(|asset| {
                        let mut layers = asset
                            .recipe
                            .as_ref()
                            .map(|recipe| recipe.bytes.keys().cloned().collect::<Vec<_>>())
                            .unwrap_or_default();
                        layers.sort();
                        let path = world
                            .get_resource::<AssetServer>()
                            .and_then(|server| server.get_path(session.stage_handle().id()))
                            .map(|path| path.path().to_string_lossy().into_owned());
                        (path, layers)
                    })
                    .unwrap_or((None, Vec::new()));
                let mut preview_views: Vec<_> = viewport
                    .views()
                    .filter(|view| view.preview() == session.id())
                    .collect();
                preview_views.sort_by_key(|view| view.id().0);
                let views: Vec<ApiValue> = preview_views
                    .into_iter()
                    .map(|view| {
                        api_value!({
                            "view": view.id().0,
                            "focused": viewport.focused_view_id() == Some(view.id()),
                            "mode": view.mode().as_str(),
                            "text_layer": view.text_layer().as_str(),
                            "projection": view.projection().as_str(),
                            "target": api_value!(view.orbit().target.to_array()),
                            "distance": view.orbit().distance,
                            "orthographic_scale": view.orthographic_scale(),
                            "active_preset": view.active_preset.clone(),
                        })
                    })
                    .collect();
                api_value!({
                    "preview": session.id().0,
                    "doc_id": session.doc().raw(),
                    "edit_target": session.edit_target().as_str(),
                    "stage_asset_id": format!("{:?}", session.stage_handle().id()),
                    "stage_asset_path": stage_asset_path,
                    "recipe_layers": recipe_layers,
                    "projected_generation": session.projected_generation(),
                    "projection_ready": session.projection_ready(),
                    "text_ready": session.text_ready(),
                    "explode": session.explode.as_ref().map(|explode| {
                        api_value!({
                            "assembly": explode.assembly.clone(),
                            "parts": explode.parts.iter().map(|part| part.path.clone()).collect::<Vec<_>>(),
                            "axis": explode.axis.as_str(),
                            "spacing": explode.spacing,
                        })
                    }),
                    "focused": viewport.focused_preview_id() == Some(session.id()),
                    "views": views,
                })
            })
            .collect();

        Ok(Some(api_value!({
            "focused_preview": viewport.focused_preview_id().map(|id| id.0),
            "focused_view": viewport.focused_view_id().map(|id| id.0),
            "previews": previews,
            "preview_count": viewport.session_count(),
            "view_count": viewport.view_count(),
        })))
    }
}

/// Read-only query for the persisted USD inspection preset names.
pub(crate) struct InspectUsdInspectionPresetsProvider;

impl ApiQueryProvider for InspectUsdInspectionPresetsProvider {
    fn name(&self) -> &'static str {
        "InspectUsdInspectionPresets"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let presets = world
            .get_resource::<UsdInspectionSettings>()
            .map(|settings| {
                settings
                    .presets
                    .iter()
                    .map(|preset| {
                        api_value!({
                            "name": preset.name.clone(),
                            "projection": preset.projection.as_str(),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(Some(api_value!({ "presets": presets })))
    }
}

pub(crate) fn register_api_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let mut registry = app.world_mut().resource_mut::<ApiQueryRegistry>();
    registry.register(InspectUsdViewportProvider);
    registry.register(InspectUsdInspectionPresetsProvider);
}
