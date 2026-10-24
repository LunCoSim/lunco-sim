//! Read-only Assembly inspection through the shared API query extension.
//!
//! The provider intentionally reads the same `DocumentRegistry<UsdDocument>`
//! and `lunco-usd-compose` dependency interpreter used by editing and
//! projection. It does not maintain a second asset graph or infer an active
//! document from UI state.

use bevy::asset::Assets;
use bevy::prelude::{App, Plugin, Vec3, World};
use lunco_api::queries::{
    ApiQueryError, ApiQueryProvider, ApiQueryRegistry, ApiQueryResult, api_param_bool,
    api_param_f64, api_param_str, api_param_u64,
};
use lunco_api_core::{
    ApiErrorCode, ApiQueryParameterSchema, ApiQuerySchema, ApiValue, api_value,
    api_value_from_serializable,
};
use lunco_doc::{Document, DocumentId};
use lunco_doc_bevy::{DocumentRegistry, JournalResource};
use lunco_usd_avian_contracts::AvianMeshApproximation;
use lunco_usd_bevy_mesh::build_nurbs_collision_mesh_with_refinement_limit;
use lunco_usd_bevy_stage::{UsdRead, UsdStageAsset, stage_convention};
use lunco_usd_bevy_twin::{
    DocBackedTwinScenes, canonical_stage_for_document, stage_asset_for_document,
};
use openusd::sdf::{Path as SdfPath, Value as SdfValue};
use openusd_schemas::physics::CollisionApprox;

use lunco_usd_core::edit_session::UsdEditSessions;
use lunco_usd_document::document::UsdDocument;

fn query_ok(value: ApiValue) -> ApiQueryResult {
    Ok(Some(value))
}

fn query_error(code: ApiErrorCode, message: impl Into<String>) -> ApiQueryResult {
    Err(ApiQueryError::new(code, message))
}

fn authored_child_paths(
    document: &UsdDocument,
    edit_target: &lunco_usd_document::document::LayerId,
    path: &SdfPath,
) -> Result<Vec<String>, String> {
    let data = if edit_target.is_root() {
        document.data()
    } else if edit_target.is_runtime() {
        document.runtime_data()
    } else {
        document.view_data()
    };
    let Some(spec) = data.spec(path) else {
        return Ok(Vec::new());
    };
    let children = match spec.get("primChildren") {
        None => Vec::new(),
        Some(SdfValue::TokenVec(names)) => names.iter().map(ToString::to_string).collect(),
        Some(SdfValue::TokenListOp(names)) => names
            .flatten()
            .into_iter()
            .map(|name| name.to_string())
            .collect(),
        Some(_) => {
            return Err(format!(
                "invalid primChildren field at `{path}` in `{}`: expected token list",
                edit_target.as_str(),
            ));
        }
    };
    children
        .into_iter()
        .map(|name| {
            path.append_path(name.as_str())
                .map(|child| child.to_string())
                .map_err(|error| format!("invalid authored child `{name}` at `{path}`: {error}"))
        })
        .collect()
}

fn include_authored_children(
    result: ApiQueryResult,
    children: Option<&[String]>,
) -> ApiQueryResult {
    let Some(children) = children else {
        return result;
    };
    match result {
        Ok(Some(ApiValue::Map(mut entries))) => {
            entries.push((
                "authored_children".to_owned(),
                api_value!(children.to_vec()),
            ));
            Ok(Some(ApiValue::Map(entries)))
        }
        other => other,
    }
}

fn document_stage_recipe(
    world: &World,
    doc: DocumentId,
) -> Result<Option<lunco_usd_compose::recipe::StageRecipe>, ApiQueryError> {
    let Some(backed) = world.get_resource::<DocBackedTwinScenes>() else {
        return Ok(None);
    };
    let Some(stage_id) = stage_asset_for_document(backed, doc) else {
        return Ok(None);
    };
    let assets = world
        .get_resource::<Assets<UsdStageAsset>>()
        .ok_or_else(|| native_query_error("USD stage asset registry is unavailable"))?;
    let stages = world
        .get_non_send::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
        .ok_or_else(|| native_query_error("USD canonical stage registry is unavailable"))?;
    stages
        .admitted_recipe_for(stage_id, assets)
        .map(Some)
        .map_err(native_query_error)
}

