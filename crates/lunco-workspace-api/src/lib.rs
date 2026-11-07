//! API query providers for the Workspace session.
//!
//! This adapter crate keeps the dependency direction clean: the data-only
//! `lunco-workspace` crate does not depend on the API layer, while API hosts
//! can install [`WorkspaceApiQueriesPlugin`] in any mode without loading a
//! simulation backend.

use bevy::prelude::*;
use lunco_api::api_param_u64;
use lunco_api::executor::{DeferredCommandAppExt, PendingApiRequest, finish_command_result};
use lunco_api::{ApiQueryError, ApiQueryProvider, ApiQueryRegistry, ApiQueryResult};
use lunco_api_core::api_value;
use lunco_api_core::{ApiErrorCode, ApiValue};
use lunco_doc::DocumentOrigin;
use lunco_twin::{DocumentKindId, FileEntry, FileKind};
use lunco_workspace::WorkspaceResource;

#[derive(Resource, Default)]
struct PendingProjectSettingsReplies(Vec<(u64, u64)>);

fn has_project_settings_replies(pending: Res<PendingProjectSettingsReplies>) -> bool {
    !pending.0.is_empty()
}

fn capture_project_settings_reply(
    _: On<lunco_workspace::SetProjectSceneLoading>,
    active: Res<lunco_core::ActiveCommandId>,
    request: Res<PendingApiRequest>,
    mut pending: ResMut<PendingProjectSettingsReplies>,
) {
    if let Some(id) = active.get() {
        if request.correlation_id != 0 {
            pending.0.push((id, request.correlation_id));
        }
    }
}

fn capture_project_physics_settings_reply(
    _: On<lunco_workspace::SetProjectPhysicsEscapeGuard>,
    active: Res<lunco_core::ActiveCommandId>,
    request: Res<PendingApiRequest>,
    mut pending: ResMut<PendingProjectSettingsReplies>,
) {
    if let Some(id) = active.get() {
        if request.correlation_id != 0 {
            pending.0.push((id, request.correlation_id));
        }
    }
}

fn finish_project_settings_replies(world: &mut World) {
    let pending = std::mem::take(&mut world.resource_mut::<PendingProjectSettingsReplies>().0);
    let mut waiting = Vec::new();
    for (id, correlation) in pending {
        let outcome = world
            .resource::<lunco_core::CommandResults>()
            .get(id)
            .cloned();
        let (result, code) = match outcome {
            Some(lunco_core::CommandOutcome::Succeeded(ack)) => {
                (Ok(ack), ApiErrorCode::InternalError)
            }
            Some(lunco_core::CommandOutcome::Rejected(error)) => {
                (Err(error.to_string()), ApiErrorCode::CommandRejected)
            }
            Some(lunco_core::CommandOutcome::Failed(error)) => {
                (Err(error), ApiErrorCode::InternalError)
            }
            Some(lunco_core::CommandOutcome::Pending) | None => {
                waiting.push((id, correlation));
                continue;
            }
        };
        let admitted = world
            .get_resource::<lunco_workspace::scene_settings::PendingProjectSettingsWrites>()
            .is_some_and(|pending| pending.was_admitted(id));
        if !admitted {
            // The dispatcher has already answered an admission failure.
            continue;
        }
        world
            .resource_mut::<lunco_workspace::scene_settings::PendingProjectSettingsWrites>()
            .forget_admitted(id);
        finish_command_result(world, Some(id), Some(correlation), result, code);
    }
    world.resource_mut::<PendingProjectSettingsReplies>().0 = waiting;
}

/// Registers the Workspace session queries with the transport-neutral API.
pub struct WorkspaceApiQueriesPlugin;

impl Plugin for WorkspaceApiQueriesPlugin {
    fn build(&self, app: &mut App) {
        app.register_deferred_command::<lunco_workspace::SetProjectSceneLoading>()
            .register_deferred_command::<lunco_workspace::SetProjectPhysicsEscapeGuard>()
            .init_resource::<PendingProjectSettingsReplies>()
            .add_observer(capture_project_settings_reply)
            .add_observer(capture_project_physics_settings_reply)
            .add_systems(
                Update,
                finish_project_settings_replies.run_if(has_project_settings_replies),
            );
        app.init_resource::<ApiQueryRegistry>();
        let mut registry = app.world_mut().resource_mut::<ApiQueryRegistry>();
        registry.register(ListOpenDocumentsProvider);
        registry.register(ListRecentFilesProvider);
        registry.register(ListTwinProvider);
        registry.register(ReadActiveTwinContractProvider);
        registry.register(ReadActiveWorkspaceProvider);
        registry.register(SceneLoadingSettingsProvider);
        registry.register(PhysicsEscapeGuardSettingsProvider);
    }
}

