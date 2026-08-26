//! API query providers for the Workspace session.
//!
//! This adapter crate keeps the dependency direction clean: the data-only
//! `lunco-workspace` crate does not depend on the API layer, while API hosts
//! can install [`WorkspaceApiQueriesPlugin`] in any mode without loading a
//! simulation backend.

use bevy::prelude::*;
use lunco_api::{ApiErrorCode, ApiQueryProvider, ApiQueryRegistry, ApiResponse};
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

    fn execute(&self, world: &World, _params: &serde_json::Value) -> ApiResponse {
        let Some(ws) = world.get_resource::<WorkspaceResource>() else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "ListOpenDocuments requires WorkspacePlugin".to_string(),
            );
        };
        let active = ws.active_document;
        let items: Vec<serde_json::Value> = ws
            .documents()
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "doc_id": entry.id.raw(),
                    "title": entry.title,
                    "kind": entry.kind.to_string(),
                    "origin": origin_to_json(&entry.origin),
                    "active": Some(entry.id) == active,
                    "context_twin": entry.context_twin.map(|t| t.raw()),
                })
            })
            .collect();

        ApiResponse::ok(serde_json::json!({
            "open_documents": items,
            "count": items.len(),
            "active_doc_id": active.map(|d| d.raw()),
        }))
    }
}

struct ListRecentFilesProvider;

impl ApiQueryProvider for ListRecentFilesProvider {
    fn name(&self) -> &'static str {
        "ListRecentFiles"
    }

    fn execute(&self, world: &World, _params: &serde_json::Value) -> ApiResponse {
        let Some(ws) = world.get_resource::<WorkspaceResource>() else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "ListRecentFiles requires WorkspacePlugin".to_string(),
            );
        };

        fn entry(path: &std::path::Path) -> serde_json::Value {
            let modified_secs = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            serde_json::json!({
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
        ApiResponse::ok(serde_json::json!({
            "recent_files": files,
            "recent_twins": twins,
            "count": files.len(),
        }))
    }
}

struct ListTwinProvider;

impl ApiQueryProvider for ListTwinProvider {
    fn name(&self) -> &'static str {
        "ListTwin"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let offset = params.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        let Some(ws) = world.get_resource::<WorkspaceResource>() else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "ListTwin requires WorkspacePlugin".to_string(),
            );
        };
        let Some(twin_id) = ws.active_twin else {
            return ApiResponse::ok(serde_json::json!({ "open": false }));
        };
        let Some(twin) = ws.twin(twin_id) else {
            return ApiResponse::ok(serde_json::json!({ "open": false }));
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
            .map(|file| file_entry_to_json(file, root.as_deref()))
            .collect();

        ApiResponse::ok(serde_json::json!({
            "open": true,
            "root": root.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "files": items,
            "total": total,
            "offset": offset,
            "limit": limit,
        }))
    }
}

fn file_entry_to_json(file: &FileEntry, root: Option<&std::path::Path>) -> serde_json::Value {
    let abs = root.map(|r| r.join(&file.relative_path));
    serde_json::json!({
        "relative_path": file.relative_path.to_string_lossy(),
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

fn origin_to_json(origin: &DocumentOrigin) -> serde_json::Value {
    match origin {
        DocumentOrigin::Untitled { name } => serde_json::json!({
            "kind": "untitled",
            "name": name,
        }),
        DocumentOrigin::Bundled { filename } => serde_json::json!({
            "kind": "bundled",
            "filename": filename,
        }),
        DocumentOrigin::File { path, writable } => serde_json::json!({
            "kind": "file",
            "path": path.to_string_lossy(),
            "writable": writable,
        }),
    }
}
