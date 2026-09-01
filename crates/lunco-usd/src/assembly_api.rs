//! Read-only Assembly inspection through the shared API query extension.
//!
//! The provider intentionally reads the same `DocumentRegistry<UsdDocument>`
//! and `lunco-usd-compose` dependency interpreter used by editing and
//! projection. It does not maintain a second asset graph or infer an active
//! document from UI state.

use bevy::asset::AssetServer;
use bevy::prelude::World;
use lunco_api::queries::ApiQueryProvider;
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_doc::{Document, DocumentId};
use lunco_doc_bevy::{DocumentRegistry, JournalResource};
use lunco_usd_bevy::usd_data::UsdDataExt;
use openusd::sdf::Path as SdfPath;

use crate::document::UsdDocument;
use crate::edit_session::UsdEditSessions;

fn journal_position(world: &World, doc: DocumentId) -> serde_json::Value {
    world
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
        .unwrap_or_else(|| serde_json::json!({ "entries": 0, "cursor": null }))
}

fn runtime_source(document: &UsdDocument) -> Result<String, String> {
    lunco_usd_bevy::author::data_to_usda(document.runtime_data())
        .map_err(|error| format!("could not serialize runtime layer: {error}"))
}

fn document_snapshot(
    world: &World,
    doc: DocumentId,
    document: &UsdDocument,
    reason: &str,
    from_generation: Option<u64>,
) -> Result<serde_json::Value, String> {
    let source = document.source();
    let runtime = runtime_source(document)?;
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
    Ok(serde_json::json!({
        "kind": "snapshot",
        "reason": reason,
        "from_generation": from_generation,
        "doc": doc,
        "generation": document.generation(),
        "origin": document.origin(),
        "dirty": document.is_dirty(),
        "layers": {
            "root": {
                "id": "@root@",
                "persistent": true,
                "revision": document.base_revision(),
                "source": source,
            },
            "runtime": {
                "id": "@runtime@",
                "persistent": false,
                "revision": document.runtime_revision(),
                "source": runtime,
            },
        },
        "composition": {
            "dependencies": dependencies,
            "owner": "lunco-usd-compose",
        },
        "journal": journal_position(world, doc),
        "diagnostics": diagnostics,
    }))
}

fn canonical_stage_for_document<'a>(
    world: &'a World,
    doc: DocumentId,
) -> Option<&'a lunco_usd_bevy::CanonicalStage> {
    let (name, rel) = world
        .get_resource::<crate::twin_projection::DocBackedTwinScenes>()?
        .coords_of(doc)?;
    let twin_path = lunco_assets::twin_uri(&name, &rel);
    let stage_id = world
        .get_resource::<AssetServer>()?
        .get_handle::<lunco_usd_bevy::UsdStageAsset>(twin_path)?
        .id();
    world
        .get_non_send::<lunco_usd_bevy::CanonicalStages>()?
        .get(stage_id)
}

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
        let journal = journal_position(world, doc);

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

/// Read-only query for the Assembly Editor's pending review plans.
///
/// The response includes typed operations and their validation/conflict state,
/// but inspecting it never changes authored USD. It is document-scoped and
/// does not infer an active editor tab.
pub struct InspectUsdEditSessionProvider;

