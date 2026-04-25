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
use lunco_doc::{Document, DocumentOrigin};
use lunco_twin::{DocumentKind, FileEntry, FileKind};
use lunco_workbench::WorkspaceResource;

use crate::ast_extract;
use crate::models::bundled_models;
use crate::ui::panels::canvas_diagram::DrilledInClassNames;
use crate::ui::state::{CompileState, CompileStates, ModelicaDocumentRegistry};
use crate::visual_diagram::{msl_component_library, MSLComponentDef};
use lunco_doc::DocumentId;

/// Plugin that registers the [`ApiQueryProvider`]s exposed by
/// `lunco-modelica`. Wired into [`crate::ui::ModelicaUiPlugin`] when
/// the `lunco-api` feature is on.
pub struct ModelicaApiQueriesPlugin;

impl Plugin for ModelicaApiQueriesPlugin {
    fn build(&self, app: &mut App) {
        // Idempotent init: `LunCoApiPlugin::ApiQueryRegistryPlugin`
        // installs this resource too, but plugin ordering is not
        // guaranteed — if the modelica plugin builds before lunco-api,
        // mutating the registry would panic. `init_resource` is a
        // no-op when the resource already exists, so calling it here
        // makes our plugin order-independent.
        app.init_resource::<ApiQueryRegistry>();
        let mut registry = app.world_mut().resource_mut::<ApiQueryRegistry>();
        registry.register(ListBundledProvider);
        registry.register(ListOpenDocumentsProvider);
        registry.register(ListTwinProvider);
        registry.register(ListMslProvider);
        registry.register(ListCompileCandidatesProvider);
        registry.register(CompileStatusProvider);
        registry.register(GetDocumentSourceProvider);
        registry.register(DescribeModelProvider);
        registry.register(SnapshotVariablesProvider);
        registry.register(FindModelProvider);
        registry.register(SetModelInputProvider);
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

// ─── ListMsl ───────────────────────────────────────────────────────────

/// Default MSL page size if `limit` is not supplied. Picked so a single
/// page is comfortably under typical agent context budgets while still
/// being useful for prefix-narrowed queries.
const MSL_DEFAULT_LIMIT: usize = 200;
/// Hard cap on `limit`. Above this the response gets unwieldy and
/// agents should be paginating anyway.
const MSL_MAX_LIMIT: usize = 1000;

struct ListMslProvider;

impl ApiQueryProvider for ListMslProvider {
    fn name(&self) -> &'static str {
        "ListMsl"
    }

    fn execute(
        &self,
        _world: &mut World,
        params: &serde_json::Value,
    ) -> ApiResponse {
        // Pagination + filter params. All optional. `cursor` is an
        // opaque decimal string carrying the offset to start from
        // (returned by the previous page); v1 does not validate that
        // the caller's filter matches the cursor — changing filter
        // mid-pagination is undefined behaviour and the agent's
        // responsibility to avoid. Filter-hash invalidation is a v2
        // nicety (see spec 032 FR-004).
        let cursor: usize = params
            .get("cursor")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).min(MSL_MAX_LIMIT))
            .unwrap_or(MSL_DEFAULT_LIMIT);

