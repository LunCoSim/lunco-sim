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
use lunco_api::{ApiQueryProvider, ApiQueryRegistry, ApiResponse};
use lunco_doc::DocumentOrigin;
use lunco_twin::DocumentKind;
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
