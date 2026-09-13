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
use lunco_usd_bevy_core::UsdRead;
use lunco_usd_core::UsdDataExt;
use openusd::sdf::{Path as SdfPath, Value as SdfValue};

use lunco_usd_core::document::UsdDocument;
use lunco_usd_core::edit_session::UsdEditSessions;

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
    lunco_usd_core::author::data_to_usda(document.runtime_data())
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
        "doc_id": doc,
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

/// Resolve an explicitly mapped document to its canonical composed stage.
/// Callers requiring the current document generation must check the projection
/// cursor on `DocBackedTwinScenes` before consuming this derived stage.
pub fn canonical_stage_for_document(
    world: &World,
    doc: DocumentId,
) -> Option<&lunco_usd_bevy_core::canonical::CanonicalStage> {
    let (name, rel) = world
        .get_resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()?
        .coords_of(doc)?;
    let twin_path = lunco_assets::twin_uri(&name, &rel);
    let stage_id = world
        .get_resource::<AssetServer>()?
        .get_handle::<lunco_usd_bevy_core::UsdStageAsset>(twin_path)?
        .id();
    world
        .get_non_send::<lunco_usd_bevy_core::canonical::CanonicalStages>()?
        .get(stage_id)
}

fn composed_attribute_inspection(
    world: &World,
    doc: DocumentId,
    document: &UsdDocument,
    path: &SdfPath,
) -> (
    bool,
    Option<String>,
    bool,
    Vec<String>,
    Vec<serde_json::Value>,
) {
    // The document is the synchronous authoring boundary. The canonical stage
    // is a derived projection and may still be settling the newest generation.
    // Read document-owned paths here so a command response and its immediate
    // inspection observe the same committed USD composition.
    let composed = document.composed_arc();
    if composed.spec(path).is_some() {
        let attributes = composed
            .iter()
            .filter_map(|(property, spec)| {
                if property.prim_path() != *path || spec.ty != openusd::sdf::SpecType::Attribute {
                    return None;
                }
                let type_name = match spec.get("typeName") {
                    Some(openusd::sdf::Value::Token(type_name)) => type_name.to_string(),
                    _ => return None,
                };
                let value = spec
                    .get("default")
                    .cloned()
                    .and_then(|value| lunco_usd_core::author::value_to_literal(&type_name, value));
                Some(serde_json::json!({
                    "name": property.name(),
                    "type": type_name,
                    "value": value,
                }))
            })
            .collect();
        return (
            true,
            composed.prim_type_name(path),
            composed.prim_is_active(path),
            composed
                .prim_children(path)
                .into_iter()
                .map(|child| child.to_string())
                .collect(),
            attributes,
        );
    }

    // Paths introduced by references and other external composition arcs are
    // available only on the fully resolved live stage.
    if let Some(stage) = canonical_stage_for_document(world, doc) {
        let reader = stage.view();
        let attributes = reader
            .attr_names(path)
            .into_iter()
            .filter_map(|name| {
                let type_name = reader.attr_type_name(path, &name)?;
                let value = reader
                    .attr_value(path, &name)
                    .and_then(|value| lunco_usd_core::author::value_to_literal(&type_name, value));
                Some(serde_json::json!({
                    "name": name,
                    "type": type_name,
                    "value": value,
                }))
            })
            .collect();
        return (
            reader.has_prim(path),
            reader.type_name(path),
            reader.is_active(path),
            reader
                .children(path)
                .into_iter()
                .map(|child| child.to_string())
                .collect(),
            attributes,
        );
    }

    (false, None, false, Vec::new(), Vec::new())
}

fn reference_json(reference: &openusd::sdf::Reference) -> serde_json::Value {
    serde_json::json!({
        "asset_path": reference.asset_path,
        "prim_path": if reference.prim_path.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(reference.prim_path.to_string())
        },
    })
}

fn references_json(references: &[openusd::sdf::Reference]) -> Vec<serde_json::Value> {
    references.iter().map(reference_json).collect()
}

fn reference_list_json(data: &dyn openusd::sdf::AbstractData, path: &SdfPath) -> serde_json::Value {
    let Some(value) = data
        .try_field(path, openusd::sdf::FieldKey::References.as_str())
        .ok()
        .flatten()
    else {
        return serde_json::json!({
            "present": false,
            "items": [],
        });
    };
    let SdfValue::ReferenceListOp(op) = value.as_ref() else {
        return serde_json::json!({
            "present": false,
            "items": [],
        });
    };
    serde_json::json!({
        "present": true,
        "explicit": op.explicit,
        "explicit_items": references_json(&op.explicit_items),
        "prepended_items": references_json(&op.prepended_items),
        "appended_items": references_json(&op.appended_items),
        "added_items": references_json(&op.added_items),
        "deleted_items": references_json(&op.deleted_items),
        "ordered_items": references_json(&op.ordered_items),
        "items": references_json(&op.flatten()),
    })
}

