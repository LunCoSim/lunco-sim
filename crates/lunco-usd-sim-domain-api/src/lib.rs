//! API query providers for the USD Modelica domain projection.
//!
//! The domain runtime publishes generated-network facts; this package owns the
//! optional typed API read surface for those facts so API serialization changes do
//! not rebuild the projection engine.

use bevy::prelude::*;
use lunco_api::{ApiQueryError, ApiQueryProvider, ApiQueryRegistry, ApiQueryResult, api_param_str};
use lunco_api_core::{ApiErrorCode, ApiValue, api_value};
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

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let wanted = params
            .get("network_root")
            .map(|_| {
                api_param_str(params, "network_root")
                    .map(str::to_string)
                    .ok_or_else(|| {
                        ApiQueryError::new(
                            ApiErrorCode::DeserializationError,
                            "GeneratedModelicaSource: `network_root` must be a string",
                        )
                    })
            })
            .transpose()?;
        let Some(mut query) = bevy::ecs::query::QueryState::<(
            &GeneratedModelicaSource,
            Option<&ModelicaModel>,
        )>::try_new(world) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "GeneratedModelicaSource: ECS query is unavailable",
            ));
        };
        let networks: Vec<ApiValue> = query
            .iter(world)
            .filter(|(generated, _)| {
                wanted
                    .as_deref()
                    .is_none_or(|root| root == generated.network_root)
            })
            .map(|(generated, model)| {
                api_value!({
                    "network_root": generated.network_root.clone(),
                    "model_name": model.map(|model| model.model_name.clone()).unwrap_or_default(),
                    "doc_uri": generated.doc_uri.clone(),
                    "projection_error": generated.projection_error.clone(),
                    "boundary_inputs": generated.boundary_inputs.clone(),
                    "boundary_outputs": generated.boundary_outputs.clone(),
                    "member_output_aliases": generated.member_output_aliases.iter().map(|(member, output, alias)| {
                        api_value!([member.clone(), output.clone(), alias.clone()])
                    }).collect::<Vec<_>>(),
                    "components": generated.component_paths.clone(),
                    "members": generated
                        .members
                        .iter()
                        .map(|(prim, asset, class)| api_value!({
                            "prim": prim.clone(), "source_asset": asset.clone(), "class": class.clone(),
                        }))
                        .collect::<Vec<_>>(),
                    "source_roots": generated.source_roots.clone(),
                    "units": generated
                        .units
                        .iter()
                        .map(|unit| api_value!({
                            "name": unit.name.clone(),
                            "instance": unit.instance.clone(),
                            "components": unit.component_paths.clone(),
                            "inputs": unit.inputs.iter().cloned().collect::<Vec<_>>(),
                            "outputs": unit.outputs.iter().cloned().collect::<Vec<_>>(),
                        }))
                        .collect::<Vec<_>>(),
                    "layout": {
                        "units": generated
                            .layout
                            .unit_positions
                            .iter()
                            .map(|(name, (x, y))| api_value!({
                                "name": name.clone(), "x": *x, "y": *y,
                            }))
                            .collect::<Vec<_>>(),
                        "members": generated
                            .layout
                            .member_positions
                            .iter()
                            .map(|(path, (x, y))| api_value!({
                                "path": path.clone(), "x": *x, "y": *y,
                            }))
                            .collect::<Vec<_>>(),
                    },
                    "source": generated.source.clone(),
                })
            })
            .collect();
        Ok(Some(api_value!({ "networks": networks })))
    }
}