        let filter = params.get("filter");
        let prefix = filter
            .and_then(|f| f.get("prefix"))
            .and_then(|v| v.as_str());
        let category = filter
            .and_then(|f| f.get("category"))
            .and_then(|v| v.as_str());
        let examples_only = filter
            .and_then(|f| f.get("examples_only"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // First call here will block on the JSON parse (~hundreds of
        // ms). The agent can preflight `MslStatus` to decide whether to
        // wait. We accept the blocking cost rather than returning an
        // empty result — better to be slow than to lie.
        let lib = msl_component_library();

        // Apply filters in one pass over the static slice. The filter
        // closures are cheap; no allocation until we slice the
        // matching subset for the response.
        let matched: Vec<&MSLComponentDef> = lib
            .iter()
            .filter(|c| match prefix {
                Some(p) => c.msl_path.starts_with(p),
                None => true,
            })
            .filter(|c| match category {
                Some(cat) => {
                    // Top-level package: drop the `Modelica.` prefix
                    // and take the first segment. Matches the
                    // categories the Welcome tab and palette already
                    // surface.
                    let after_modelica = c
                        .msl_path
                        .strip_prefix("Modelica.")
                        .unwrap_or(&c.msl_path);
                    let top = after_modelica.split('.').next().unwrap_or("");
                    top.eq_ignore_ascii_case(cat)
                }
                None => true,
            })
            .filter(|c| !examples_only || c.msl_path.contains(".Examples."))
            .collect();

        let total = matched.len();
        let end = (cursor + limit).min(total);
        let page_slice = if cursor >= total {
            &[][..]
        } else {
            &matched[cursor..end]
        };

        let items: Vec<serde_json::Value> = page_slice
            .iter()
            .map(|c| {
                serde_json::json!({
                    "qualified": c.msl_path,
                    "name": c.name,
                    "category": c.category,
                    "display_name": c.display_name,
                    "description": c.description,
                })
            })
            .collect();

        let next_cursor = if end < total {
            Some(end.to_string())
        } else {
            None
        };

        ApiResponse::ok(serde_json::json!({
            "items": items,
            "count": items.len(),
            "total_matched": total,
            "next_cursor": next_cursor,
            "loaded": true,
        }))
    }
}

// ─── ListCompileCandidates (spec 033 P0) ───────────────────────────────

struct ListCompileCandidatesProvider;

impl ApiQueryProvider for ListCompileCandidatesProvider {
    fn name(&self) -> &'static str {
        "ListCompileCandidates"
    }

    fn execute(
        &self,
        world: &mut World,
        params: &serde_json::Value,
    ) -> ApiResponse {
        let Some(doc_id) = parse_doc_id(params, "doc") else {
            return err_missing_field("doc");
        };
        let registry = world.resource::<ModelicaDocumentRegistry>();
        let Some(host) = registry.host(doc_id) else {
            return err_doc_not_found(doc_id);
        };
        let document = host.document();
        let Some(ast) = document.ast().result.as_ref().ok().cloned() else {
            return ApiResponse::ok(serde_json::json!({
                "doc_id": doc_id.raw(),
                "candidates": [],
                "ast_parsed": false,
            }));
        };
        let candidates: Vec<serde_json::Value> =
            ast_extract::collect_non_package_classes_qualified(&ast)
                .into_iter()
                .map(|qualified| {
                    let short = qualified
                        .rsplit('.')
                        .next()
                        .unwrap_or(&qualified)
                        .to_string();
                    serde_json::json!({
                        "qualified": qualified,
                        "short": short,
                    })
                })
                .collect();
        ApiResponse::ok(serde_json::json!({
            "doc_id": doc_id.raw(),
            "candidates": candidates,
            "count": candidates.len(),
            "ast_parsed": true,
        }))
    }
}

// ─── CompileStatus (spec 033 P0) ───────────────────────────────────────

struct CompileStatusProvider;