struct PhysicsEscapeGuardSettingsProvider;

impl ApiQueryProvider for PhysicsEscapeGuardSettingsProvider {
    fn name(&self) -> &'static str {
        "GetPhysicsEscapeGuardSettings"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let workspace = world.get_resource::<WorkspaceResource>().ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::InternalError,
                "WorkspacePlugin is not installed",
            )
        })?;
        let global = world
            .get_resource::<lunco_workspace::PhysicsEscapeGuardSettings>()
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::InternalError,
                    "Physics escape guard settings are not installed",
                )
            })?;
        let id = workspace.active_twin;
        let twin = id.and_then(|id| workspace.twin(id));
        let project = id
            .and_then(|id| workspace.project_physics_escape_settings(id))
            .and_then(|value| value.finite_world_escape_guard);
        let twin_value = lunco_workspace::twin_physics_escape_guard_setting(
            twin.and_then(|twin| twin.manifest.as_ref()),
        )
        .map_err(|error| ApiQueryError::new(ApiErrorCode::InternalError, error))?;
        let effective = lunco_workspace::resolve_physics_escape_guard(
            global.finite_world_escape_guard,
            project,
            twin_value,
        );
        Ok(Some(api_value!({
            "global": global.finite_world_escape_guard,
            "project": project,
            "twin": twin_value,
            "effective": effective.enabled,
            "source": effective.source.as_str(),
            "project_pending": id.is_some_and(|id| workspace.project_settings_pending(id)),
            "project_error": id.and_then(|id| workspace.project_settings_error(id)),
        })))
    }
}

struct SceneLoadingSettingsProvider;

impl ApiQueryProvider for SceneLoadingSettingsProvider {
    fn name(&self) -> &'static str {
        "GetSceneLoadingSettings"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let workspace = world.get_resource::<WorkspaceResource>().ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::InternalError,
                "WorkspacePlugin is not installed",
            )
        })?;
        let global = world
            .get_resource::<lunco_workspace::SceneLoadingSettings>()
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::InternalError,
                    "Scene loading settings are not installed",
                )
            })?;
        let cli = world
            .get_resource::<lunco_workspace::SceneLoadingCli>()
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::InternalError,
                    "Scene loading CLI settings are not installed",
                )
            })?;
        let id = workspace.active_twin;
        let twin = id.and_then(|id| workspace.twin(id));
        let project = id
            .and_then(|id| workspace.project_scene_settings(id))
            .and_then(|value| value.load_default);
        let twin_value = lunco_workspace::scene_settings::twin_scene_loading_setting(
            twin.and_then(|twin| twin.manifest.as_ref()),
        )
        .map_err(|error| ApiQueryError::new(ApiErrorCode::InternalError, error))?;
        let effective = lunco_workspace::scene_settings::resolve_scene_loading(
            global.load_default,
            project,
            twin_value,
            cli.0,
        );
        let admitted = id.and_then(|id| workspace.scene_loading_decision(id));
        Ok(Some(api_value!({
            "global": global.load_default,
            "project": project,
            "twin": twin_value,
            "cli": cli.0,
            "effective": effective.load_default,
            "source": effective.source.as_str(),
            "admitted": admitted.map(|value| value.load_default),
            "admitted_source": admitted.map(|value| value.source.as_str()),
            "scene_override": id.and_then(|id| workspace.scene_override(id)),
            "project_pending": id.is_some_and(|id| workspace.project_settings_pending(id)),
            "project_error": id.and_then(|id| workspace.project_settings_error(id)),
        })))
    }
}

struct ListOpenDocumentsProvider;

