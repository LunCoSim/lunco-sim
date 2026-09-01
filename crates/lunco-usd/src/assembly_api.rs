//! Read-only Assembly inspection through the shared API query extension.
//!
//! The provider intentionally reads the same `DocumentRegistry<UsdDocument>`
//! and `lunco-usd-compose` dependency interpreter used by editing and
//! projection. It does not maintain a second asset graph or infer an active
//! document from UI state.

use bevy::prelude::World;
use lunco_api::queries::ApiQueryProvider;
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_doc::{Document, DocumentId};
use lunco_doc_bevy::{DocumentRegistry, JournalResource};
use lunco_usd_bevy::usd_data::UsdDataExt;
use openusd::sdf::Path as SdfPath;

use crate::document::UsdDocument;

/// Read-only query for one explicit open USD document.
///
/// Parameters:
///
/// ```json
/// { "doc": 3, "path": "/Rover" }
/// ```
///
/// `path` is optional. Without it, the response describes document identity,
/// authored layers, revisions, dependencies, and journal position. With it,
/// the response additionally reports the composed local prim type, activity,
/// and immediate children.
pub struct InspectUsdDocumentProvider;

impl ApiQueryProvider for InspectUsdDocumentProvider {
    fn name(&self) -> &'static str {
        "InspectUsdDocument"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(raw_doc) = params.get("doc").and_then(serde_json::Value::as_u64) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "InspectUsdDocument requires an explicit numeric `doc`",
            );
        };
        let doc = DocumentId::new(raw_doc);
        let Some(host) = world
            .get_resource::<DocumentRegistry<UsdDocument>>()
            .and_then(|registry| registry.host(doc))
        else {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("USD document {doc} is not open"),
            );
        };
        let document = host.document();
        let source = document.source();
        let runtime_bytes = lunco_usd_bevy::author::data_to_usda(document.runtime_data())
            .map_or(0, |source| source.len());
        let mut diagnostics = Vec::new();
        if let Some(error) = document.parse_error() {
            diagnostics.push(error.to_owned());
        }
        let dependencies = match lunco_usd_compose::layer_dependency_arcs(&source) {
            Some(dependencies) => dependencies,
            None => {
                diagnostics.push("document source is not valid USDA".to_owned());
                Vec::new()
            }
        };
        let journal = world
            .get_resource::<JournalResource>()
            .map(|journal| {
                journal.with_read(|journal| {
                    let entries: Vec<_> = journal.entries_for_doc(doc).collect();
                    serde_json::json!({
                        "entries": entries.len(),
                        "cursor": entries.last().map(|entry| entry.id.clone()),
                    })
                })
            })
            .unwrap_or_else(|| serde_json::json!({ "entries": 0, "cursor": null }));

        let mut response = serde_json::json!({
            "doc": doc,
            "generation": document.generation(),
            "origin": document.origin(),
            "dirty": document.is_dirty(),
            "layers": {
                "root": {
                    "id": "@root@",
                    "persistent": true,
                    "revision": document.base_revision(),
                    "bytes": source.len(),
                },
                "runtime": {
                    "id": "@runtime@",
                    "persistent": false,
                    "revision": document.runtime_revision(),
                    "bytes": runtime_bytes,
                },
            },
            "composition": {
                "dependencies": dependencies,
                "owner": "lunco-usd-compose",
            },
            "journal": journal,
            "diagnostics": diagnostics,
        });

        if let Some(raw_path) = params.get("path").and_then(serde_json::Value::as_str) {
            let Ok(path) = SdfPath::new(raw_path) else {
                return ApiResponse::error(
                    ApiErrorCode::DeserializationError,
                    format!("invalid USD prim path `{raw_path}`"),
                );
            };
            let composed = document.composed_arc();
            response["prim"] = serde_json::json!({
                "path": raw_path,
                "exists": composed.spec(&path).is_some(),
                "type": composed.prim_type_name(&path),
                "active": composed.prim_is_active(&path),
                "children": composed
                    .prim_children(&path)
                    .into_iter()
                    .map(|child| child.to_string())
                    .collect::<Vec<_>>(),
            });
        }

        ApiResponse::ok(response)
    }
}
