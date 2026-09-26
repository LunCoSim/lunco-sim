//! Read-only Assembly inspection through the shared API query extension.
//!
//! The provider intentionally reads the same `DocumentRegistry<UsdDocument>`
//! and `lunco-usd-compose` dependency interpreter used by editing and
//! projection. It does not maintain a second asset graph or infer an active
//! document from UI state.

use bevy::prelude::{App, Plugin, Vec3, World};
use lunco_api::queries::{
    ApiQueryError, ApiQueryProvider, ApiQueryRegistry, ApiQueryResult, api_param_f64,
    api_param_str, api_param_u64,
};
use lunco_api_core::{ApiErrorCode, ApiValue, api_value, api_value_from_serializable};
use lunco_doc::{Document, DocumentId};
use lunco_doc_bevy::{DocumentRegistry, JournalResource};
use lunco_usd_avian_contracts::AvianMeshApproximation;
use lunco_usd_bevy_mesh::build_nurbs_collision_mesh_to_tolerance;
use lunco_usd_bevy_stage::{UsdRead, stage_convention};
use lunco_usd_bevy_twin::{DocBackedTwinScenes, canonical_stage_for_document};
use lunco_usd_data::usd_data::UsdDataExt;
use openusd::schemas::physics::CollisionApprox;
use openusd::sdf::{Path as SdfPath, Value as SdfValue};

use lunco_usd_core::edit_session::UsdEditSessions;
use lunco_usd_document::document::UsdDocument;

fn query_ok(value: ApiValue) -> ApiQueryResult {
    Ok(Some(value))
}

fn query_error(code: ApiErrorCode, message: impl Into<String>) -> ApiQueryResult {
    Err(ApiQueryError::new(code, message))
}

/// Installs the public USD document query providers.
///
/// Query registration belongs beside the provider implementations, not in the
/// USD command/lifecycle package. Runtime compositions can therefore update
/// query code without rebuilding the command observers.
pub struct UsdQueriesPlugin;

impl Plugin for UsdQueriesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ApiQueryRegistry>();
        let mut registry = app.world_mut().resource_mut::<ApiQueryRegistry>();
        registry.register(InspectUsdDocumentProvider);
        registry.register(InspectUsdEditSessionProvider);
        registry.register(ResolveUsdTargetProvider);
        registry.register(SyncUsdDocumentProvider);
        registry.register(AvianMeshCollisionApproximationsProvider);
        registry.register(PlanNurbsCollisionProxyProvider);
    }
}

/// Report the mesh approximation tokens implemented by the active Avian USD
/// adapter. Authoring tools consume this capability query instead of keeping
/// their own token allow-lists.
pub struct AvianMeshCollisionApproximationsProvider;

impl ApiQueryProvider for AvianMeshCollisionApproximationsProvider {
    fn name(&self) -> &'static str {
        "AvianMeshCollisionApproximations"
    }

    fn execute(&self, _world: &World, _params: &ApiValue) -> ApiQueryResult {
        let approximations =
            AvianMeshApproximation::ALL.map(|mode| mode.as_usd_approximation().as_token());
        query_ok(api_value!({ "approximations": approximations }))
    }
}

/// Plan a source-derived collision mesh for an explicit NURBS prim. This is a
/// read-only geometry cook: its output is intended for a normal USD edit
/// proposal, so it does not write around the document journal.
pub struct PlanNurbsCollisionProxyProvider;