impl ApiQueryProvider for CompileStatusProvider {
    fn name(&self) -> &'static str {
        "CompileStatus"
    }

    fn execute(
        &self,
        world: &mut World,
        params: &serde_json::Value,
    ) -> ApiResponse {
        let Some(doc_id) = parse_doc_id(params, "doc") else {
            return err_missing_field("doc");
        };
        // Pull each piece of state in turn — `world.resource::<...>` borrows
        // are scoped to the line, so successive `let`s are fine even though
        // we touch four different resources.
        let state = world
            .get_resource::<CompileStates>()
            .map(|cs| cs.state_of(doc_id))
            .unwrap_or(CompileState::Idle);
        let drilled_in = world
            .get_resource::<DrilledInClassNames>()
            .and_then(|m| m.get(doc_id).map(str::to_string));
        // `picker_pending` mirrors the gate in `on_compile_model`: we
        // would be in the picker branch if no class is pinned and the
        // doc has 2+ non-package classes. Easier to recompute than to
        // expose CompileClassPickerState which is a UI concern.
        let registry = world.resource::<ModelicaDocumentRegistry>();
        let (candidates, has_ast) = match registry.host(doc_id) {
            Some(host) => {
                let ast = host.document().ast().result.as_ref().cloned().ok();
                let cands = ast
                    .as_ref()
                    .map(|a| ast_extract::collect_non_package_classes_qualified(a))
                    .unwrap_or_default();
                (cands, ast.is_some())
            }
            None => return err_doc_not_found(doc_id),
        };
        drop(registry);
        let picker_pending = drilled_in.is_none() && candidates.len() >= 2;

        // Error message lives on `WorkbenchState.compilation_error`. It
        // is doc-global today (one slot, last writer wins) — fine for
        // single-doc workflows; revisit if multi-doc compile lands.
        let error_message = world
            .get_resource::<crate::ui::state::WorkbenchState>()
            .and_then(|ws| ws.compilation_error.clone());

        let state_label = match state {
            CompileState::Idle => "idle",
            CompileState::Compiling => "compiling",
            CompileState::Ready => "ok",
            CompileState::Error => "error",
        };
        ApiResponse::ok(serde_json::json!({
            "doc_id": doc_id.raw(),
            "state": state_label,
            "drilled_in_class": drilled_in,
            "picker_pending": picker_pending,
            "candidates": candidates,
            "ast_parsed": has_ast,
            "error_message": error_message,
        }))
    }
}

// ─── GetDocumentSource (spec 033 P0, US 1.6) ───────────────────────────

struct GetDocumentSourceProvider;

impl ApiQueryProvider for GetDocumentSourceProvider {
    fn name(&self) -> &'static str {
        "GetDocumentSource"
    }

    fn execute(
        &self,
        world: &mut World,
        params: &serde_json::Value,
    ) -> ApiResponse {
        let Some(doc_id) = parse_doc_id(params, "doc") else {
            return err_missing_field("doc");
        };

        // Modelica docs are the only kind in the `ModelicaDocumentRegistry`
        // today; future kinds (USD, SysML) will need their own registries
        // and a fan-out by `DocumentKind` here. The cross-domain
        // workspace entry tells us which registry to query, so this
        // dispatch is centralised.
        let ws = world.resource::<WorkspaceResource>();
        let entry = ws.document(doc_id).cloned();
        drop(ws);
        let Some(entry) = entry else {
            return err_doc_not_found(doc_id);
        };

        match entry.kind {
            DocumentKind::Modelica => {
                let registry = world.resource::<ModelicaDocumentRegistry>();
                let Some(host) = registry.host(doc_id) else {
                    return err_doc_not_found(doc_id);
                };
                let document = host.document();
                ApiResponse::ok(serde_json::json!({
                    "doc_id": doc_id.raw(),
                    "kind": "modelica",
                    "source": document.source(),
                    "generation": document.generation(),
                    "dirty": document.is_dirty(),
                    "origin": origin_to_json(document.origin()),
                    "title": entry.title,
                }))
            }
            other => {
                // Other kinds don't have a content registry hooked up
                // yet — return metadata + a stub so callers can detect
                // the gap programmatically rather than guess.
                ApiResponse::error(
                    ApiErrorCode::InternalError,
                    format!(
                        "GetDocumentSource not yet implemented for kind `{}` — \
                         only Modelica docs expose source today.",
                        document_kind_label(&other),
                    ),
                )
            }
        }
    }
}

// ─── DescribeModel (spec 033 P1, structural extension) ────────────────
//
// Returns the structural picture of one class within a doc:
// class_kind, extends, components (subinstances), connections (wiring),
// plus typed inputs / parameters / outputs with units, bounds and
// defaults. The agent picks which class via the `class` parameter; the
// default is the drilled-in class or the first non-package class.
// Equations and full annotations are intentionally not surfaced here
// — those are best read via `get_document_source` when the agent
// genuinely needs them.

