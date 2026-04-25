//! API query providers for Modelica + cross-domain workspace listings.
//!
//! Registers two [`ApiQueryProvider`]s (see `lunco-api` for the trait):
//!
//! - **`ListBundled`** — embedded `assets/models/*.mo` examples. Modelica-
//!   specific; lives here because that's where the data lives.
//! - **`ListOpenDocuments`** — cross-domain workspace state. Reads
//!   [`lunco_workbench::WorkspaceResource`], so it transparently surfaces
//!   USD / SysML / Mission / Markdown documents in addition to Modelica
//!   ones — anything the Workspace layer tracks.
//!
//! ## Why `ListOpenDocuments` lives in the Modelica crate (for now)
//!
//! `lunco-workspace` does not depend on `lunco-api`, and we don't want to
//! grow that dep yet. The Modelica plugin is the one binary surface that
//! always loads when the API is present, so registering the provider here
//! is the path of least resistance. The provider's *implementation* is
//! kind-agnostic — only its registration site is here. When a non-Modelica
//! binary ships, this provider can move to `lunco-workspace` (with an
//! optional `lunco-api` feature) without changing its behaviour.

use bevy::prelude::*;
use lunco_api::{ApiErrorCode, ApiQueryProvider, ApiQueryRegistry, ApiResponse};
use lunco_doc::DocumentOrigin;
use lunco_twin::{DocumentKind, FileEntry, FileKind};
use lunco_workbench::WorkspaceResource;

use crate::models::bundled_models;

/// Plugin that registers the [`ApiQueryProvider`]s exposed by
/// `lunco-modelica`. Wired into [`crate::ui::ModelicaUiPlugin`] when
/// the `lunco-api` feature is on.
pub struct ModelicaApiQueriesPlugin;

impl Plugin for ModelicaApiQueriesPlugin {
    fn build(&self, app: &mut App) {
        // The registry resource is initialized by `ApiQueryRegistryPlugin`
        // (added by `LunCoApiPlugin`); we just push our providers in.
        let mut registry = app.world_mut().resource_mut::<ApiQueryRegistry>();
        registry.register(ListBundledProvider);
        registry.register(ListOpenDocumentsProvider);
        registry.register(ListTwinProvider);
    }
}

// ─── ListBundled ───────────────────────────────────────────────────────

struct ListBundledProvider;

impl ApiQueryProvider for ListBundledProvider {
    fn name(&self) -> &'static str {
        "ListBundled"
    }

    fn execute(
        &self,
        _world: &mut World,
        _params: &serde_json::Value,
    ) -> ApiResponse {
        let items: Vec<serde_json::Value> = bundled_models()
            .into_iter()
            .map(|m| {
                serde_json::json!({
                    "filename": m.filename,
                    "tagline": m.tagline,
                    // `bundled://Filename.mo` is the canonical id — never
                    // leak an absolute filesystem path here. On wasm32
                    // builds there is no fs path at all; the embedded
                    // source is the only addressable form.
                    "uri": format!("bundled://{}", m.filename),
                })
            })
            .collect();
        ApiResponse::ok(serde_json::json!({
            "bundled": items,
            "count": items.len(),
        }))
    }
}

// ─── ListOpenDocuments ─────────────────────────────────────────────────

struct ListOpenDocumentsProvider;