/// Read reference opinions from the canonical PCP stack when the document is
/// mounted. The document-layer view above remains the synchronous authoring
/// source; this additional view reports the composed sites and list result
/// after external references and local overrides have been resolved.
fn canonical_reference_json(
    world: &World,
    doc: DocumentId,
    path: &SdfPath,
) -> Option<serde_json::Value> {
    let stage = canonical_stage_for_document(world, doc)?;
    let prim = stage.stage().prim(path.clone());
    if !prim.is_valid().ok()? {
        return None;
    }
    let stack = prim.prim_stack().ok()?;
    let mut sites = Vec::new();
    let mut list_ops = Vec::new();
    for (layer_id, authored_path) in stack {
        let Some(layer) = stage.stage().layer(&layer_id) else {
            continue;
        };
        let Some(value) = layer
            .data()
            .try_field(&authored_path, openusd::sdf::FieldKey::References.as_str())
            .ok()
            .flatten()
        else {
            continue;
        };
        let SdfValue::ReferenceListOp(op) = value.as_ref() else {
            continue;
        };
        sites.push(serde_json::json!({
            "layer": layer_id,
            "path": authored_path.to_string(),
            "list": reference_list_json(layer.data(), &authored_path),
        }));
        list_ops.push(op.clone());
    }
    let mut composed = Vec::new();
    for op in list_ops.iter().rev() {
        composed = op.compose_over(&composed);
    }
    Some(serde_json::json!({
        "source": "canonical_stage",
        "sites": sites,
        "items": references_json(&composed),
    }))
}

fn token_metadata_json(
    data: &dyn openusd::sdf::AbstractData,
    path: &SdfPath,
    field: &str,
    source: &str,
) -> serde_json::Value {
    let Some(value) = data.try_field(path, field).ok().flatten() else {
        return serde_json::json!({
            "present": false,
            "value": serde_json::Value::Null,
            "source": source,
        });
    };
    match value.as_ref() {
        SdfValue::Token(token) => serde_json::json!({
            "present": true,
            "value": token.to_string(),
            "source": source,
        }),
        _ => serde_json::json!({
            "present": true,
            "value": serde_json::Value::Null,
            "source": source,
            "error": "metadata is not a USD token",
        }),
    }
}

fn variant_selection_metadata_json(
    data: &dyn openusd::sdf::AbstractData,
    path: &SdfPath,
    source: &str,
) -> serde_json::Value {
    let Some(value) = data
        .try_field(path, openusd::sdf::FieldKey::VariantSelection.as_str())
        .ok()
        .flatten()
    else {
        return serde_json::json!({
            "present": false,
            "value": serde_json::Value::Null,
            "source": source,
        });
    };
    match value.as_ref() {
        SdfValue::VariantSelectionMap(map) => {
            let selections = map
                .iter()
                .map(|(set, variant)| (set.to_string(), serde_json::json!(variant.to_string())))
                .collect::<serde_json::Map<_, _>>();
            serde_json::json!({
                "present": true,
                "value": selections,
                "source": source,
            })
        }
        _ => serde_json::json!({
            "present": true,
            "value": serde_json::Value::Null,
            "source": source,
            "error": "metadata is not a USD variant-selection map",
        }),
    }
}

fn variant_selection_stack_json(document: &UsdDocument, path: &SdfPath) -> serde_json::Value {
    serde_json::json!({
        "authored": {
            "root": variant_selection_metadata_json(
                document.data(), path, "@root@",
            ),
            "runtime": variant_selection_metadata_json(
                document.runtime_data(), path, "@runtime@",
            ),
        },
        "composed": variant_selection_metadata_json(
            document.composed_arc().as_ref(), path, "document_composed",
        ),
    })
}

fn metadata_stack_json(
    document: &UsdDocument,
    path: &SdfPath,
    field: &str,
    canonical_value: Option<String>,
) -> serde_json::Value {
    let mut metadata = serde_json::json!({
        "authored": {
            "root": token_metadata_json(document.data(), path, field, "@root@"),
            "runtime": token_metadata_json(document.runtime_data(), path, field, "@runtime@"),
        },
        "composed": token_metadata_json(
            document.composed_arc().as_ref(),
            path,
            field,
            "document_composed",
        ),
    });
    if let Some(value) = canonical_value {
        metadata["canonical_stage"] = serde_json::json!({
            "present": true,
            "value": value,
            "source": "canonical_stage",
        });
    }
    metadata
}

