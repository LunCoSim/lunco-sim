//! API query providers for the USD Modelica domain projection.
//!
//! The domain runtime publishes generated-network facts; this package owns the
//! optional JSON/API read surface for those facts so API serialization changes do
//! not rebuild the projection engine.

use bevy::prelude::*;
use lunco_api::{ApiErrorCode, ApiQueryProvider, ApiQueryRegistry, ApiResponse};
use lunco_modelica_runtime::ModelicaModel;
use lunco_usd_sim_domain::GeneratedModelicaSource;

/// Registers the generated-network source query when an API registry is present.
pub struct UsdSimDomainApiPlugin;

impl Plugin for UsdSimDomainApiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, |registry: Option<ResMut<ApiQueryRegistry>>| {
            if let Some(mut registry) = registry {
                registry.register(GeneratedSourceProvider);
            }
        });
    }
}

/// Read back the exact Modelica text emitted for a projected USD network.
pub struct GeneratedSourceProvider;

impl ApiQueryProvider for GeneratedSourceProvider {
    fn name(&self) -> &'static str {
        "GeneratedModelicaSource"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let wanted = params
            .get("network_root")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let Some(mut query) = bevy::ecs::query::QueryState::<(
            &GeneratedModelicaSource,
            Option<&ModelicaModel>,
        )>::try_new(world) else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "GeneratedModelicaSource: ECS query is unavailable",
            );
        };
        let networks: Vec<serde_json::Value> = query
            .iter(world)
            .filter(|(generated, _)| {
                wanted
                    .as_deref()
                    .is_none_or(|root| root == generated.network_root)
            })
            .map(|(generated, model)| {
                serde_json::json!({
                    "network_root": generated.network_root,
                    "model_name": model.map(|model| model.model_name.clone()).unwrap_or_default(),
                    "doc_uri": generated.doc_uri,
                    "projection_error": generated.projection_error,
                    "boundary_inputs": generated.boundary_inputs,
                    "boundary_outputs": generated.boundary_outputs,
                    "member_output_aliases": generated.member_output_aliases,
                    "components": generated.component_paths,
                    "members": generated
                        .members
                        .iter()
                        .map(|(prim, asset, class)| serde_json::json!({
                            "prim": prim, "source_asset": asset, "class": class,
                        }))
                        .collect::<Vec<_>>(),
                    "source_roots": generated.source_roots,
                    "units": generated
                        .units
                        .iter()
                        .map(|unit| serde_json::json!({
                            "name": unit.name,
                            "instance": unit.instance,
                            "components": unit.component_paths,
                            "inputs": unit.inputs,
                            "outputs": unit.outputs,
                        }))
                        .collect::<Vec<_>>(),
                    "layout": {
                        "units": generated
                            .layout
                            .unit_positions
                            .iter()
                            .map(|(name, (x, y))| serde_json::json!({
                                "name": name, "x": x, "y": y,
                            }))
                            .collect::<Vec<_>>(),
                        "members": generated
                            .layout
                            .member_positions
                            .iter()
                            .map(|(path, (x, y))| serde_json::json!({
                                "path": path, "x": x, "y": y,
                            }))
                            .collect::<Vec<_>>(),
                    },
                    "source": generated.source,
                })
            })
            .collect();
        ApiResponse::ok(serde_json::json!({ "networks": networks }))
    }
}