fn native_query_error(error: impl std::fmt::Display) -> ApiQueryError {
    ApiQueryError::new(
        ApiErrorCode::InternalError,
        format!("OpenUSD document query failed: {error}"),
    )
}

fn open_document_stage(
    world: &World,
    doc: DocumentId,
    document: &UsdDocument,
) -> Result<openusd::usd::Stage, ApiQueryError> {
    if document.parse_error().is_some() {
        return Err(ApiQueryError::new(
            ApiErrorCode::DeserializationError,
            "cannot query composition of an invalid USD document source",
        ));
    }
    match document_stage_recipe(world, doc)? {
        Some(recipe) => document.open_composed_stage_with_recipe(&recipe),
        None => document.open_composed_stage(),
    }
    .map_err(native_query_error)
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

/// Report the mesh approximation modes implemented by the active Avian USD
/// adapter, including cooked geometry and rigid-body compatibility. Authoring
/// tools consume this capability query instead of keeping mode rules locally.
pub struct AvianMeshCollisionApproximationsProvider;

impl ApiQueryProvider for AvianMeshCollisionApproximationsProvider {
    fn name(&self) -> &'static str {
        "AvianMeshCollisionApproximations"
    }

    fn execute(&self, _world: &World, _params: &ApiValue) -> ApiQueryResult {
        query_ok(api_value!({ "modes": avian_mesh_approximation_capabilities() }))
    }
}