impl ApiQueryProvider for ListOpenDocumentsProvider {
    fn name(&self) -> &'static str {
        "ListOpenDocuments"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let Some(ws) = world.get_resource::<WorkspaceResource>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "ListOpenDocuments requires WorkspacePlugin",
            ));
        };
        let active = ws.active_document;
        let items: Vec<ApiValue> = ws
            .documents()
            .iter()
            .map(|entry| {
                api_value!({
                    "doc_id": entry.id.raw(),
                    "title": entry.title.clone(),
                    "kind": entry.kind.to_string(),
                    "origin": origin_to_api_value(&entry.origin),
                    "dirty": entry.dirty,
                    "active": Some(entry.id) == active,
                    "runtime_context": runtime_owner_to_api_value(&entry.runtime_context),
                })
            })
            .collect();
        let count = items.len();

        Ok(Some(api_value!({
            "open_documents": items,
            "count": count,
            "active_doc_id": active.map(|d| d.raw()),
        })))
    }
}

struct ListRecentFilesProvider;

impl ApiQueryProvider for ListRecentFilesProvider {
    fn name(&self) -> &'static str {
        "ListRecentFiles"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let Some(ws) = world.get_resource::<WorkspaceResource>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "ListRecentFiles requires WorkspacePlugin",
            ));
        };

        fn entry(path: &std::path::Path) -> ApiValue {
            let modified_secs = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            api_value!({
                "path": path.display().to_string(),
                "name": path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default(),
                "exists": path.exists(),
                "modified_secs": modified_secs,
            })
        }

        let files: Vec<_> = ws.recents.loose_paths.iter().map(|p| entry(p)).collect();
        let twins: Vec<_> = ws.recents.twin_paths.iter().map(|p| entry(p)).collect();
        let count = files.len();
        Ok(Some(api_value!({
            "recent_files": files,
            "recent_twins": twins,
            "count": count,
        })))
    }
}

struct ListTwinProvider;

impl ApiQueryProvider for ListTwinProvider {
    fn name(&self) -> &'static str {
        "ListTwin"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let offset = match params.get("offset") {
            None => 0,
            Some(_) => api_param_u64(params, "offset").ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "ListTwin: `offset` must be an unsigned integer",
                )
            })? as usize,
        };
        let limit = match params.get("limit") {
            None => None,
            Some(_) => Some(api_param_u64(params, "limit").ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "ListTwin: `limit` must be an unsigned integer",
                )
            })? as usize),
        };

        let Some(ws) = world.get_resource::<WorkspaceResource>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "ListTwin requires WorkspacePlugin",
            ));
        };
        let Some(twin_id) = ws.active_twin else {
            return Ok(Some(api_value!({ "open": false })));
        };
        let Some(twin) = ws.twin(twin_id) else {
            return Ok(Some(api_value!({ "open": false })));
        };
        if twin.manifest.is_none() {
            return Ok(Some(api_value!({ "open": false })));
        }

        let all = twin.files();
        let total = all.len();
        let end = limit.map_or(total, |n| (offset + n).min(total));
        let slice = if offset >= total {
            &[][..]
        } else {
            &all[offset..end]
        };
        let root = twin.root_handle().as_file_path().map(|p| p.to_path_buf());
        let items: Vec<_> = slice
            .iter()
            .map(|file| file_entry_to_api_value(file, root.as_deref()))
            .collect();

        Ok(Some(api_value!({
            "open": true,
            "root": root.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "files": items,
            "total": total,
            "offset": offset,
            "limit": limit,
        })))
    }
}

/// Read typed Twin-owned policy inputs for Rhai workflows without routing
/// them through the asset validator's report projection.
struct ReadActiveTwinContractProvider;

struct ReadActiveWorkspaceProvider;

impl ApiQueryProvider for ReadActiveWorkspaceProvider {
    fn name(&self) -> &'static str {
        "ReadActiveWorkspace"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let ws = world.get_resource::<WorkspaceResource>().ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::InternalError,
                "ReadActiveWorkspace requires WorkspacePlugin",
            )
        })?;
        let Some((id, twin)) = ws
            .active_twin
            .and_then(|id| ws.twin(id).map(|twin| (id, twin)))
        else {
            return Ok(Some(api_value!({ "open": false })));
        };
        let kind = match ws.scope_kind(id) {
            Some(lunco_workspace::WorkspaceScopeKind::AuthoredTwin) => "twin",
            Some(lunco_workspace::WorkspaceScopeKind::Ephemeral) => "ephemeral",
            None => return Ok(Some(api_value!({ "open": false }))),
        };
        Ok(Some(
            api_value!({ "open": true, "kind": kind, "scope_id": id.raw(), "root": twin.root.to_string_lossy().into_owned() }),
        ))
    }
}