impl ApiQueryProvider for ListOpenDocumentsProvider {
    fn name(&self) -> &'static str {
        "ListOpenDocuments"
    }

    fn execute(
        &self,
        world: &mut World,
        _params: &serde_json::Value,
    ) -> ApiResponse {
        let ws = world.resource::<WorkspaceResource>();
        let active = ws.active_document;

        let items: Vec<serde_json::Value> = ws
            .documents()
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "doc_id": entry.id.raw(),
                    "title": entry.title,
                    "kind": document_kind_label(&entry.kind),
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

// ─── ListTwin ──────────────────────────────────────────────────────────

struct ListTwinProvider;

impl ApiQueryProvider for ListTwinProvider {
    fn name(&self) -> &'static str {
        "ListTwin"
    }

    fn execute(
        &self,
        world: &mut World,
        params: &serde_json::Value,
    ) -> ApiResponse {
        // Pagination params: both optional. `offset` defaults to 0,
        // `limit` defaults to "all" (no slicing). Caller supplies them
        // when a Twin folder is large enough to warrant paging; the
        // common case (<100 files) returns the whole list.
        let offset = params
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        let ws = world.resource::<WorkspaceResource>();

        // No Twin open → explicit `{open: false}`. Distinguish from
        // "Twin open but empty" (`{open: true, files: [], total: 0}`)
        // so the agent does not retry pointlessly.
        let Some(twin_id) = ws.active_twin else {
            return ApiResponse::ok(serde_json::json!({ "open": false }));
        };
        let Some(twin) = ws.twin(twin_id) else {
            // Active id points at a Twin that's no longer registered
            // — possible if the Twin was closed but `active_twin`
            // wasn't cleared. Treat as no Twin open.
            return ApiResponse::ok(serde_json::json!({ "open": false }));
        };

        let all = twin.files();
        let total = all.len();
        let end = match limit {
            Some(n) => (offset + n).min(total),
            None => total,
        };
        let slice = if offset >= total {
            &[][..]
        } else {
            &all[offset..end]
        };

        let root = twin
            .root_handle()
            .as_file_path()
            .map(|p| p.to_path_buf());
        let items: Vec<serde_json::Value> = slice
            .iter()
            .map(|f| file_entry_to_json(f, root.as_deref()))
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

fn file_entry_to_json(
    f: &FileEntry,
    root: Option<&std::path::Path>,
) -> serde_json::Value {
    let abs = root.map(|r| r.join(&f.relative_path));
    serde_json::json!({
        "relative_path": f.relative_path.to_string_lossy(),
        "absolute_path": abs.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "kind": file_kind_label(&f.kind),
    })
}

/// Compact string form of a [`FileKind`]. Documents become
/// `"document/<subkind>"` so the agent can filter by the broad category
/// or the specific domain. File references and unknowns are flat.
fn file_kind_label(k: &FileKind) -> String {
    match k {
        FileKind::Document(d) => format!("document/{}", document_kind_label(d)),
        FileKind::FileReference => "file_reference".into(),
        FileKind::Unknown => "unknown".into(),
    }
}

#[allow(dead_code)] // used by ListTwin error responses once we wire bound checks
fn err_invalid_params(msg: impl Into<String>) -> ApiResponse {
    ApiResponse::error(ApiErrorCode::DeserializationError, msg)
}

/// Stable string label for a [`DocumentKind`]. Matches the file-extension
/// taxonomy in [`lunco_twin::file_kind`]. The `Other(s)` escape hatch
/// passes the inner string through unchanged so future domain crates can
/// expose new kinds without changes here — that's how Markdown will land
/// when it graduates from `FileReference` to a real `Document` kind.
fn document_kind_label(kind: &DocumentKind) -> String {
    match kind {
        DocumentKind::Modelica => "modelica".into(),
        DocumentKind::Usd => "usd".into(),
        DocumentKind::Sysml => "sysml".into(),
        DocumentKind::Mission => "mission".into(),
        DocumentKind::Data => "data".into(),
        DocumentKind::Other(s) => s.clone(),
    }
}

/// Project a [`DocumentOrigin`] onto a JSON object. Untitled docs carry
/// only a name; File docs carry an absolute path + writability flag —
/// matches the discriminator the Twin Browser already shows in the UI.
fn origin_to_json(origin: &DocumentOrigin) -> serde_json::Value {
    match origin {
        DocumentOrigin::Untitled { name } => serde_json::json!({
            "kind": "untitled",
            "name": name,
        }),
        DocumentOrigin::File { path, writable } => serde_json::json!({
            "kind": "file",
            "path": path.to_string_lossy(),
            "writable": writable,
        }),
    }
}