struct DescribeModelProvider;

impl ApiQueryProvider for DescribeModelProvider {
    fn name(&self) -> &'static str {
        "DescribeModel"
    }

    fn execute(
        &self,
        world: &mut World,
        params: &serde_json::Value,
    ) -> ApiResponse {
        let Some(doc_id) = parse_doc_id(params, "doc") else {
            return err_missing_field("doc");
        };
        let class_param = params
            .get("class")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);

        // Resolve drilled-in class as the fallback target before we
        // borrow the modelica registry — `DrilledInClassNames` is a
        // separate resource and we need both. Reading them in
        // sequence keeps the borrow checker simple.
        let drilled_in = world
            .get_resource::<DrilledInClassNames>()
            .and_then(|m| m.get(doc_id).map(str::to_string));

        let registry = world.resource::<ModelicaDocumentRegistry>();
        let Some(host) = registry.host(doc_id) else {
            return err_doc_not_found(doc_id);
        };
        let document = host.document();
        let Some(ast) = document.ast().result.as_ref().ok().cloned() else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                format!(
                    "doc {} has no parsed AST — fix any parse errors first",
                    doc_id.raw()
                ),
            );
        };

        // Class resolution: explicit `class` param > drilled-in pin >
        // first non-package class. Match by short name (the same
        // convention `compile_model.class` uses) so the caller can pass
        // either short or qualified.
        let target_class_name = class_param
            .or(drilled_in)
            .or_else(|| ast_extract::extract_model_name_from_ast(&ast));
        let Some(target_name) = target_class_name else {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!(
                    "doc {} has no non-package class to describe",
                    doc_id.raw()
                ),
            );
        };
        // The caller may pass `Foo.Bar` — try the short tail first.
        let short = target_name
            .rsplit('.')
            .next()
            .unwrap_or(&target_name);
        let Some(class) = ast_extract::find_class_by_short_name(&ast, short) else {
            let candidates =
                ast_extract::collect_non_package_classes_qualified(&ast);
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!(
                    "class `{}` not found in doc {}. Candidates: [{}]",
                    target_name,
                    doc_id.raw(),
                    candidates.join(", ")
                ),
            );
        };

        let inputs = ast_extract::extract_typed_inputs_for_class(class);
        let parameters = ast_extract::extract_typed_parameters_for_class(class);
        let outputs = ast_extract::extract_typed_outputs_for_class(class);
        let components = ast_extract::extract_components_for_class(class);
        let connections = ast_extract::extract_connections_for_class(class);
        let extends = ast_extract::extract_extends_for_class(class);

        ApiResponse::ok(serde_json::json!({
            "doc_id": doc_id.raw(),
            "class_name": short,
            "class_kind": ast_extract::class_kind_label(class),
            "extends": extends,
            "components": components.iter().map(component_info_to_json).collect::<Vec<_>>(),
            "connections": connections
                .iter()
                .map(|(from, to)| serde_json::json!({"from": from, "to": to}))
                .collect::<Vec<_>>(),
            "inputs": inputs.iter().map(typed_to_json).collect::<Vec<_>>(),
            "parameters": parameters.iter().map(typed_to_json).collect::<Vec<_>>(),
            "outputs": outputs.iter().map(typed_to_json).collect::<Vec<_>>(),
        }))
    }
}

fn typed_to_json(c: &ast_extract::TypedComponent) -> serde_json::Value {
    serde_json::json!({
        "name": c.name,
        "type": c.type_name,
        "unit": c.unit,
        "default": c.default,
        "min": c.min,
        "max": c.max,
        "description": if c.description.is_empty() { None } else { Some(c.description.clone()) },
    })
}

fn component_info_to_json(c: &ast_extract::ComponentInfo) -> serde_json::Value {
    let mods: serde_json::Map<String, serde_json::Value> = c
        .modifications
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    serde_json::json!({
        "name": c.name,
        "type": c.type_name,
        "description": if c.description.is_empty() { None } else { Some(c.description.clone()) },
        "modifications": serde_json::Value::Object(mods),
    })
}