impl ApiQueryProvider for PlanNurbsCollisionProxyProvider {
    fn name(&self) -> &'static str {
        "PlanNurbsCollisionProxy"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(raw_doc) = api_param_u64(params, "doc_id") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "PlanNurbsCollisionProxy: explicit numeric doc_id is required",
            ));
        };
        let doc = DocumentId::new(raw_doc);
        let Some(host) = world
            .get_resource::<DocumentRegistry<UsdDocument>>()
            .and_then(|registry| registry.host(doc))
        else {
            return Err(ApiQueryError::new(
                ApiErrorCode::EntityNotFound,
                format!("PlanNurbsCollisionProxy: USD document {doc} is not open"),
            ));
        };
        let generation = host.document().generation();
        if world
            .get_resource::<DocBackedTwinScenes>()
            .and_then(|scenes| scenes.synced_generation(doc))
            .is_some_and(|synced| synced != generation)
        {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                format!("PlanNurbsCollisionProxy: document {doc} projection is not current"),
            ));
        }

        let Some(source_path) = api_param_str(params, "path") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "PlanNurbsCollisionProxy: USD source `path` is required",
            ));
        };
        let Ok(source) = SdfPath::new(source_path) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!("PlanNurbsCollisionProxy: `{source_path}` is not a valid USD path"),
            ));
        };
        let Some(proxy_name) = api_param_str(params, "proxy_name") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "PlanNurbsCollisionProxy: one child `proxy_name` is required",
            ));
        };
        if !SdfPath::is_valid_identifier(proxy_name) {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!("PlanNurbsCollisionProxy: `{proxy_name}` is not a valid prim identifier"),
            ));
        }
        let proxy_path = format!("{source}/{proxy_name}");
        let proxy = SdfPath::new(&proxy_path).map_err(|_| {
            ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "PlanNurbsCollisionProxy: generated proxy path is invalid",
            )
        })?;
        let approximation_token = api_param_str(params, "approximation")
            .unwrap_or(CollisionApprox::ConvexHull.as_token());
        let usd_approximation = CollisionApprox::from_token(approximation_token).ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!(
                    "PlanNurbsCollisionProxy: `{approximation_token}` is not a standard UsdPhysicsMeshCollisionAPI approximation token"
                ),
            )
        })?;
        let approximation = AvianMeshApproximation::try_from(usd_approximation).map_err(
            |unsupported| {
                let supported = AvianMeshApproximation::ALL
                    .map(|mode| mode.as_usd_approximation().as_token())
                    .join(", ");
                ApiQueryError::new(
                    ApiErrorCode::CommandRejected,
                    format!(
                        "PlanNurbsCollisionProxy: Avian does not implement the standard `{}` mode; supported Avian modes are {supported}",
                        unsupported.as_token()
                    ),
                )
            },
        )?;

        let Some(stage) = canonical_stage_for_document(world, doc) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                format!("PlanNurbsCollisionProxy: document {doc} has no composed USD stage"),
            ));
        };
        let view = stage.view();
        if !view.has_prim(&source) {
            return Err(ApiQueryError::new(
                ApiErrorCode::EntityNotFound,
                format!("PlanNurbsCollisionProxy: source prim `{source}` is not composed"),
            ));
        }
        if view.type_name(&source).as_deref() != Some("NurbsPatch") {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!("PlanNurbsCollisionProxy: `{source}` must be a UsdGeomNurbsPatch"),
            ));
        }
        if approximation.requires_static_or_kinematic_body() {
            let mut ancestor = Some(source.clone());
            while let Some(path) = ancestor {
                if view.has_api_schema(&path, "PhysicsRigidBodyAPI") {
                    let enabled = match view.boolean(&path, "physics:rigidBodyEnabled") {
                        Some(value) => value,
                        None if view.has_authored_attribute(&path, "physics:rigidBodyEnabled") => {
                            return Err(ApiQueryError::new(
                                ApiErrorCode::DeserializationError,
                                format!(
                                    "PlanNurbsCollisionProxy: `{path}` has malformed physics:rigidBodyEnabled"
                                ),
                            ));
                        }
                        None => true,
                    };
                    let kinematic = match view.boolean(&path, "physics:kinematicEnabled") {
                        Some(value) => value,
                        None if view.has_authored_attribute(&path, "physics:kinematicEnabled") => {
                            return Err(ApiQueryError::new(
                                ApiErrorCode::DeserializationError,
                                format!(
                                    "PlanNurbsCollisionProxy: `{path}` has malformed physics:kinematicEnabled"
                                ),
                            ));
                        }
                        None => false,
                    };
                    if enabled && !kinematic {
                        return Err(ApiQueryError::new(
                            ApiErrorCode::CommandRejected,
                            format!(
                                "PlanNurbsCollisionProxy: `{}` creates a triangle mesh and cannot be used on dynamic body `{path}`; choose one of the supported convex approximations",
                                approximation.as_usd_approximation().as_token()
                            ),
                        ));
                    }
                }
                ancestor = path.parent();
            }
        }
        let create_prim = if view.has_prim(&proxy) {
            let owned = view.type_name(&proxy).as_deref() == Some("Mesh")
                && view.has_api_schema(&proxy, "LunCoDerivedGeometryAPI")
                && view.rel_targets(&proxy, "lunco:derived:source").as_slice() == [source.clone()];
            if !owned {
                return Err(ApiQueryError::new(
                    ApiErrorCode::CommandRejected,
                    format!(
                        "PlanNurbsCollisionProxy: `{proxy}` already exists and is not this source's derived proxy"
                    ),
                ));
            }
            false
        } else {
            true
        };
        let mut schemas = if create_prim {
            Vec::new()
        } else {
            view.api_schemas(&proxy)
        };
        for required in [
            "PhysicsCollisionAPI",
            "PhysicsMeshCollisionAPI",
            "LunCoDerivedGeometryAPI",
        ] {
            if !schemas.iter().any(|schema| schema == required) {
                schemas.push(required.to_owned());
            }
        }

        let Some(deviation_tolerance_m) = api_param_f64(params, "deviation_tolerance_m") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "PlanNurbsCollisionProxy: positive `deviation_tolerance_m` in canonical metres is required",
            ));
        };
        let cooked = build_nurbs_collision_mesh_to_tolerance(&view, &source, deviation_tolerance_m)
            .map_err(|error| {
                let code = match &error {
                    lunco_usd_bevy_mesh::NurbsCollisionCookError::InvalidTolerance
                    | lunco_usd_bevy_mesh::NurbsCollisionCookError::InvalidSurface => {
                        ApiErrorCode::DeserializationError
                    }
                    _ => ApiErrorCode::CommandRejected,
                };
                ApiQueryError::new(
                    code,
                    format!("PlanNurbsCollisionProxy: `{source}`: {error}"),
                )
            })?;
        let convention = stage_convention(&view).map_err(|error| {
            ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!("PlanNurbsCollisionProxy: invalid USD stage convention: {error}"),
            )
        })?;
        let points = cooked
            .mesh
            .points
            .iter()
            .map(|point| {
                let p = convention.stage_point(Vec3::from_array(*point));
                api_value!([p.x, p.y, p.z])
            })
            .collect::<Vec<_>>();

        Ok(Some(api_value!({
            "doc_id": raw_doc,
            "generation": generation,
            "source_path": source.to_string(),
            "proxy_name": proxy_name,
            "proxy_path": proxy.to_string(),
            "create_prim": create_prim,
            "approximation": approximation.as_usd_approximation().as_token(),
            "supported_approximations": AvianMeshApproximation::ALL
                .map(|mode| mode.as_usd_approximation().as_token()),
            "schemas": schemas,
            "deviation_tolerance_m": cooked.deviation_tolerance_m,
            "refinement_deviation_m": cooked.refinement_deviation_m,
            "u_subdivisions": cooked.tessellation.u_subdivisions as i64,
            "v_subdivisions": cooked.tessellation.v_subdivisions as i64,
            "trim_curve_samples": cooked.tessellation.trim_curve_samples as i64,
            "trim_grid_subdivisions": cooked.tessellation.trim_grid_subdivisions as i64,
            "geometry_fingerprint": cooked.mesh.geometry_fingerprint,
            "points": points,
            "face_vertex_counts": cooked.mesh.face_vertex_counts,
            "face_vertex_indices": cooked.mesh.face_vertex_indices,
        })))
    }
}