/// Read-only query for one explicit open USD document.
///
/// Parameters:
///
/// ```json
/// { "doc_id": 3, "path": "/Rover" }
/// ```
///
/// `path` is optional. Without it, the response describes document identity,
/// authored layers, revisions, dependencies, and journal position. With it,
/// the response additionally reports the composed prim type, activity,
/// immediate children, and each composed attribute's exact USD declaration
/// and default literal. The declaration comes from OpenUSD's `typeName`,
/// preserving roles and array shape that are not represented by `sdf::Value`
/// variants.
pub struct InspectUsdDocumentProvider;

impl ApiQueryProvider for InspectUsdDocumentProvider {
    fn name(&self) -> &'static str {
        "InspectUsdDocument"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(raw_doc) = params.get("doc_id").and_then(serde_json::Value::as_u64) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "InspectUsdDocument requires an explicit numeric `doc_id`",
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
        let runtime_bytes = lunco_usd_core::author::data_to_usda(document.runtime_data())
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
        let root_path = SdfPath::abs_root();
        let canonical_default_prim =
            canonical_stage_for_document(world, doc).and_then(|stage| stage.view().default_prim());

        let mut response = serde_json::json!({
            "doc_id": doc,
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
            "metadata": {
                "defaultPrim": metadata_stack_json(
                    document,
                    &root_path,
                    openusd::sdf::FieldKey::DefaultPrim.as_str(),
                    canonical_default_prim,
                ),
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
            let (exists, type_name, active, children, attributes) =
                composed_attribute_inspection(world, doc, document, &path);
            let mut references = serde_json::json!({
                "authored": {
                    "root": reference_list_json(document.data(), &path),
                    "runtime": reference_list_json(document.runtime_data(), &path),
                },
                "composed": reference_list_json(document.composed_arc().as_ref(), &path),
            });
            if let Some(canonical) = canonical_reference_json(world, doc, &path) {
                references["canonical_stage"] = canonical;
            }
            let canonical_kind =
                canonical_stage_for_document(world, doc).and_then(|stage| stage.view().kind(&path));
            response["prim"] = serde_json::json!({
                "path": raw_path,
                "exists": exists,
                "type": type_name,
                "active": active,
                "children": children,
                "attributes": attributes,
                "metadata": {
                    "kind": metadata_stack_json(
                        document,
                        &path,
                        openusd::sdf::FieldKey::Kind.as_str(),
                        canonical_kind,
                    ),
                    "variantSelections": variant_selection_stack_json(document, &path),
                },
                "references": references,
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
        let Some(raw_doc) = params.get("doc_id").and_then(serde_json::Value::as_u64) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "InspectUsdEditSession requires an explicit numeric `doc_id`",
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
            "doc_id": doc,
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
/// { "doc_id": 3, "since_generation": 17 }
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
        let Some(raw_doc) = params.get("doc_id").and_then(serde_json::Value::as_u64) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "SyncUsdDocument requires an explicit numeric `doc_id`",
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
                "doc_id": doc,
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
/// { "doc_id": 3, "path": "/Rover/Wheel", "edit_target": "@runtime@" }
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
        let Some(raw_doc) = params.get("doc_id").and_then(serde_json::Value::as_u64) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "ResolveUsdTarget requires an explicit numeric `doc_id`",
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
        let edit_target = lunco_usd_core::LayerId::new(raw_target);
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
                return ApiResponse::error(ApiErrorCode::DeserializationError, error.to_string());
            }
        };
        let authored_in_document = match (
            document.authored_prim_exists(&lunco_usd_core::LayerId::root(), raw_path),
            document.authored_prim_exists(&lunco_usd_core::LayerId::runtime(), raw_path),
        ) {
            (Ok(root), Ok(runtime)) => root || runtime,
            (Err(error), _) | (_, Err(error)) => {
                return ApiResponse::error(ApiErrorCode::DeserializationError, error.to_string());
            }
        };
        let under_arc = match document.path_is_under_composed_arc(raw_path) {
            Ok(value) => value,
            Err(error) => {
                return ApiResponse::error(ApiErrorCode::DeserializationError, error.to_string());
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
                    );
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
                    "doc_id": doc,
                    "path": raw_path,
                    "edit_target": edit_target,
                    "status": "resolved",
                    "source": "canonical_stage",
                    "composed_exists": true,
                    "authored_here": authored_here,
                    "authored_in_document": authored_in_document,
                    "under_arc": under_arc,
                    "edit_scope": if authored_here {
                        "authored_layer"
                    } else if authored_in_document || under_arc {
                        "local_override"
                    } else {
                        "composed_read_only"
                    },
                    "prim_stack": stack.into_iter().map(|(layer, authored_path)| {
                        serde_json::json!({
                            "layer": layer,
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
            "doc_id": doc,
            "path": raw_path,
            "edit_target": edit_target,
            "status": if composed_exists { "resolved" } else { "missing" },
            "source": "document_layers",
            "composed_exists": composed_exists,
            "authored_here": authored_here,
            "authored_in_document": authored_in_document,
            "under_arc": under_arc,
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