// ─── SnapshotVariables (spec 033 P1) ───────────────────────────────────

struct SnapshotVariablesProvider;

impl ApiQueryProvider for SnapshotVariablesProvider {
    fn name(&self) -> &'static str {
        "SnapshotVariables"
    }

    fn execute(
        &self,
        world: &mut World,
        params: &serde_json::Value,
    ) -> ApiResponse {
        let Some(doc_id) = parse_doc_id(params, "doc") else {
            return err_missing_field("doc");
        };
        // Optional `names` filter — when absent, return everything.
        // Accepts either an array of strings or null/missing.
        let name_filter: Option<Vec<String>> = params.get("names").and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
        });

        // Doc must exist before we go fishing for the entity. A doc with
        // no linked entity (compile not run yet) is not an error per
        // spec 033 US 4 #3 — return an empty payload with `t: null` so
        // the agent can detect the gap programmatically.
        let registry = world.resource::<ModelicaDocumentRegistry>();
        if registry.host(doc_id).is_none() {
            return err_doc_not_found(doc_id);
        }
        let entities = registry.entities_linked_to(doc_id);
        drop(registry);
        let Some(entity) = entities.first().copied() else {
            return ApiResponse::ok(serde_json::json!({
                "doc_id": doc_id.raw(),
                "t": null,
                "compiled": false,
                "parameters": {},
                "inputs": {},
                "variables": {},
            }));
        };

        let Some(model) = world.get::<crate::ModelicaModel>(entity) else {
            return ApiResponse::ok(serde_json::json!({
                "doc_id": doc_id.raw(),
                "t": null,
                "compiled": false,
                "parameters": {},
                "inputs": {},
                "variables": {},
            }));
        };

        // Project to JSON, optionally honoring the `names` filter.
        // Filter is applied uniformly across parameters/inputs/variables
        // because the agent does not always know which bucket a name
        // lives in (e.g. `valve` is an input on this model but might be
        // a parameter on the next one).
        let in_filter = |name: &str| -> bool {
            name_filter.as_ref().is_none_or(|f| f.iter().any(|n| n == name))
        };
        let project = |map: &std::collections::HashMap<String, f64>| -> serde_json::Value {
            let inner: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .filter(|(k, _)| in_filter(k))
                .map(|(k, v)| (k.clone(), serde_json::json!(v)))
                .collect();
            serde_json::Value::Object(inner)
        };

        ApiResponse::ok(serde_json::json!({
            "doc_id": doc_id.raw(),
            "t": model.current_time,
            "compiled": true,
            "model_name": model.model_name,
            "paused": model.paused,
            "parameters": project(&model.parameters),
            "inputs": project(&model.inputs),
            "variables": project(&model.variables),
        }))
    }
}

// ─── FindModel (spec 033 P3) ───────────────────────────────────────────
//
// Cross-source fuzzy search. Scans bundled examples, the active
// Twin's documents, the MSL library, and currently-open documents,
// scores each entry against the caller's query, and returns a
// ranked list with canonical URIs. Eliminates the
// list-then-grep-then-guess pattern an agent otherwise has to
// implement client-side every time it wants to resolve "Annotated
// Rocket Engine" → `bundled://AnnotatedRocketStage.mo`.
//
// Scoring is intentionally simple: substring containment + weight
// for token starts (so `"rocket"` matches `RocketEngine.mo` higher
// than a class with "rocket" buried in its description). Anything
// fancier (token overlap, edit distance, embedding similarity) is a
// later iteration on the same provider — wire shape doesn't change.

struct FindModelProvider;