impl ApiQueryProvider for ReadActiveTwinContractProvider {
    fn name(&self) -> &'static str {
        "ReadActiveTwinContract"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let Some(ws) = world.get_resource::<WorkspaceResource>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "ReadActiveTwinContract requires WorkspacePlugin",
            ));
        };
        let Some(twin_id) = ws.active_twin else {
            return Ok(Some(api_value!({ "open": false })));
        };
        let Some(twin) = ws.twin(twin_id) else {
            return Ok(Some(api_value!({ "open": false })));
        };
        if twin.manifest.is_none() {
            return Ok(Some(api_value!({ "open": false })));
        }

        // Paths remain native PathBuf values in Twin; their textual form is
        // created only for this API/Rhai addressing boundary.
        let components: Vec<_> = twin
            .components()
            .iter()
            .map(|component| {
                api_value!({
                    "name": component.name.clone(),
                    "requirements": component.requirements.to_string_lossy().into_owned(),
                    "verification": component.verification.clone(),
                    "usd_path": component.usd_path.clone(),
                })
            })
            .collect();
        let verification_cases: Vec<_> = twin
            .verification_cases()
            .iter()
            .map(|case| {
                api_value!({
                    "name": case.name.clone(),
                    "scene": case.scene.to_string_lossy().into_owned(),
                    "script": case.script.to_string_lossy().into_owned(),
                    "verdict_channel": case.verdict_channel.clone(),
                })
            })
            .collect();
        let sysml_source_errors = twin
            .discover_sysml_sources_checked()
            .err()
            .unwrap_or_default();

        Ok(Some(api_value!({
            "open": true,
            "twin_id": twin_id.raw(),
            "manifest_present": twin.manifest.is_some(),
            "name": twin
                .manifest
                .as_ref()
                .map(|manifest| manifest.name.clone())
                .unwrap_or_else(|| twin.root.file_name().unwrap_or_default().to_string_lossy().into_owned()),
            "components": components,
            "component_errors": twin.component_registry_errors(),
            "verification_cases": verification_cases,
            "verification_errors": twin.verification_registry_errors(),
            "sysml_source_errors": sysml_source_errors,
        })))
    }
}

fn file_entry_to_api_value(file: &FileEntry, root: Option<&std::path::Path>) -> ApiValue {
    let abs = root.map(|r| r.join(&file.relative_path));
    api_value!({
        "relative_path": file.relative_path.to_string_lossy().into_owned(),
        "absolute_path": abs.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "kind": file_kind_label(&file.kind),
    })
}

fn file_kind_label(kind: &FileKind) -> String {
    match kind {
        FileKind::Document(document) => format!("document/{}", document_kind_label(document)),
        FileKind::FileReference => "file_reference".into(),
        FileKind::Unknown => "unknown".into(),
    }
}

fn document_kind_label(kind: &DocumentKindId) -> String {
    kind.to_string()
}

fn origin_to_api_value(origin: &DocumentOrigin) -> ApiValue {
    match origin {
        DocumentOrigin::Untitled { name } => api_value!({
            "kind": "untitled",
            "name": name,
        }),
        DocumentOrigin::Bundled { filename } => api_value!({
            "kind": "bundled",
            "filename": filename,
        }),
        DocumentOrigin::File { path, writable } => api_value!({
            "kind": "file",
            "path": path.to_string_lossy().into_owned(),
            "writable": *writable,
        }),
    }
}

/// External projection of the authoritative typed document runtime.
fn runtime_owner_to_api_value(owner: &lunco_workspace::DocumentRuntimeOwner) -> ApiValue {
    match owner {
        lunco_workspace::DocumentRuntimeOwner::Application => api_value!({"kind": "application"}),
        lunco_workspace::DocumentRuntimeOwner::LocalTwin(twin) => {
            api_value!({"kind": "local_twin", "twin": twin.raw()})
        }
        lunco_workspace::DocumentRuntimeOwner::Replicated(owner) => match owner {
            lunco_workspace::ReplicationOwner::Application { connection } => {
                api_value!({"kind": "replicated_application", "connection": connection.to_bits()})
            }
            lunco_workspace::ReplicationOwner::Twin { scene } => {
                api_value!({"kind": "replicated_twin", "connection": scene.connection.to_bits(), "host_twin": scene.host_twin.raw(), "authority": scene.authority.clone()})
            }
        },
    }
}