impl ApiQueryProvider for InspectUsdEditSessionProvider {
    fn name(&self) -> &'static str {
        "InspectUsdEditSession"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(raw_doc) = params.get("doc").and_then(serde_json::Value::as_u64) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "InspectUsdEditSession requires an explicit numeric `doc`",
            );
        };
        let doc = DocumentId::new(raw_doc);
        let Some(document) = world
            .get_resource::<DocumentRegistry<UsdDocument>>()
            .and_then(|registry| registry.host(doc))
            .map(|host| host.document())
        else {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("USD document {doc} is not open"),
            );
        };
        let externally_stale = world
            .get_resource::<DocumentRegistry<UsdDocument>>()
            .is_some_and(|registry| registry.stale_docs().contains(&doc));

        let mut proposals: Vec<_> = world
            .get_resource::<UsdEditSessions>()
            .map(|sessions| {
                sessions
                    .for_document(doc)
                    .map(|proposal| {
                        serde_json::json!({
                            "id": proposal.id,
                            "scope": proposal.scope.as_str(),
                            "label": proposal.label,
                            "parent_generation": proposal.parent_generation,
                            "base_revision": proposal.base_revision,
                            "origin": proposal.origin,
                            "state": proposal.state.as_str(),
                            "ops": proposal.ops,
                            "affected_paths": proposal.affected_paths,
                            "diagnostics": proposal.diagnostics,
                            "stale": proposal.parent_generation != document.generation()
                                || proposal.base_revision != document.base_revision()
                                || proposal.origin != document.origin().session_uri()
                                || externally_stale,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        proposals.sort_by_key(|proposal| {
            proposal
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default()
        });

        ApiResponse::ok(serde_json::json!({
            "doc": doc,
            "generation": document.generation(),
            "base_revision": document.base_revision(),
            "dirty": document.is_dirty(),
            "origin": document.origin(),
            "proposals": proposals,
        }))
    }
}

/// Return the exact typed-op suffix after a document generation, or a complete
/// layer snapshot when the document's bounded op ring no longer covers the
/// requested generation.
///
/// Parameters:
///
/// ```json
/// { "doc": 3, "since_generation": 17 }
/// ```
///
/// Omitting `since_generation` asks for a complete snapshot. A generation
/// newer than the document is rejected; the caller must not silently reset its
/// cursor and miss an edit.
pub struct SyncUsdDocumentProvider;

impl ApiQueryProvider for SyncUsdDocumentProvider {
    fn name(&self) -> &'static str {
        "SyncUsdDocument"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(raw_doc) = params.get("doc").and_then(serde_json::Value::as_u64) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "SyncUsdDocument requires an explicit numeric `doc`",
            );
        };
        let doc = DocumentId::new(raw_doc);
        let Some(document) = world
            .get_resource::<DocumentRegistry<UsdDocument>>()
            .and_then(|registry| registry.host(doc))
            .map(|host| host.document())
        else {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("USD document {doc} is not open"),
            );
        };
        let since = match params.get("since_generation") {
            None => None,
            Some(value) => {
                let Some(generation) = value.as_u64() else {
                    return ApiResponse::error(
                        ApiErrorCode::DeserializationError,
                        "SyncUsdDocument `since_generation` must be a non-negative integer",
                    );
                };
                Some(generation)
            }
        };
        let Some(since) = since else {
            return match document_snapshot(world, doc, document, "initial", None) {
                Ok(snapshot) => ApiResponse::ok(snapshot),
                Err(error) => ApiResponse::error(ApiErrorCode::InternalError, error),
            };
        };
        let generation = document.generation();
        if since > generation {
            return ApiResponse::error(
                ApiErrorCode::CommandRejected,
                format!(
                    "SyncUsdDocument cursor {since} is newer than document {doc} generation {generation}"
                ),
            );
        }
        match document.ops_since(since) {
            Some(ops) => ApiResponse::ok(serde_json::json!({
                "kind": "delta",
                "doc": doc,
                "from_generation": since,
                "to_generation": generation,
                "ops": ops,
                "journal": journal_position(world, doc),
            })),
            None => match document_snapshot(
                world,
                doc,
                document,
                "history_window_exceeded",
                Some(since),
            ) {
                Ok(snapshot) => ApiResponse::ok(snapshot),
                Err(error) => ApiResponse::error(ApiErrorCode::InternalError, error),
            },
        }
    }
}

/// Resolve one explicit edit target against the document's authored layers and,
/// when necessary, the one live OpenUSD composed stage.
///
/// Parameters:
///
/// ```json
/// { "doc": 3, "path": "/Rover/Wheel", "edit_target": "@runtime@" }
/// ```
///
/// Referenced and payloaded paths are never guessed from the authored layer.
/// They require the already-mounted canonical stage, whose OpenUSD PCP stack
/// supplies the authoritative authored layer/path pairs.
pub struct ResolveUsdTargetProvider;