impl ApiQueryProvider for FindModelProvider {
    fn name(&self) -> &'static str {
        "FindModel"
    }

    fn execute(
        &self,
        world: &mut World,
        params: &serde_json::Value,
    ) -> ApiResponse {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if query.is_empty() {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "find_model requires a non-empty `query` string",
            );
        }
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1, 200))
            .unwrap_or(20);
        let q = query.to_ascii_lowercase();
        let mut hits: Vec<FindHit> = Vec::new();

        // ── Bundled embedded examples ────────────────────────────
        for m in bundled_models() {
            let label = m.filename.trim_end_matches(".mo").to_string();
            if let Some(score) = score(&q, &label, m.tagline) {
                hits.push(FindHit {
                    uri: format!("bundled://{}", m.filename),
                    label,
                    source: "bundled",
                    description: m.tagline.to_string(),
                    score,
                });
            }
        }

        // ── Active Twin folder ───────────────────────────────────
        let twin_files: Vec<(String, String)> = {
            let ws = world.resource::<WorkspaceResource>();
            let twin = ws.active_twin.and_then(|id| ws.twin(id));
            let root = twin
                .and_then(|t| t.root_handle().as_file_path().map(|p| p.to_path_buf()));
            twin.map(|t| {
                t.files()
                    .iter()
                    .map(|f| {
                        let abs = root
                            .as_ref()
                            .map(|r| r.join(&f.relative_path).to_string_lossy().into_owned())
                            .unwrap_or_else(|| f.relative_path.to_string_lossy().into_owned());
                        let label = f
                            .relative_path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_string();
                        (abs, label)
                    })
                    .collect()
            })
            .unwrap_or_default()
        };
        for (abs, label) in twin_files {
            if let Some(score) = score(&q, &label, &abs) {
                hits.push(FindHit {
                    uri: abs.clone(),
                    label,
                    source: "twin",
                    description: abs,
                    score,
                });
            }
        }

        // ── MSL library ──────────────────────────────────────────
        // Scan the cached library if it's been initialized; force
        // initialization here would block on the JSON parse, which
        // is acceptable since the result is cached after the first
        // call. Subsequent finds hit the warm cache.
        for c in msl_component_library() {
            if let Some(score) = score(&q, &c.display_name, &c.msl_path) {
                hits.push(FindHit {
                    uri: c.msl_path.clone(),
                    label: c.display_name.clone(),
                    source: "msl",
                    description: c
                        .description
                        .clone()
                        .unwrap_or_else(|| c.msl_path.clone()),
                    score,
                });
            }
        }

        // ── Currently-open documents ─────────────────────────────
        let open_docs: Vec<(u64, String, String)> = {
            let ws = world.resource::<WorkspaceResource>();
            ws.documents()
                .iter()
                .map(|e| {
                    let uri = match &e.origin {
                        DocumentOrigin::File { path, .. } => {
                            path.to_string_lossy().into_owned()
                        }
                        DocumentOrigin::Untitled { name } => format!("mem://{name}"),
                    };
                    (e.id.raw(), e.title.clone(), uri)
                })
                .collect()
        };
        for (_id, title, uri) in open_docs {
            if let Some(score) = score(&q, &title, &uri) {
                hits.push(FindHit {
                    uri: uri.clone(),
                    label: title,
                    source: "open",
                    description: uri,
                    score,
                });
            }
        }

        // Sort by score desc, then label asc for stable tie-breaking.
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.label.cmp(&b.label))
        });
        hits.truncate(limit);

        let total_matched = hits.len();
        let items: Vec<serde_json::Value> = hits
            .into_iter()
            .map(|h| {
                serde_json::json!({
                    "uri": h.uri,
                    "label": h.label,
                    "source": h.source,
                    "description": h.description,
                    "score": h.score,
                })
            })
            .collect();
        ApiResponse::ok(serde_json::json!({
            "query": query,
            "items": items,
            "count": total_matched,
        }))
    }
}

#[derive(Debug)]
struct FindHit {
    uri: String,
    label: String,
    source: &'static str,
    description: String,
    score: f32,
}