fn journal_position(world: &World, doc: DocumentId) -> Result<ApiValue, ApiQueryError> {
    let journal = world.get_resource::<JournalResource>().ok_or_else(|| {
        ApiQueryError::new(
            ApiErrorCode::InternalError,
            "USD document journal is not installed",
        )
    })?;
    let (entry_count, cursor) = journal.with_read(|journal| {
        let entries: Vec<_> = journal.entries_for_doc(doc).collect();
        (entries.len(), entries.last().map(|entry| entry.id.clone()))
    });
    let cursor = api_value_from_serializable(&cursor)?;
    Ok(api_value!({
        "entries": entry_count,
        "cursor": cursor,
    }))
}

fn runtime_source(document: &UsdDocument) -> Result<String, String> {
    lunco_usd_authoring::author::data_to_usda(document.runtime_data())
        .map_err(|error| format!("could not serialize runtime layer: {error}"))
}

fn view_source(document: &UsdDocument) -> Result<String, String> {
    lunco_usd_authoring::author::data_to_usda(document.view_data())
        .map_err(|error| format!("could not serialize view layer: {error}"))
}

fn document_snapshot(
    world: &World,
    doc: DocumentId,
    document: &UsdDocument,
    reason: &str,
    from_generation: Option<u64>,
) -> Result<ApiValue, ApiQueryError> {
    let source = document.source();
    let runtime = runtime_source(document)
        .map_err(|error| ApiQueryError::new(ApiErrorCode::InternalError, error))?;
    let view = view_source(document)
        .map_err(|error| ApiQueryError::new(ApiErrorCode::InternalError, error))?;
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
    Ok(api_value!({
        "kind": "snapshot",
        "reason": reason,
        "from_generation": from_generation,
        "doc_id": doc.raw(),
        "generation": document.generation(),
        "origin": api_value_from_serializable(document.origin())?,
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
                "persistent": "twin_policy",
                "revision": document.runtime_revision(),
                "source": runtime,
            },
            "view": {
                "id": "@view@",
                "persistent": false,
                "revision": document.view_revision(),
                "source": view,
            },
        },
        "composition": {
            "dependencies": dependencies,
            "owner": "lunco-usd-compose",
        },
        "journal": journal_position(world, doc)?,
        "diagnostics": diagnostics,
    }))
}

