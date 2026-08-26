//! API/script read surface for the generic dataset registry.
//!
//! Download ownership stays in `lunco-assets`; this module only adapts its
//! authoritative state to the existing language-neutral query bridge.

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::{ApiErrorCode, ApiResponse};

/// `ListDatasets` — list declared engine and Twin datasets without exposing
/// machine-local paths. Requests use each returned `id` with `RequestDataset`
/// or `CancelDataset`.
///
/// params: `{ scope?: string }` where `scope` is the engine group or Twin name
/// · returns `{ datasets: [{ id, key, group, scope, name, state, processed, recommended }] }`
pub struct ListDatasetsProvider;

impl ApiQueryProvider for ListDatasetsProvider {
    fn name(&self) -> &'static str {
        "ListDatasets"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let filter = match params.get("scope") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(scope)) => Some(scope.as_str()),
            Some(_) => {
                return ApiResponse::error(
                    ApiErrorCode::DeserializationError,
                    "ListDatasets: `scope` must be a string",
                )
            }
        };
        let Some(registry) = world.get_resource::<lunco_assets::datasets::DatasetRegistry>() else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "ListDatasets: dataset registry is not installed",
            );
        };
        let datasets = registry
            .entries()
            .iter()
            .filter(|entry| filter.is_none_or(|value| entry.scope.label() == value))
            .map(|entry| {
                let state = match &entry.state {
                    lunco_assets::datasets::DatasetState::Missing => {
                        serde_json::json!({ "kind": "missing" })
                    }
                    lunco_assets::datasets::DatasetState::Downloading {
                        bytes_done,
                        bytes_total,
                    } => serde_json::json!({
                        "kind": "downloading",
                        "bytes_done": bytes_done,
                        "bytes_total": bytes_total,
                    }),
                    lunco_assets::datasets::DatasetState::Processing { kind } => {
                        serde_json::json!({ "kind": "processing", "process": kind })
                    }
                    lunco_assets::datasets::DatasetState::Cancelling => {
                        serde_json::json!({ "kind": "cancelling" })
                    }
                    lunco_assets::datasets::DatasetState::Installed => {
                        serde_json::json!({ "kind": "installed" })
                    }
                    lunco_assets::datasets::DatasetState::Cancelled => {
                        serde_json::json!({ "kind": "cancelled" })
                    }
                    lunco_assets::datasets::DatasetState::Failed(error) => {
                        serde_json::json!({ "kind": "failed", "error": error })
                    }
                };
                serde_json::json!({
                    "id": entry.id,
                    "key": entry.key,
                    "group": entry.group,
                    "scope": entry.scope.label(),
                    "name": entry.name,
                    "state": state,
                    "processed": entry.spec.process.is_some(),
                    "recommended": entry.recommended,
                })
            })
            .collect::<Vec<_>>();
        ApiResponse::ok(serde_json::json!({ "datasets": datasets }))
    }
}

/// Register the dataset read surface beside the other script/API queries.
pub fn register_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(ListDatasetsProvider);
}