/// Substring-with-weighted-start scoring. Returns `None` for misses
/// so the caller can `filter_map` the negative cases away cheaply.
///
/// Scoring (pick the highest among label + secondary):
/// - `1.0` — exact match (case-insensitive) on the label
/// - `0.9` — label *starts* with `q`
/// - `0.7` — label contains `q` as a whole word at a token boundary
/// - `0.5` — label contains `q` anywhere
/// - `0.3` — secondary (description / path) contains `q`
///
/// All comparisons are lowercase. `q` is the already-lowercased query.
fn score(q: &str, label: &str, secondary: &str) -> Option<f32> {
    if q.is_empty() {
        return None;
    }
    let label_lc = label.to_ascii_lowercase();
    if label_lc == q {
        return Some(1.0);
    }
    if label_lc.starts_with(q) {
        return Some(0.9);
    }
    // Token boundary: `q` follows a non-alphanumeric char in the label.
    if label_lc
        .match_indices(q)
        .any(|(idx, _)| idx == 0 || !label_lc.as_bytes()[idx - 1].is_ascii_alphanumeric())
    {
        return Some(0.7);
    }
    if label_lc.contains(q) {
        return Some(0.5);
    }
    if secondary.to_ascii_lowercase().contains(q) {
        return Some(0.3);
    }
    None
}

// ─── SetModelInput (spec 033 P2 — error-reporting variant) ─────────────
//
// Wraps the same `apply_set_model_input` mutation the
// `SetModelInput` Reflect-event observer uses, but returns a
// structured `{ok, error?}` payload instead of fire-and-forget. The
// executor's provider check runs before reflect dispatch, so an API
// caller hitting `command="SetModelInput"` lands here; in-process
// triggers (GUI panels, tests) keep going through the Reflect event.
// Both paths converge on the shared mutation helper, so they can't
// drift.

struct SetModelInputProvider;

impl ApiQueryProvider for SetModelInputProvider {
    fn name(&self) -> &'static str {
        "SetModelInput"
    }

    fn execute(
        &self,
        world: &mut World,
        params: &serde_json::Value,
    ) -> ApiResponse {
        // Wire-format mirror of the Reflect event:
        // `{ doc: u64, name: String, value: f64 }`. `doc == 0` means
        // "active document" — same convention the event uses.
        let doc = params
            .get("doc")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let Some(name) = params
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            return err_missing_field("name");
        };
        let Some(value) = params.get("value").and_then(|v| v.as_f64()) else {
            return err_missing_field("value");
        };

        match crate::ui::commands::apply_set_model_input(world, doc, &name, value) {
            Ok(resolved_doc) => ApiResponse::ok(serde_json::json!({
                "ok": true,
                "doc": resolved_doc.raw(),
                "name": name,
                "value": value,
            })),
            Err(e) => {
                use crate::ui::commands::SetModelInputError;
                let code = match e {
                    SetModelInputError::NoActiveDocument
                    | SetModelInputError::NoLinkedEntity { .. }
                    | SetModelInputError::EntityMissingModel { .. } => {
                        ApiErrorCode::EntityNotFound
                    }
                    SetModelInputError::UnknownInput { .. } => {
                        ApiErrorCode::DeserializationError
                    }
                };
                ApiResponse::error(code, e.message())
            }
        }
    }
}

// ─── Provider helpers ──────────────────────────────────────────────────

fn parse_doc_id(params: &serde_json::Value, field: &str) -> Option<DocumentId> {
    params
        .get(field)
        .and_then(|v| v.as_u64())
        .filter(|id| *id != 0)
        .map(DocumentId::new)
}

fn err_missing_field(field: &str) -> ApiResponse {
    ApiResponse::error(
        ApiErrorCode::DeserializationError,
        format!("missing or invalid `{field}` field (must be a non-zero u64 doc_id)"),
    )
}

fn err_doc_not_found(doc_id: DocumentId) -> ApiResponse {
    ApiResponse::error(
        ApiErrorCode::EntityNotFound,
        format!("doc_id {} not in registry", doc_id.raw()),
    )
}

#[allow(dead_code)] // available for providers that want to surface validation errors
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