fn composed_attribute_inspection(
    world: &World,
    doc: DocumentId,
    document: &UsdDocument,
    path: &SdfPath,
) -> (bool, Option<String>, bool, Vec<String>, Vec<ApiValue>) {
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
                let value = spec.get("default").cloned().and_then(|value| {
                    lunco_usd_authoring::author::value_to_literal(&type_name, value)
                });
                Some(api_value!({
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
    if let Some(stage) = lunco_usd_bevy_twin::canonical_stage_for_document(world, doc) {
        let reader = stage.view();
        let attributes = reader
            .attr_names(path)
            .into_iter()
            .filter_map(|name| {
                let type_name = reader.attr_type_name(path, &name)?;
                let value = reader.attr_value(path, &name).and_then(|value| {
                    lunco_usd_authoring::author::value_to_literal(&type_name, value)
                });
                Some(api_value!({
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

fn reference_api_value(reference: &openusd::sdf::Reference) -> ApiValue {
    api_value!({
        "asset_path": reference.asset_path.clone(),
        "prim_path": if reference.prim_path.is_empty() {
            ApiValue::Unit
        } else {
            ApiValue::Str(reference.prim_path.to_string())
        },
    })
}

fn references_api_value(references: &[openusd::sdf::Reference]) -> Vec<ApiValue> {
    references.iter().map(reference_api_value).collect()
}

fn reference_list_api_value(data: &dyn openusd::sdf::AbstractData, path: &SdfPath) -> ApiValue {
    let Some(value) = data
        .try_field(path, openusd::sdf::FieldKey::References.as_str())
        .ok()
        .flatten()
    else {
        return api_value!({
            "present": false,
            "items": [],
        });
    };
    let SdfValue::ReferenceListOp(op) = value.as_ref() else {
        return api_value!({
            "present": false,
            "items": [],
        });
    };
    api_value!({
        "present": true,
        "explicit": op.explicit,
        "explicit_items": references_api_value(&op.explicit_items),
        "prepended_items": references_api_value(&op.prepended_items),
        "appended_items": references_api_value(&op.appended_items),
        "added_items": references_api_value(&op.added_items),
        "deleted_items": references_api_value(&op.deleted_items),
        "ordered_items": references_api_value(&op.ordered_items),
        "items": references_api_value(&op.flatten()),
    })
}

/// Read reference opinions from the canonical PCP stack when the document is
/// mounted. The document-layer view above remains the synchronous authoring
/// source; this additional view reports the composed sites and list result
/// after external references and local overrides have been resolved.
fn canonical_reference_api_value(
    world: &World,
    doc: DocumentId,
    path: &SdfPath,
) -> Option<ApiValue> {
    let stage = lunco_usd_bevy_twin::canonical_stage_for_document(world, doc)?;
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
        sites.push(api_value!({
            "layer": layer_id,
            "path": authored_path.to_string(),
            "list": reference_list_api_value(layer.data(), &authored_path),
        }));
        list_ops.push(op.clone());
    }
    let mut composed = Vec::new();
    for op in list_ops.iter().rev() {
        composed = op.compose_over(&composed);
    }
    Some(api_value!({
        "source": "canonical_stage",
        "sites": sites,
        "items": references_api_value(&composed),
    }))
}

fn token_metadata_api_value(
    data: &dyn openusd::sdf::AbstractData,
    path: &SdfPath,
    field: &str,
    source: &str,
) -> ApiValue {
    let Some(value) = data.try_field(path, field).ok().flatten() else {
        return api_value!({
            "present": false,
            "value": ApiValue::Unit,
            "source": source,
        });
    };
    match value.as_ref() {
        SdfValue::Token(token) => api_value!({
            "present": true,
            "value": token.to_string(),
            "source": source,
        }),
        _ => api_value!({
            "present": true,
            "value": ApiValue::Unit,
            "source": source,
            "error": "metadata is not a USD token",
        }),
    }
}

fn variant_selection_metadata_api_value(
    data: &dyn openusd::sdf::AbstractData,
    path: &SdfPath,
    source: &str,
) -> ApiValue {
    let Some(value) = data
        .try_field(path, openusd::sdf::FieldKey::VariantSelection.as_str())
        .ok()
        .flatten()
    else {
        return api_value!({
            "present": false,
            "value": ApiValue::Unit,
            "source": source,
        });
    };
    match value.as_ref() {
        SdfValue::VariantSelectionMap(map) => {
            let selections = map
                .iter()
                .map(|(set, variant)| (set.to_string(), api_value!(variant.to_string())))
                .collect::<Vec<(String, ApiValue)>>();
            api_value!({
                "present": true,
                "value": ApiValue::Map(selections),
                "source": source,
            })
        }
        _ => api_value!({
            "present": true,
            "value": ApiValue::Unit,
            "source": source,
            "error": "metadata is not a USD variant-selection map",
        }),
    }
}

fn variant_selection_stack_api_value(document: &UsdDocument, path: &SdfPath) -> ApiValue {
    api_value!({
        "authored": {
            "root": variant_selection_metadata_api_value(
                document.data(), path, "@root@",
            ),
            "runtime": variant_selection_metadata_api_value(
                document.runtime_data(), path, "@runtime@",
            ),
            "view": variant_selection_metadata_api_value(
                document.view_data(), path, "@view@",
            ),
        },
        "composed": variant_selection_metadata_api_value(
            document.composed_arc().as_ref(), path, "document_composed",
        ),
    })
}

fn metadata_stack_api_value(
    document: &UsdDocument,
    path: &SdfPath,
    field: &str,
    canonical_value: Option<String>,
) -> ApiValue {
    let mut metadata = vec![
        (
            "authored".to_owned(),
            api_value!({
                "root": token_metadata_api_value(document.data(), path, field, "@root@"),
                "runtime": token_metadata_api_value(document.runtime_data(), path, field, "@runtime@"),
                "view": token_metadata_api_value(document.view_data(), path, field, "@view@"),
            }),
        ),
        (
            "composed".to_owned(),
            token_metadata_api_value(
                document.composed_arc().as_ref(),
                path,
                field,
                "document_composed",
            ),
        ),
    ];
    if let Some(value) = canonical_value {
        metadata.push((
            "canonical_stage".to_owned(),
            api_value!({
                "present": true,
                "value": value,
                "source": "canonical_stage",
            }),
        ));
    }
    ApiValue::Map(metadata)
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

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(raw_doc) = api_param_u64(params, "doc_id") else {
            return query_error(
                ApiErrorCode::DeserializationError,
                "InspectUsdDocument requires an explicit numeric `doc_id`",
            );
        };
        let doc = DocumentId::new(raw_doc);
        let Some(host) = world
            .get_resource::<DocumentRegistry<UsdDocument>>()
            .and_then(|registry| registry.host(doc))
        else {
            return query_error(
                ApiErrorCode::EntityNotFound,
                format!("USD document {doc} is not open"),
            );
        };
        let document = host.document();
        let source = document.source();
        let runtime_bytes = runtime_source(document)
            .map_err(|error| ApiQueryError::new(ApiErrorCode::InternalError, error))?
            .len();
        let view_bytes = view_source(document)
            .map_err(|error| ApiQueryError::new(ApiErrorCode::InternalError, error))?
            .len();
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
        let journal = journal_position(world, doc)?;
        let root_path = SdfPath::abs_root();
        let canonical_default_prim = lunco_usd_bevy_twin::canonical_stage_for_document(world, doc)
            .and_then(|stage| stage.view().default_prim());

        let mut response = vec![
            ("doc_id".to_owned(), api_value!(doc.raw())),
            ("generation".to_owned(), api_value!(document.generation())),
            (
                "origin".to_owned(),
                api_value_from_serializable(document.origin())?,
            ),
            ("dirty".to_owned(), api_value!(document.is_dirty())),
            (
                "layers".to_owned(),
                api_value!({
                    "root": {
                        "id": "@root@",
                        "persistent": true,
                        "revision": document.base_revision(),
                        "bytes": source.len(),
                    },
                    "runtime": {
                        "id": "@runtime@",
                        "persistent": "twin_policy",
                        "revision": document.runtime_revision(),
                        "bytes": runtime_bytes,
                    },
                    "view": {
                        "id": "@view@",
                        "persistent": false,
                        "revision": document.view_revision(),
                        "bytes": view_bytes,
                    },
                }),
            ),
            (
                "composition".to_owned(),
                api_value!({
                    "dependencies": dependencies,
                    "owner": "lunco-usd-compose",
                }),
            ),
            (
                "metadata".to_owned(),
                api_value!({
                    "defaultPrim": metadata_stack_api_value(
                        document,
                        &root_path,
                        openusd::sdf::FieldKey::DefaultPrim.as_str(),
                        canonical_default_prim,
                    ),
                }),
            ),
            ("journal".to_owned(), journal),
            ("diagnostics".to_owned(), api_value!(diagnostics)),
        ];

        if params.get("path").is_some() && params.get("path").and_then(ApiValue::as_str).is_none() {
            return query_error(
                ApiErrorCode::DeserializationError,
                "InspectUsdDocument `path` must be a string",
            );
        }
        if let Some(raw_path) = params.get("path").and_then(ApiValue::as_str) {
            let Ok(path) = SdfPath::new(raw_path) else {
                return query_error(
                    ApiErrorCode::DeserializationError,
                    format!("invalid USD prim path `{raw_path}`"),
                );
            };
            let (exists, type_name, active, children, attributes) =
                composed_attribute_inspection(world, doc, document, &path);
            let mut references = vec![
                (
                    "authored".to_owned(),
                    api_value!({
                        "root": reference_list_api_value(document.data(), &path),
                        "runtime": reference_list_api_value(document.runtime_data(), &path),
                        "view": reference_list_api_value(document.view_data(), &path),
                    }),
                ),
                (
                    "composed".to_owned(),
                    reference_list_api_value(document.composed_arc().as_ref(), &path),
                ),
            ];
            if let Some(canonical) = canonical_reference_api_value(world, doc, &path) {
                references.push(("canonical_stage".to_owned(), canonical));
            }
            let canonical_kind = lunco_usd_bevy_twin::canonical_stage_for_document(world, doc)
                .and_then(|stage| stage.view().kind(&path));
            response.push((
                "prim".to_owned(),
                api_value!({
                    "path": raw_path,
                    "exists": exists,
                    "type": type_name,
                    "active": active,
                    "children": children,
                    "attributes": attributes,
                    "metadata": {
                        "kind": metadata_stack_api_value(
                            document,
                            &path,
                            openusd::sdf::FieldKey::Kind.as_str(),
                            canonical_kind,
                        ),
                        "variantSelections": variant_selection_stack_api_value(document, &path),
                    },
                    "references": ApiValue::Map(references),
                }),
            ));
        }

        query_ok(ApiValue::Map(response))
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

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(raw_doc) = api_param_u64(params, "doc_id") else {
            return query_error(
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
            return query_error(
                ApiErrorCode::EntityNotFound,
                format!("USD document {doc} is not open"),
            );
        };
        let externally_stale = world
            .get_resource::<DocumentRegistry<UsdDocument>>()
            .is_some_and(|registry| registry.stale_docs().contains(&doc));

        let sessions = world.get_resource::<UsdEditSessions>().ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::InternalError,
                "USD edit sessions are not installed",
            )
        })?;
        let mut proposals: Vec<_> = sessions
            .for_document(doc)
            .map(|proposal| -> Result<_, ApiQueryError> {
                Ok((
                    proposal.id.0,
                    api_value!({
                            "id": proposal.id.0,
                            "scope": proposal.scope.as_str(),
                            "label": proposal.label.clone(),
                            "parent_generation": proposal.parent_generation,
                            "base_revision": proposal.base_revision,
                            "origin": proposal.origin.clone(),
                            "state": proposal.state.as_str(),
                            "ops": api_value_from_serializable(&proposal.ops)?,
                            "affected_paths": proposal.affected_paths.clone(),
                            "diagnostics": proposal.diagnostics.clone(),
                            "stale": proposal.parent_generation != document.generation()
                                || proposal.base_revision != document.base_revision()
                                || proposal.origin != document.origin().session_uri()
                                || externally_stale,
                    }),
                ))
            })
            .collect::<Result<_, ApiQueryError>>()?;
        proposals.sort_by_key(|(id, _)| *id);
        let proposals = proposals
            .into_iter()
            .map(|(_, proposal)| proposal)
            .collect::<Vec<_>>();

        let origin = api_value_from_serializable(document.origin())?;
        query_ok(api_value!({
            "doc_id": doc.raw(),
            "generation": document.generation(),
            "base_revision": document.base_revision(),
            "dirty": document.is_dirty(),
            "origin": origin,
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

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(raw_doc) = api_param_u64(params, "doc_id") else {
            return query_error(
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
            return query_error(
                ApiErrorCode::EntityNotFound,
                format!("USD document {doc} is not open"),
            );
        };
        let since = match params.get("since_generation") {
            None => None,
            Some(_) => {
                let Some(generation) = api_param_u64(params, "since_generation") else {
                    return query_error(
                        ApiErrorCode::DeserializationError,
                        "SyncUsdDocument `since_generation` must be a non-negative integer",
                    );
                };
                Some(generation)
            }
        };
        let Some(since) = since else {
            return match document_snapshot(world, doc, document, "initial", None) {
                Ok(snapshot) => query_ok(snapshot),
                Err(error) => Err(error),
            };
        };
        let generation = document.generation();
        if since > generation {
            return query_error(
                ApiErrorCode::CommandRejected,
                format!(
                    "SyncUsdDocument cursor {since} is newer than document {doc} generation {generation}"
                ),
            );
        }
        match document.ops_since(since) {
            Some(ops) => {
                let journal = journal_position(world, doc)?;
                let ops = api_value_from_serializable(&ops)?;
                query_ok(api_value!({
                    "kind": "delta",
                    "doc_id": doc.raw(),
                    "from_generation": since,
                    "to_generation": generation,
                    "ops": ops,
                    "journal": journal,
                }))
            }
            None => match document_snapshot(
                world,
                doc,
                document,
                "history_window_exceeded",
                Some(since),
            ) {
                Ok(snapshot) => query_ok(snapshot),
                Err(error) => Err(error),
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
/// Composed-only referenced and payloaded paths require the already-mounted
/// canonical stage, whose OpenUSD PCP stack supplies the authoritative
/// authored layer/path pairs. A path authored in this document remains
/// resolvable from the document layer while the canonical stage catches up
/// with that local edit; projection latency must not turn a valid authoring
/// target into a user-visible missing-path error.
pub struct ResolveUsdTargetProvider;

impl ApiQueryProvider for ResolveUsdTargetProvider {
    fn name(&self) -> &'static str {
        "ResolveUsdTarget"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(raw_doc) = api_param_u64(params, "doc_id") else {
            return query_error(
                ApiErrorCode::DeserializationError,
                "ResolveUsdTarget requires an explicit numeric `doc_id`",
            );
        };
        let Some(raw_path) = params.get("path").and_then(ApiValue::as_str) else {
            return query_error(
                ApiErrorCode::DeserializationError,
                "ResolveUsdTarget requires an explicit USD prim `path`",
            );
        };
        let Some(raw_target) = params.get("edit_target").and_then(ApiValue::as_str) else {
            return query_error(
                ApiErrorCode::DeserializationError,
                "ResolveUsdTarget requires an explicit `edit_target`",
            );
        };
        let edit_target = lunco_usd_document::document::LayerId::new(raw_target);
        if !edit_target.is_root() && !edit_target.is_runtime() {
            return query_error(
                ApiErrorCode::DeserializationError,
                format!("unknown USD edit target `{raw_target}`"),
            );
        }
        let Ok(path) = SdfPath::new(raw_path) else {
            return query_error(
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
            return query_error(
                ApiErrorCode::EntityNotFound,
                format!("USD document {doc} is not open"),
            );
        };
        let authored_here = match document.authored_prim_exists(&edit_target, raw_path) {
            Ok(exists) => exists,
            Err(error) => {
                return query_error(ApiErrorCode::DeserializationError, error.to_string());
            }
        };
        let authored_in_document = match (
            document.authored_prim_exists(&lunco_usd_document::document::LayerId::root(), raw_path),
            document
                .authored_prim_exists(&lunco_usd_document::document::LayerId::runtime(), raw_path),
        ) {
            (Ok(root), Ok(runtime)) => root || runtime,
            (Err(error), _) | (_, Err(error)) => {
                return query_error(ApiErrorCode::DeserializationError, error.to_string());
            }
        };
        let under_arc = match document.path_is_under_composed_arc(raw_path) {
            Ok(value) => value,
            Err(error) => {
                return query_error(ApiErrorCode::DeserializationError, error.to_string());
            }
        };

        let document_composed_exists = document
            .composed_arc()
            .spec(&path)
            .is_some_and(|spec| spec.ty == openusd::sdf::SpecType::Prim)
            || authored_in_document;
        let document_layer_response = || {
            query_ok(api_value!({
                "doc_id": doc.raw(),
                "path": raw_path,
                "edit_target": edit_target.as_str(),
                "status": if document_composed_exists { "resolved" } else { "missing" },
                "source": "document_layers",
                "composed_exists": document_composed_exists,
                "authored_here": authored_here,
                "authored_in_document": authored_in_document,
                "under_arc": under_arc,
                "edit_scope": if authored_here {
                    "authored_layer"
                } else if document_composed_exists && authored_in_document {
                    "local_override"
                } else {
                    "missing"
                },
                "prim_stack": [],
            }))
        };

        if let Some(stage) = lunco_usd_bevy_twin::canonical_stage_for_document(world, doc) {
            let prim = stage.stage().prim(path.clone());
            let composed_exists = match prim.is_valid() {
                Ok(exists) => exists,
                Err(error) => {
                    return query_error(
                        ApiErrorCode::InternalError,
                        format!("OpenUSD could not validate `{raw_path}`: {error}"),
                    );
                }
            };
            if composed_exists {
                let Ok(stack) = prim.prim_stack() else {
                    return query_error(
                        ApiErrorCode::InternalError,
                        format!("OpenUSD could not return the prim stack for `{raw_path}`"),
                    );
                };
                return query_ok(api_value!({
                    "doc_id": doc.raw(),
                    "path": raw_path,
                    "edit_target": edit_target.as_str(),
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
                        api_value!({
                            "layer": layer,
                            "path": authored_path.to_string(),
                        })
                    }).collect::<Vec<_>>(),
                }));
            }
            if authored_in_document {
                return document_layer_response();
            }
            if under_arc {
                return query_error(
                    ApiErrorCode::EntityNotFound,
                    format!("OpenUSD composed stage does not contain referenced path `{raw_path}`"),
                );
            }
        } else if under_arc {
            if authored_in_document {
                return document_layer_response();
            }
            return query_error(
                ApiErrorCode::CommandRejected,
                format!(
                    "referenced path `{raw_path}` cannot be resolved until its canonical USD stage is mounted"
                ),
            );
        }
        document_layer_response()
    }
}
