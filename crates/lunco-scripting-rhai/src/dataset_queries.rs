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
/// machine-local paths. Requests use each returned `id` with `RequestDataset`
/// or `CancelDataset`.
///
/// params: `{ scope?: string }` where `scope` is the engine group or Twin name
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
        let datasets = registry
            .entries()
            .iter()
            .filter(|entry| filter.is_none_or(|value| entry.scope.label() == value))
            .map(|entry| {
                let state = match &entry.state {
                    lunco_assets_datasets::DatasetState::Missing => {
                        api_value!({ "kind": "missing" })
                    }
                    lunco_assets_datasets::DatasetState::Downloading {
                        bytes_done,
                        bytes_total,
                    } => api_value!({
                        "kind": "downloading",
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
                Ok(api_value!({
                    "id": entry.id.clone(),
                    "key": entry.key.clone(),
                    "group": entry.group.clone(),
                    "scope": entry.scope.label(),
                    "name": entry.name.clone(),
                    "state": state,
                    "processed": entry.spec.process.is_some(),
                    "recommended": entry.recommended,
                    "artifact_uri": entry.artifact_uri(),
                    "metadata": metadata,
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
