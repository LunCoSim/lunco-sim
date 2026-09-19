//! API query providers for the Workspace session.
//!
//! This adapter crate keeps the dependency direction clean: the data-only
//! `lunco-workspace` crate does not depend on the API layer, while API hosts
//! can install [`WorkspaceApiQueriesPlugin`] in any mode without loading a
//! simulation backend.

use bevy::prelude::*;
use lunco_api::api_param_u64;
use lunco_api::{ApiQueryError, ApiQueryProvider, ApiQueryRegistry, ApiQueryResult};
use lunco_api_core::api_value;
use lunco_api_core::{ApiErrorCode, ApiValue};
use lunco_doc::DocumentOrigin;
use lunco_twin::{DocumentKindId, FileEntry, FileKind};
use lunco_workspace::WorkspaceResource;

/// Registers the Workspace session queries with the transport-neutral API.
pub struct WorkspaceApiQueriesPlugin;

impl Plugin for WorkspaceApiQueriesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ApiQueryRegistry>();
        let mut registry = app.world_mut().resource_mut::<ApiQueryRegistry>();
        registry.register(ListOpenDocumentsProvider);
        registry.register(ListRecentFilesProvider);
        registry.register(ListTwinProvider);
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
                    "context_twin": entry.context_twin.map(|t| t.raw()),
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