impl ApiQueryProvider for ResolveUsdTargetProvider {
    fn name(&self) -> &'static str {
        "ResolveUsdTarget"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(raw_doc) = params.get("doc").and_then(serde_json::Value::as_u64) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "ResolveUsdTarget requires an explicit numeric `doc`",
            );
        };
        let Some(raw_path) = params.get("path").and_then(serde_json::Value::as_str) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "ResolveUsdTarget requires an explicit USD prim `path`",
            );
        };
        let Some(raw_target) = params
            .get("edit_target")
            .and_then(serde_json::Value::as_str)
        else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "ResolveUsdTarget requires an explicit `edit_target`",
            );
        };
        let edit_target = crate::LayerId::new(raw_target);
        if !edit_target.is_root() && !edit_target.is_runtime() {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                format!("unknown USD edit target `{raw_target}`"),
            );
        }
        let Ok(path) = SdfPath::new(raw_path) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                format!("invalid USD prim path `{raw_path}`"),
            );
        };
        let doc = DocumentId::new(raw_doc);
        let Some(document) = world
            .get_resource::<DocumentRegistry<UsdDocument>>()
            .and_then(|registry| registry.host(doc))
            .map(|host| host.document())
        else {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("USD document {doc} is not open"),
            );
        };
        let authored_here = match document.authored_prim_exists(&edit_target, raw_path) {
            Ok(exists) => exists,
            Err(error) => {
                return ApiResponse::error(ApiErrorCode::DeserializationError, error.to_string())
            }
        };
        let authored_in_document = match (
            document.authored_prim_exists(&crate::LayerId::root(), raw_path),
            document.authored_prim_exists(&crate::LayerId::runtime(), raw_path),
        ) {
            (Ok(root), Ok(runtime)) => root || runtime,
            (Err(error), _) | (_, Err(error)) => {
                return ApiResponse::error(ApiErrorCode::DeserializationError, error.to_string())
            }
        };
        let under_arc = match document.path_is_under_composed_arc(raw_path) {
            Ok(value) => value,
            Err(error) => {
                return ApiResponse::error(ApiErrorCode::DeserializationError, error.to_string())
            }
        };

        if let Some(stage) = canonical_stage_for_document(world, doc) {
            let prim = stage.stage().prim(path.clone());
            let composed_exists = match prim.is_valid() {
                Ok(exists) => exists,
                Err(error) => {
                    return ApiResponse::error(
                        ApiErrorCode::InternalError,
                        format!("OpenUSD could not validate `{raw_path}`: {error}"),
                    )
                }
            };
            if composed_exists {
                let Ok(stack) = prim.prim_stack() else {
                    return ApiResponse::error(
                        ApiErrorCode::InternalError,
                        format!("OpenUSD could not return the prim stack for `{raw_path}`"),
                    );
                };
                return ApiResponse::ok(serde_json::json!({
                    "doc": doc,
                    "path": raw_path,
                    "edit_target": edit_target,
                    "status": "resolved",
                    "source": "canonical_stage",
                    "composed_exists": true,
                    "authored_here": authored_here,
                    "authored_in_document": authored_in_document,
                    "edit_scope": if authored_here {
                        "authored_layer"
                    } else if authored_in_document || under_arc {
                        "local_override"
                    } else {
                        "composed_read_only"
                    },
                    "prim_stack": stack.into_iter().map(|(layer, authored_path)| {
                        serde_json::json!({
                            "layer": layer.to_string(),
                            "path": authored_path.to_string(),
                        })
                    }).collect::<Vec<_>>(),
                }));
            }
            if under_arc {
                return ApiResponse::error(
                    ApiErrorCode::EntityNotFound,
                    format!("OpenUSD composed stage does not contain referenced path `{raw_path}`"),
                );
            }
        } else if under_arc {
            return ApiResponse::error(
                ApiErrorCode::CommandRejected,
                format!(
                    "referenced path `{raw_path}` cannot be resolved until its canonical USD stage is mounted"
                ),
            );
        }

        let composed_exists = document
            .composed_arc()
            .spec(&path)
            .is_some_and(|spec| spec.ty == openusd::sdf::SpecType::Prim)
            || authored_in_document;
        ApiResponse::ok(serde_json::json!({
            "doc": doc,
            "path": raw_path,
            "edit_target": edit_target,
            "status": if composed_exists { "resolved" } else { "missing" },
            "source": "document_layers",
            "composed_exists": composed_exists,
            "authored_here": authored_here,
            "authored_in_document": authored_in_document,
            "edit_scope": if authored_here {
                "authored_layer"
            } else if composed_exists && authored_in_document {
                "local_override"
            } else {
                "missing"
            },
            "prim_stack": [],
        }))
    }
}