fn avian_mesh_approximation_capabilities() -> ApiValue {
    ApiValue::Array(
        AvianMeshApproximation::ALL
            .into_iter()
            .map(|mode| {
                let capability = mode.capability();
                api_value!({
                    "token": mode.as_usd_approximation().as_token(),
                    "geometry": capability.geometry.as_token(),
                    "body_support": capability.body_support.as_token(),
                })
            })
            .collect(),
    )
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

        let Some(max_refinement_delta_m) = api_param_f64(params, "max_refinement_delta_m") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "PlanNurbsCollisionProxy: positive `max_refinement_delta_m` in canonical metres is required",
            ));
        };
        let cooked = build_nurbs_collision_mesh_with_refinement_limit(
            &view,
            &source,
            max_refinement_delta_m,
        )
        .map_err(|error| {
            let code = match &error {
                lunco_usd_bevy_mesh::NurbsCollisionCookError::InvalidRefinementLimit
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
            "modes": avian_mesh_approximation_capabilities(),
            "schemas": schemas,
            "max_refinement_delta_m": cooked.max_refinement_delta_m,
            "refinement_delta_m": cooked.refinement_delta_m,
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
    let source = document
        .source()
        .map_err(|error| ApiQueryError::new(ApiErrorCode::InternalError, error.to_string()))?;
    let runtime = runtime_source(document)
        .map_err(|error| ApiQueryError::new(ApiErrorCode::InternalError, error))?;
    let view = view_source(document)
        .map_err(|error| ApiQueryError::new(ApiErrorCode::InternalError, error))?;
    let mut diagnostics = Vec::new();
    if let Some(error) = document.parse_error() {
        diagnostics.push(error.to_owned());
    }
    let dependencies = match lunco_usd_compose::layer_dependency_arcs(source.as_bytes()) {
        Ok(dependencies) => dependencies,
        Err(error) => {
            diagnostics.push(format!("document dependency analysis failed: {error}"));
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
    document: &UsdDocument,
    stage: &openusd::usd::Stage,
    path: &SdfPath,
) -> Result<(bool, Option<String>, bool, Vec<String>, Vec<ApiValue>), ApiQueryError> {
    let prim = stage.prim(path).map_err(native_query_error)?;
    if !prim.is_valid().map_err(native_query_error)? {
        return Ok((false, None, false, Vec::new(), Vec::new()));
    }
    let attributes = prim
        .attributes()
        .map_err(native_query_error)?
        .into_iter()
        .map(|attribute| {
            let type_name = attribute
                .type_name()
                .map_err(native_query_error)?
                .ok_or_else(|| {
                    native_query_error(format!("attribute `{}` has no USD type", attribute.path()))
                })?
                .to_string();
            let value = attribute
                .get::<SdfValue>()
                .map_err(native_query_error)?
                .map(|value| {
                    lunco_usd_authoring::author::value_to_literal(&type_name, value).ok_or_else(
                        || {
                            native_query_error(format!(
                                "attribute `{}` has no USDA literal form",
                                attribute.path()
                            ))
                        },
                    )
                })
                .transpose()?;
            let property = attribute.path();
            Ok(api_value!({
                "name": attribute.name(), "type": type_name, "value": value,
                "authored": {
                    "root": document.data().spec(property).is_some(),
                    "runtime": document.runtime_data().spec(property).is_some(),
                    "view": document.view_data().spec(property).is_some(),
                },
            }))
        })
        .collect::<Result<Vec<_>, ApiQueryError>>()?;
    Ok((
        true,
        prim.type_name()
            .map_err(native_query_error)?
            .map(|name| name.to_string()),
        prim.is_active().map_err(native_query_error)?,
        prim.children()
            .map_err(native_query_error)?
            .into_iter()
            .map(|child| child.path().to_string())
            .collect(),
        attributes,
    ))
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

fn reference_list_value(value: Option<&SdfValue>) -> Result<ApiValue, ApiQueryError> {
    let Some(value) = value else {
        return Ok(api_value!({ "present": false, "items": [] }));
    };
    let SdfValue::ReferenceListOp(op) = value else {
        return Err(native_query_error(
            "references metadata is not a USD reference list-op",
        ));
    };
    Ok(api_value!({
        "present": true,
        "explicit": op.is_explicit(),
        "explicit_items": references_api_value(op.explicit_items()),
        "prepended_items": references_api_value(op.prepended_items()),
        "appended_items": references_api_value(op.appended_items()),
        "added_items": references_api_value(op.added_items()),
        "deleted_items": references_api_value(op.deleted_items()),
        "ordered_items": references_api_value(op.ordered_items()),
        "items": references_api_value(&op.flatten()),
    }))
}

fn reference_list_api_value(
    data: &dyn openusd::sdf::AbstractData,
    path: &SdfPath,
) -> Result<ApiValue, ApiQueryError> {
    let value = data
        .try_field(path, openusd::sdf::FieldKey::References.as_str())
        .map_err(native_query_error)?;
    reference_list_value(value.as_deref())
}

/// Native composed reference metadata and the authored spec sites that supply it.
fn composed_reference_api_value(
    stage: &openusd::usd::Stage,
    path: &SdfPath,
) -> Result<ApiValue, ApiQueryError> {
    let prim = stage.prim(path).map_err(native_query_error)?;
    let value = stage
        .field::<SdfValue>(path, openusd::sdf::FieldKey::References)
        .map_err(native_query_error)?;
    let mut sites = Vec::new();
    for site in prim.prim_stack().map_err(native_query_error)? {
        let layer = stage.layer(&site.layer).ok_or_else(|| {
            native_query_error(format!("missing native spec-site layer `{}`", site.layer))
        })?;
        let value = layer
            .data()
            .try_field(&site.path, openusd::sdf::FieldKey::References.as_str())
            .map_err(native_query_error)?;
        if value.is_some() {
            sites.push(api_value!({ "layer": site.layer, "path": site.path.to_string(), "list": reference_list_value(value.as_deref())? }));
        }
    }
    let ApiValue::Map(mut entries) = reference_list_value(value.as_ref())? else {
        return Err(native_query_error(
            "reference inspection must produce a map",
        ));
    };
    entries.push(("source".to_owned(), api_value!("document_composition")));
    entries.push(("sites".to_owned(), api_value!(sites)));
    Ok(ApiValue::Map(entries))
}

fn metadata_api_value(
    value: Option<&SdfValue>,
    source: &str,
    variant_selections: bool,
) -> ApiValue {
    let Some(value) = value else {
        return api_value!({ "present": false, "value": ApiValue::Unit, "source": source });
    };
    let decoded = match value {
        SdfValue::Token(token) if !variant_selections => Some(api_value!(token.to_string())),
        SdfValue::VariantSelectionMap(map) if variant_selections => Some(ApiValue::Map(
            map.iter()
                .map(|(set, variant)| (set.to_string(), api_value!(variant.to_string())))
                .collect(),
        )),
        _ => None,
    };
    match decoded {
        Some(value) => api_value!({ "present": true, "value": value, "source": source }),
        None => api_value!({ "present": true, "value": ApiValue::Unit, "source": source,
            "error": if variant_selections { "metadata is not a USD variant-selection map" } else { "metadata is not a USD token" } }),
    }
}

fn metadata_stack_api_value(
    document: &UsdDocument,
    stage: &openusd::usd::Stage,
    path: &SdfPath,
    field: &str,
) -> Result<ApiValue, ApiQueryError> {
    let variant_selections = field == openusd::sdf::FieldKey::VariantSelection.as_str();
    let mut authored = Vec::new();
    for (name, id, data) in [
        ("root", "@root@", document.data()),
        ("runtime", "@runtime@", document.runtime_data()),
        ("view", "@view@", document.view_data()),
    ] {
        let value =
            openusd::sdf::AbstractData::try_field(data, path, field).map_err(native_query_error)?;
        authored.push((
            name.to_owned(),
            metadata_api_value(value.as_deref(), id, variant_selections),
        ));
    }
    let value = stage
        .field::<SdfValue>(path, field)
        .map_err(native_query_error)?;
    Ok(api_value!({
        "authored": ApiValue::Map(authored),
        "composed": metadata_api_value(value.as_ref(), "document_composition", variant_selections),
    }))
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
/// and default literal, including schema defaults. Layer statistics count
/// authored specs; inspection never serializes a layer to count bytes. The
/// declaration comes from OpenUSD's `typeName`,
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
        let stage = open_document_stage(world, doc, document)?;
        let dependencies = lunco_usd_compose::layer_dependency_paths(document.data())
            .map_err(native_query_error)?;
        let diagnostics: Vec<String> = Vec::new();
        let journal = journal_position(world, doc)?;
        let root_path = SdfPath::abs_root();

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
                        "spec_count": document.data().len(),
                    },
                    "runtime": {
                        "id": "@runtime@",
                        "persistent": "twin_policy",
                        "revision": document.runtime_revision(),
                        "spec_count": document.runtime_data().len(),
                    },
                    "view": {
                        "id": "@view@",
                        "persistent": false,
                        "revision": document.view_revision(),
                        "spec_count": document.view_data().len(),
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
                        &stage,
                        &root_path,
                        openusd::sdf::FieldKey::DefaultPrim.as_str(),
                    )?,
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
                composed_attribute_inspection(document, &stage, &path)?;
            let references = api_value!({
                "authored": {
                    "root": reference_list_api_value(document.data(), &path)?,
                    "runtime": reference_list_api_value(document.runtime_data(), &path)?,
                    "view": reference_list_api_value(document.view_data(), &path)?,
                },
                "composed": composed_reference_api_value(&stage, &path)?,
            });
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
                            &stage,
                            &path,
                            openusd::sdf::FieldKey::Kind.as_str(),
                        )?,
                        "variantSelections": metadata_stack_api_value(document, &stage, &path, openusd::sdf::FieldKey::VariantSelection.as_str())?,
                    },
                    "references": references,
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

/// Resolve one explicit edit target against the current native document stage
/// and its admitted dependency recipe. Native PCP spec sites supply authored
/// layer/path pairs; a missing prim returns `status: "missing"`.
/// Set `authored_children: true` to include direct prim child paths from the
/// explicitly selected document layer without waiting for scene projection.
pub struct ResolveUsdTargetProvider;

impl ApiQueryProvider for ResolveUsdTargetProvider {
    fn name(&self) -> &'static str {
        "ResolveUsdTarget"
    }

    fn schema(&self) -> ApiQuerySchema {
        ApiQuerySchema {
            name: self.name().to_owned(),
            description: Some(
                "Resolve an explicit USD edit target and optionally read direct authored child paths from its layer."
                    .to_owned(),
            ),
            parameters: Some(vec![
                ApiQueryParameterSchema {
                    name: "doc_id".to_owned(),
                    type_name: "u64".to_owned(),
                    required: true,
                    description: "Open USD document identity.".to_owned(),
                    allowed_values: None,
                },
                ApiQueryParameterSchema {
                    name: "path".to_owned(),
                    type_name: "usd_prim_path".to_owned(),
                    required: true,
                    description: "Absolute prim path to resolve.".to_owned(),
                    allowed_values: None,
                },
                ApiQueryParameterSchema {
                    name: "edit_target".to_owned(),
                    type_name: "usd_edit_target".to_owned(),
                    required: true,
                    description: "Document layer used to resolve the authoring target.".to_owned(),
                    allowed_values: Some(vec![
                        "@root@".to_owned(),
                        "@runtime@".to_owned(),
                        "@view@".to_owned(),
                    ]),
                },
                ApiQueryParameterSchema {
                    name: "authored_children".to_owned(),
                    type_name: "bool".to_owned(),
                    required: false,
                    description: "Include direct prim child paths from the selected layer's primChildren field.".to_owned(),
                    allowed_values: None,
                },
            ]),
            exactly_one_of: Vec::new(),
            response: Some(
                "{ doc_id, path, status, source, authored_here, edit_scope, prim_stack, authored_children?: string[] }"
                    .to_owned(),
            ),
        }
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let include_children = match params.get("authored_children") {
            None => false,
            Some(_) => match api_param_bool(params, "authored_children") {
                Some(include) => include,
                None => {
                    return query_error(
                        ApiErrorCode::DeserializationError,
                        "ResolveUsdTarget parameter `authored_children` must be a boolean",
                    );
                }
            },
        };
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
        if !edit_target.is_root() && !edit_target.is_runtime() && !edit_target.is_view() {
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
        let authored_children = if include_children {
            match authored_child_paths(document, &edit_target, &path) {
                Ok(children) => Some(children),
                Err(error) => return query_error(ApiErrorCode::DeserializationError, error),
            }
        } else {
            None
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
        // Resolve the current document layers through one native stage, with
        // the admitted dependency recipe. A derived projection may lag this
        // generation and cannot decide whether the command target exists.
        let stage = open_document_stage(world, doc, document)?;
        let resolved = (|| -> Result<_, String> {
            let prim = stage.prim(&path).map_err(|error| error.to_string())?;
            let exists = prim.is_valid().map_err(|error| error.to_string())?;
            let under_arc = prim
                .has_composition_arc()
                .map_err(|error| error.to_string())?;
            let stack = prim.prim_stack().map_err(|error| error.to_string())?;
            Ok((exists, under_arc, stack))
        })();
        let (exists, under_arc, stack) = match resolved {
            Ok(facts) => facts,
            Err(error) => {
                return query_error(
                    ApiErrorCode::InternalError,
                    format!("OpenUSD could not resolve `{raw_path}`: {error}"),
                );
            }
        };
        include_authored_children(
            query_ok(api_value!({
                "doc_id": doc.raw(), "path": raw_path, "edit_target": edit_target.as_str(),
                "status": if exists { "resolved" } else { "missing" },
                "source": "document_composition", "composed_exists": exists,
                "authored_here": authored_here, "authored_in_document": authored_in_document,
                "under_arc": under_arc,
                "edit_scope": if !exists { "missing" } else if authored_here { "authored_layer" }
                    else if authored_in_document || under_arc { "local_override" } else { "composed_read_only" },
                "prim_stack": stack.into_iter().map(|site| api_value!({ "layer": site.layer, "path": site.path.to_string() })).collect::<Vec<_>>(),
            })),
            authored_children.as_deref(),
        )
    }
}
