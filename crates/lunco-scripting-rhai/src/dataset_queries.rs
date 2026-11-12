//! API/script read surface for the generic dataset registry.
//!
//! Download ownership stays in `lunco-assets`; declarations and state stay in
//! `lunco-assets-datasets`; this module only adapts the authoritative dataset
//! state to the existing language-neutral query bridge.

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::{ApiQueryError, ApiQueryResult};
use lunco_api_core::{ApiErrorCode, ApiValue, api_value};

/// `ListDatasets` — list declared engine and Twin datasets without exposing
/// machine-local paths. Use each returned `id` with `RequestDataset`,
/// `ProcessDataset`, or `CancelDataset` according to the entry's `processed`
/// flag and live state.
///
/// params: `{ scope?: string }` where `scope` is `engine` or `twin:<source_identity>`
/// · returns `{ datasets: [{ id, key, group, scope, name, state, processed,
///   recommended, artifact_uri, metadata }] }`
pub struct ListDatasetsProvider;

impl ApiQueryProvider for ListDatasetsProvider {
    fn name(&self) -> &'static str {
        "ListDatasets"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let filter = match params.get("scope") {
            None | Some(ApiValue::Unit) => None,
            Some(ApiValue::Str(scope)) => Some(scope.as_str()),
            Some(_) => {
                return Err(ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "ListDatasets: `scope` must be a string",
                ));
            }
        };
        let Some(registry) = world.get_resource::<lunco_assets_datasets::DatasetRegistry>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "ListDatasets: dataset registry is not installed",
            ));
        };
        if let Some(scope) = filter {
            if !registry
                .scanned_scopes()
                .iter()
                .chain(registry.entries().iter().map(|entry| &entry.scope))
                .any(|known| known.id() == scope)
            {
                return Err(ApiQueryError::new(
                    ApiErrorCode::CommandRejected,
                    format!("ListDatasets: scope is not ready or declared: {scope}"),
                ));
            }
        }
        let datasets = registry
            .entries()
            .iter()
            .filter(|entry| filter.is_none_or(|value| entry.scope.id() == value))
            .map(|entry| {
                let artifact_uri = entry.artifact_uri();
                let query_artifact_uri = artifact_uri
                    .clone()
                    .map(ApiValue::str)
                    .unwrap_or(ApiValue::Unit);
                let state = match &entry.state {
                    lunco_assets_datasets::DatasetState::Queued => api_value!({"kind":"queued"}),
                    lunco_assets_datasets::DatasetState::Missing => {
                        api_value!({ "kind": "missing" })
                    }
                    lunco_assets_datasets::DatasetState::Downloading {
                        source,
                        bytes_done,
                        bytes_total,
                    } => api_value!({
                        "kind": "downloading",
                        "source": source.clone(),
                        "bytes_done": *bytes_done,
                        "bytes_total": *bytes_total,
                    }),
                    lunco_assets_datasets::DatasetState::Processing { kind } => {
                        api_value!({ "kind": "processing", "process": kind.clone() })
                    }
                    lunco_assets_datasets::DatasetState::Cancelling => {
                        api_value!({ "kind": "cancelling" })
                    }
                    lunco_assets_datasets::DatasetState::Installed => {
                        api_value!({ "kind": "installed" })
                    }
                    lunco_assets_datasets::DatasetState::Cancelled => {
                        api_value!({ "kind": "cancelled" })
                    }
                    lunco_assets_datasets::DatasetState::Failed(error) => {
                        api_value!({ "kind": "failed", "error": error.clone() })
                    }
                };
                let metadata = lunco_api_core::api_value_from_serializable(&entry.spec.extra)?;
                let inputs:Vec<_>=entry.inputs.iter().map(|input|api_value!({"id":input.id.clone(),"name":input.name.clone(),"url":input.url.clone(),"state":input.state.kind(),"error":input.state.error()})).collect();
                let recipe = match entry.spec.process.as_ref().and_then(|process|process.pipeline.as_ref()) {
                    Some(pipeline) => {
                        let steps:Vec<_> = pipeline.steps.iter().map(|step| api_value!({
                            "id":step.id.clone(), "kind":step.kind.clone(), "input":step.input.clone(),
                        })).collect();
                        let artifacts:Vec<_> = pipeline.artifacts.iter().map(|artifact| {
                            let dimensions = artifact.dimensions.map(|dimensions|api_value!([dimensions[0],dimensions[1]])).unwrap_or(ApiValue::Unit);
                            let uri = artifact_uri.as_ref().map(|base| ApiValue::str(format!("{}/{}",base,artifact.path))).unwrap_or(ApiValue::Unit);
                            api_value!({"uri":uri,"kind":artifact.kind.clone(),"dimensions":dimensions})
                        }).collect();
                        api_value!({"steps":steps,"artifacts":artifacts})
                    }
                    None => ApiValue::Unit,
                };
                Ok(api_value!({
                    "id": entry.id.clone(),
                    "key": entry.key.clone(),
                    "group": entry.group.clone(),
                    "scope": entry.scope.id(),
                    "name": entry.name.clone(),
                    "state": state,
                    "processed": entry.spec.process.is_some(),
                    "recommended": entry.recommended,
                    "required_for_scene": world.get_resource::<lunco_assets_datasets::SceneDatasetRequirements>()
                        .is_some_and(|requirements| requirements.ids.contains(&entry.id)),
                    "artifact_uri": query_artifact_uri,
                    "metadata": metadata,
                    "recipe": recipe,
                    "inputs": inputs,
                    "inputs_ready": entry.inputs_ready(),
                }))
            })
            .collect::<Result<Vec<_>, ApiQueryError>>()?;
        Ok(Some(api_value!({ "datasets": datasets })))
    }
}

/// Register the dataset read surface beside the other script/API queries.
pub fn register_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(ListDatasetsProvider);
}
