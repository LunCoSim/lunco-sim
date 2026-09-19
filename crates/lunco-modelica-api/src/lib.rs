//! API query providers for Modelica documents and simulation state.
//!
//! Registers the Modelica-owned [`ApiQueryProvider`] implementations (see
//! `lunco-api` for the trait). Workspace queries remain owned by
//! `lunco-workspace-api`.
//!
//! - **`ListBundled`** — runtime `assets/models/*.mo` examples and the complete
//!   source inventory used by authored validation. Modelica-specific; lives here
//!   because that's where the data lives.

/// Transport-free Modelica document edit commands and their observers.
pub mod edit;

use bevy::prelude::*;
use lunco_api::queries::{api_param_array, api_param_u64, ApiQueryError, ApiQueryResult};
use lunco_api::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api_core::{api_value, ApiErrorCode, ApiValue};
use lunco_doc::{Document, DocumentOrigin};
use lunco_modelica_runtime::ModelicaModel;
use lunco_workspace::WorkspaceResource;

use lunco_experiments::{ExperimentId, ExperimentRegistry, RunStatus};
use lunco_modelica_core::models::bundled_models;
use lunco_modelica_runner::ExperimentSources;
// `DrilledInClassNames` reads migrated to
// `lunco_modelica_core::sim_default::drilled_class_for_doc`.
use lunco_doc::CompileState;
use lunco_doc::DocumentId;
use lunco_doc_bevy::{DocumentDiagnostics, DocumentRegistry};
use lunco_modelica_document::ModelicaDocument;
use lunco_modelica_index::visual_diagram::library_class_library;

type ModelicaDocuments = DocumentRegistry<ModelicaDocument>;

fn query_ok(value: ApiValue) -> ApiQueryResult {
    Ok(Some(value))
}

fn query_error(code: ApiErrorCode, message: impl Into<String>) -> ApiQueryResult {
    Err(ApiQueryError::new(code, message))
}

fn is_generated_document(document: &ModelicaDocument) -> bool {
    lunco_modelica_runtime::generated_source::is_generated_origin(document.origin())
}

/// Plugin that registers the Modelica query providers and edit commands. Hosts
/// add this capability alongside the Modelica compiler plugin when they expose
/// the transport-free Modelica API surface.
pub struct ModelicaApiQueriesPlugin;

impl Plugin for ModelicaApiQueriesPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<edit::ModelicaApiEditPlugin>() {
            app.add_plugins(edit::ModelicaApiEditPlugin);
        }
        // Idempotent init: `LunCoApiPlugin::ApiQueryRegistryPlugin`
        // installs this resource too, but plugin ordering is not
        // guaranteed — if the modelica plugin builds before lunco-api,
        // mutating the registry would panic. `init_resource` is a
        // no-op when the resource already exists, so calling it here
        // makes our plugin order-independent.
        app.init_resource::<ApiQueryRegistry>();
        let mut registry = app.world_mut().resource_mut::<ApiQueryRegistry>();
        registry.register(ListBundledProvider);
        registry.register(ListSolversProvider);
        registry.register(ListLibraryProvider);
        registry.register(ListCompileCandidatesProvider);
        registry.register(QueryExperimentBoundsProvider);
        registry.register(CompileStatusProvider);
        registry.register(RunStatusProvider);
        registry.register(ListRunsProvider);
        registry.register(GetExperimentResultProvider);
        registry.register(GetDocumentSourceProvider);
        registry.register(DescribeModelProvider);
        registry.register(SnapshotVariablesProvider);
        registry.register(FindModelProvider);
        registry.register(GetShareLinkProvider);
    }
}

// ─── ListBundled ───────────────────────────────────────────────────────

struct ListBundledProvider;

impl ApiQueryProvider for ListBundledProvider {
    fn name(&self) -> &'static str {
        "ListBundled"
    }

    fn execute(&self, _world: &World, _params: &ApiValue) -> ApiQueryResult {
        let models = match bundled_models() {
            Ok(models) => models,
            Err(error) => return query_error(ApiErrorCode::InternalError, error),
        };
        let items: Vec<ApiValue> = models
            .iter()
            .map(|m| {
                api_value!({
                    "filename": m.filename.clone(),
                    "tagline": m.tagline.clone(),
                    // `bundled://Filename.mo` is the canonical id — never
                    // leak an absolute filesystem path here. On wasm32
                    // builds there is no filesystem path at all; the runtime
                    // source is the only addressable form.
                    "uri": format!("bundled://{}", m.filename),
                })
            })
            .collect();
        let mut source_paths = models
            .iter()
            .map(|model| model.filename.to_string())
            .collect::<Vec<_>>();
        let packages = match lunco_assets_runtime::models::package_roots() {
            Ok(packages) => packages,
            Err(error) => return query_error(ApiErrorCode::InternalError, error),
        };
        for package in packages {
            let files = match lunco_assets_runtime::models::package_files(&package) {
                Ok(files) => files,
                Err(error) => return query_error(ApiErrorCode::InternalError, error),
            };
            source_paths.extend(files.into_iter().map(|(path, _)| path));
        }
        source_paths.sort();
        source_paths.dedup();
        let sources = source_paths
            .into_iter()
            .map(|path| {
                api_value!({
                    "path": path.clone(),
                    "uri": lunco_assets_core::engine_model_asset_uri(&path),
                })
            })
            .collect::<Vec<_>>();
        let count = items.len();
        query_ok(api_value!({
            "bundled": items,
            "count": count,
            "sources": sources,
        }))
    }
}

// ─── ListSolvers ───────────────────────────────────────────────────────

/// The solver registry, verbatim: ids, capabilities and rank.
///
/// This is the vocabulary `RunExperiment`/`FastRunActiveModel` accept in their
/// `solver` field, and it is the ONLY place that vocabulary is defined — the
/// registry replaced a closed `SolverChoice` enum, so pre-registry spellings
/// (`"rk_like"`, `"RkLike"`) name nothing and are refused rather than aliased.
/// A caller that hardcoded one needs this list, not a guess.
///
/// Capabilities are reported because they explain the refusals: a run that asks
/// for a backend without `usable_live` inside the frame loop is rejected by
/// `solver::resolve`, and the field says why before the run is attempted.
struct ListSolversProvider;

impl ApiQueryProvider for ListSolversProvider {
    fn name(&self) -> &'static str {
        "ListSolvers"
    }

    fn execute(&self, _world: &World, _params: &ApiValue) -> ApiQueryResult {
        // The builtin backends register on first use rather than at plugin
        // build, so a query that arrives before any run would otherwise see an
        // empty registry and report "no solvers exist".
        lunco_modelica_execution::ensure_builtin_solvers();
        let items: Vec<ApiValue> = lunco_experiments::solver::registered()
            .into_iter()
            .map(|s| {
                api_value!({
                    "id": s.id.to_string(),
                    "label": s.label,
                    "rank": s.rank,
                    "usable_live": s.caps.usable_live,
                    "fixed_step": s.caps.fixed_step,
                    "deterministic": s.caps.deterministic,
                })
            })
            .collect();
        let count = items.len();
        query_ok(api_value!({
            "solvers": items,
            "count": count,
        }))
    }
}

// ─── ListLibrary ───────────────────────────────────────────────────────────

/// Default source library page size if `limit` is not supplied. Picked so a single
/// page is comfortably under typical agent context budgets while still
/// being useful for prefix-narrowed queries.
const LIBRARY_DEFAULT_LIMIT: usize = 200;
/// Hard cap on `limit`. Above this the response gets unwieldy and
/// agents should be paginating anyway.
const LIBRARY_MAX_LIMIT: usize = 1000;

struct ListLibraryProvider;

impl ApiQueryProvider for ListLibraryProvider {
    fn name(&self) -> &'static str {
        "ListLibrary"
    }

    fn execute(&self, _world: &World, params: &ApiValue) -> ApiQueryResult {
        // Pagination + filter params. All optional. `cursor` is an
        // opaque decimal string carrying the offset to start from
        // (returned by the previous page); v1 does not validate that
        // the caller's filter matches the cursor — changing filter
        // mid-pagination is undefined behaviour and the agent's
        // responsibility to avoid. Filter-hash invalidation is a v2
        // nicety (see spec 032 FR-004).
        let cursor = match params.get("cursor") {
            None => 0,
            Some(ApiValue::Str(value)) => match value.parse::<usize>() {
                Ok(cursor) => cursor,
                Err(_) => {
                    return query_error(
                        ApiErrorCode::DeserializationError,
                        "ListLibrary: `cursor` must be a decimal offset string",
                    );
                }
            },
            Some(_) => {
                return query_error(
                    ApiErrorCode::DeserializationError,
                    "ListLibrary: `cursor` must be a decimal offset string",
                );
            }
        };

        let limit = match params.get("limit") {
            None => LIBRARY_DEFAULT_LIMIT,
            Some(_) => match api_param_u64(params, "limit") {
                Some(limit) => limit.min(LIBRARY_MAX_LIMIT as u64) as usize,
                None => {
                    return query_error(
                        ApiErrorCode::DeserializationError,
                        "ListLibrary: `limit` must be an unsigned integer",
                    );
                }
            },
        };

        let filter = params.get("filter");
        if filter.is_some_and(|filter| !matches!(filter, ApiValue::Map(_))) {
            return query_error(
                ApiErrorCode::DeserializationError,
                "ListLibrary: `filter` must be a map",
            );
        }
        let prefix = match filter.and_then(|value| value.get("prefix")) {
            None => None,
            Some(ApiValue::Str(value)) => Some(value.as_str()),
            Some(_) => {
                return query_error(
                    ApiErrorCode::DeserializationError,
                    "ListLibrary: `filter.prefix` must be a string",
                );
            }
        };
        let category = match filter.and_then(|value| value.get("category")) {
            None => None,
            Some(ApiValue::Str(value)) => Some(value.as_str()),
            Some(_) => {
                return query_error(
                    ApiErrorCode::DeserializationError,
                    "ListLibrary: `filter.category` must be a string",
                );
            }
        };
        let examples_only = match filter.and_then(|f| f.get("examples_only")) {
            None => false,
            Some(ApiValue::Bool(value)) => *value,
            Some(_) => {
                return query_error(
                    ApiErrorCode::DeserializationError,
                    "ListLibrary: `filter.examples_only` must be a boolean",
                );
            }
        };

        // The first call may block on index initialization. The provider waits
        // for the authoritative index rather than returning an empty result.
        let lib = library_class_library();

        // Apply filters in one pass over the static slice. The filter
        // closures are cheap; no allocation until we slice the
        // matching subset for the response.
        let matched: Vec<&lunco_modelica_index::index::ClassEntry> = lib
            .iter()
            .filter(|c| match prefix {
                Some(p) => c.name.starts_with(p),
                None => true,
            })
            .filter(|c| match category {
                Some(cat) => {
                    // The first package segment after the source-root name
                    // is the category. This works for every installed source
                    // root, regardless of its authored top-level name.
                    let top = c.name.split('.').nth(1).unwrap_or("");
                    top.eq_ignore_ascii_case(cat)
                }
                None => true,
            })
            .filter(|c| !examples_only || c.is_example())
            .collect();

        let total = matched.len();
        let end = (cursor + limit).min(total);
        let page_slice = if cursor >= total {
            &[][..]
        } else {
            &matched[cursor..end]
        };

        let items: Vec<ApiValue> = page_slice
            .iter()
            .map(|c| {
                api_value!({
                    "qualified": c.name.clone(),
                    "name": c.short_name(),
                    "category": c.category.clone(),
                    "display_name": c.short_name(),
                    "description": if c.description.is_empty() { None } else { Some(c.description.clone()) },
                })
            })
            .collect();

        let next_cursor = if end < total {
            Some(end.to_string())
        } else {
            None
        };

        let count = items.len();
        query_ok(api_value!({
            "items": items,
            "count": count,
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

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(doc_id) = parse_doc_id(params, "doc_id") else {
            return err_missing_field("doc_id");
        };
        let registry = world.resource::<ModelicaDocuments>();
        let Some(host) = registry.host(doc_id) else {
            return err_doc_not_found(doc_id);
        };
        // Read non-package classes from the per-doc Index — sees
        // optimistic structural patches (ClassAdded / ClassRemoved)
        // and avoids walking the AST. Matches the same convention
        // the panels use.
        let candidates: Vec<ApiValue> = host
            .document()
            .index()
            .simulation_candidates()
            .into_iter()
            .map(|qualified| {
                let short = qualified
                    .rsplit('.')
                    .next()
                    .unwrap_or(&qualified)
                    .to_string();
                api_value!({
                    "qualified": qualified,
                    "short": short,
                })
            })
            .collect();
        let count = candidates.len();
        query_ok(api_value!({
            "doc_id": doc_id.raw(),
            "candidates": candidates,
            "count": count,
            "ast_parsed": true,
        }))
    }
}

// ─── QueryExperimentBounds ─────────────────────────────────────────────

/// Reports, per non-package class in a document, the simulation bounds the
/// Fast Run popup / Experiments Setup would use — and *where they come
/// from*. Answers "why does it propose 10 s?": a class with no
/// `experiment(...)` annotation (or one missing `StopTime`) resolves to the
/// 10 s fallback, while an annotated class surfaces its authored `StopTime`.
///
/// Params: `{doc_id}` (required), `{class}` (optional — short or qualified
/// name; default = every non-package class).
struct QueryExperimentBoundsProvider;

impl ApiQueryProvider for QueryExperimentBoundsProvider {
    fn name(&self) -> &'static str {
        "QueryExperimentBounds"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(doc_id) = parse_doc_id(params, "doc_id") else {
            return err_missing_field("doc_id");
        };
        let class_filter = match params.get("class") {
            None => None,
            Some(ApiValue::Str(value)) => Some(value.clone()),
            Some(_) => {
                return query_error(
                    ApiErrorCode::DeserializationError,
                    "ListCompileCandidates: `class` must be a string",
                );
            }
        }
        .filter(|value| !value.is_empty());

        // Snapshot (class-name, has-annotation) up front, then drop the
        // registry borrow before calling the resolve helpers — they take
        // `&World` and would otherwise alias the registry borrow.
        let class_list: Vec<(String, bool)> = {
            let registry = world.resource::<ModelicaDocuments>();
            let Some(host) = registry.host(doc_id) else {
                return err_doc_not_found(doc_id);
            };
            host.document()
                .index()
                .classes
                .values()
                .filter(|c| !matches!(c.kind, lunco_modelica_index::index::ClassKind::Package))
                .filter(|c| {
                    class_filter.as_ref().is_none_or(|f| {
                        c.name == *f || c.name.rsplit('.').next() == Some(f.as_str())
                    })
                })
                .map(|c| (c.name.clone(), c.experiment.is_some()))
                .collect()
        };

        if class_list.is_empty() {
            return query_error(
                ApiErrorCode::EntityNotFound,
                "no matching non-package class in document".to_string(),
            );
        }

        use lunco_experiments::{ExperimentRunner, ModelRef};
        use lunco_modelica_runner::{bounds_from_annotation, resolve_setup_bounds};

        let classes: Vec<ApiValue> = class_list
            .into_iter()
            .map(|(name, has_ann)| {
                let mref = ModelRef(name.clone());
                let annotation = bounds_from_annotation(world, doc_id, &mref);
                let resolved = resolve_setup_bounds(world, doc_id, &mref);
                // Mirror `resolve_setup_bounds` exactly. Keep the provenance
                // label beside the canonical resolver until the API can return
                // its typed result directly.
                let has_draft = world
                    .get_resource::<lunco_modelica_runner::ExperimentDrafts>()
                    .and_then(|d| {
                        d.get(doc_id, &mref)
                            .and_then(|dr| dr.bounds_override.clone())
                    })
                    .is_some();
                let has_runner_cache = world
                    .get_resource::<lunco_modelica_runner::ModelicaRunnerResource>()
                    .and_then(|r| r.0.default_bounds(&mref))
                    .is_some();
                let source = if has_draft {
                    "draft_override"
                } else if annotation.is_some() {
                    "annotation"
                } else if has_runner_cache {
                    "runner_cache"
                } else {
                    "fallback_1s"
                };
                api_value!({
                    "class": name,
                    "has_experiment_annotation": has_ann,
                    "annotation_bounds": annotation.as_ref().map(bounds_api_value),
                    "resolved_bounds": bounds_api_value(&resolved),
                    "source": source,
                })
            })
            .collect();

        let count = classes.len();
        query_ok(api_value!({
            "doc_id": doc_id.raw(),
            "classes": classes,
            "count": count,
        }))
    }
}

/// Compact typed API value for a [`lunco_experiments::RunBounds`] (the simulation
/// time window + sampling/tolerance), used by `QueryExperimentBounds`.
fn bounds_api_value(b: &lunco_experiments::RunBounds) -> ApiValue {
    api_value!({
        "t_start": b.t_start,
        "t_end": b.t_end,
        "dt": b.dt,
        "n_intervals": b.n_intervals,
        "tolerance": b.tolerance,
    })
}

// ─── CompileStatus (spec 033 P0) ───────────────────────────────────────

struct CompileStatusProvider;

impl ApiQueryProvider for CompileStatusProvider {
    fn name(&self) -> &'static str {
        "CompileStatus"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(doc_id) = parse_doc_id(params, "doc_id") else {
            return err_missing_field("doc_id");
        };
        // Pull each piece of state in turn — `world.resource::<...>` borrows
        // are scoped to the line, so successive `let`s are fine even though
        // we touch four different resources.
        let state = world
            .get_resource::<DocumentDiagnostics>()
            .map(|cs| cs.state_of(doc_id))
            .unwrap_or(CompileState::Idle);
        // of going through the `DrilledInClassNames` cache. The
        // helper falls back to first-tab-for-doc when no
        // `TabRenderContext` is in scope (which is the case here —
        // API queries run off-render).
        let drilled_in = lunco_modelica_core::sim_default::drilled_class_for_doc(world, doc_id);
        // `picker_pending` mirrors the gate in `on_compile_model`: we
        // would be in the picker branch if no class is pinned and the
        // doc has 2+ non-package classes. Easier to recompute than to
        // expose CompileClassPickerState which is a UI concern.
        let registry = world.resource::<ModelicaDocuments>();
        let (candidates, preferred_count, has_ast) = match registry.host(doc_id) {
            Some(host) => {
                let doc_ref = host.document();
                let has_ast = !doc_ref.ast().has_errors();
                // Non-package class qualified names from the per-doc
                // Index — sees optimistic patches and avoids walking
                // the AST. Same pattern as the candidates query above.
                let index = doc_ref.index();
                // Rank once, derive both the candidate list and the
                // preferred-tier count from it (CQ-206 — was two full
                // index walks via simulation_candidates +
                // simulation_preferred_count).
                let ranked = index.ranked_simulation_candidates();
                let top_level =
                    lunco_modelica_index::index::ModelicaIndex::preferred_count_of(&ranked);
                let cands: Vec<String> = ranked.into_iter().map(|(_, n)| n).collect();
                (cands, top_level, has_ast)
            }
            None => return err_doc_not_found(doc_id),
        };
        // `picker_pending` is meaningful only when the doc is in the
        // `idle` state — i.e. nothing is compiling yet and a *future*
        // compile with no `class` argument would open the GUI picker.
        // Once a compile is in flight, has succeeded, or has errored,
        // the caller already provided enough context (or the picker
        // was bypassed), so reporting `true` here would be misleading.
        // Mirror the gate in `on_compile_model`: the picker only opens
        // when there's no obvious top-level root (i.e. !=1 top-level
        // candidates) AND the doc has 2+ candidates total. With one
        // clear root the compile path auto-picks it without prompting.
        let picker_pending = matches!(state, CompileState::Idle)
            && drilled_in.is_none()
            && preferred_count != 1
            && candidates.len() >= 2;

        let error_message = world
            .get_resource::<lunco_doc_bevy::DocumentDiagnostics>()
            .and_then(|cs| cs.error_message(doc_id).map(str::to_string));

        // Live run-state, read from the `ModelicaModel` for this doc's
        // entity (if one exists yet). Lets a single CompileStatus call
        // answer "is it compiled / running / stale?" without a second
        // entity query. Defaults (no entity) report uncompiled + stale.
        // One `ModelicaDocuments` borrow yields both the doc
        // generation and the linked run entity (CQ-216 — was two fetches).
        let (doc_generation, run_entity) = world
            .get_resource::<ModelicaDocuments>()
            .map(|r| {
                let generation = r
                    .host(doc_id)
                    .map(|h| h.document().generation_owned())
                    .unwrap_or(0);
                let entity = r.entities_linked_to(doc_id).into_iter().next();
                (generation, entity)
            })
            .unwrap_or((0, None));
        let (is_compiled, is_compiling, paused, running, run_stale, current_time) = run_entity
            .and_then(|e| world.get::<ModelicaModel>(e))
            .map(|m| {
                let stale = !m.is_compiled || m.compiled_generation != doc_generation;
                (
                    m.is_compiled,
                    m.is_compiling,
                    m.paused,
                    m.is_compiled && !m.paused,
                    stale,
                    m.current_time,
                )
            })
            .unwrap_or((false, false, false, false, true, 0.0));

        // Convenience pointer to the most recent run for this doc.
        // Run errors live on `RunStatus::Failed`, not here — this is
        // just a hint so a single CompileStatus call can tell the
        // caller "there's something to look at on the run side".
        let latest_run = latest_run_for_doc(world, doc_id);

        let state_label = match state {
            CompileState::Idle => "idle",
            CompileState::Compiling => "compiling",
            CompileState::Ready => "ok",
            CompileState::Error => "error",
        };
        query_ok(api_value!({
            "doc_id": doc_id.raw(),
            "state": state_label,
            "drilled_in_class": drilled_in,
            "picker_pending": picker_pending,
            "candidates": candidates,
            "ast_parsed": has_ast,
            "error_message": error_message,
            "latest_run": latest_run,
            "is_compiled": is_compiled,
            "is_compiling": is_compiling,
            "paused": paused,
            "running": running,
            "stale": run_stale,
            "current_time": current_time,
        }))
    }
}

// ─── RunStatus / ListRuns ──────────────────────────────────────────────

struct RunStatusProvider;

impl ApiQueryProvider for RunStatusProvider {
    fn name(&self) -> &'static str {
        "RunStatus"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(id) = parse_experiment_id(params, "experiment_id") else {
            return err_missing_field("experiment_id");
        };
        let sources_doc = world
            .get_resource::<ExperimentSources>()
            .and_then(|s| s.0.get(&id).copied().map(|d| d.raw()));
        let Some(registry) = world.get_resource::<ExperimentRegistry>() else {
            return query_error(
                ApiErrorCode::EntityNotFound,
                "experiment registry not installed".to_string(),
            );
        };
        let Some(exp) = registry.get(id) else {
            return query_error(
                ApiErrorCode::EntityNotFound,
                format!("experiment {id:?} not in registry"),
            );
        };
        query_ok(run_summary(exp, sources_doc))
    }
}

struct ListRunsProvider;

impl ApiQueryProvider for ListRunsProvider {
    fn name(&self) -> &'static str {
        "ListRuns"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        // Optional `doc_id` filter — when absent, list every run in the
        // registry (across docs/twins).
        let filter_doc = match params.get("doc_id") {
            None => None,
            Some(_) => match parse_doc_id(params, "doc_id") {
                Some(doc_id) => Some(doc_id),
                None => {
                    return query_error(
                        ApiErrorCode::DeserializationError,
                        "ListRuns: `doc_id` must be a non-zero unsigned integer",
                    );
                }
            },
        };
        // Snapshot the sources map into an id→doc table we can reuse
        // per row without re-borrowing the resource.
        let id_to_doc: std::collections::HashMap<ExperimentId, u64> = world
            .get_resource::<ExperimentSources>()
            .map(|s| s.0.iter().map(|(k, v)| (*k, v.raw())).collect())
            .unwrap_or_default();
        let Some(registry) = world.get_resource::<ExperimentRegistry>() else {
            return query_ok(api_value!({"runs": [], "count": 0}));
        };
        let mut rows: Vec<ApiValue> = Vec::new();
        for exp in registry.iter_all() {
            let exp_doc = id_to_doc.get(&exp.id).copied();
            if let Some(want) = filter_doc {
                if exp_doc != Some(want.raw()) {
                    continue;
                }
            }
            rows.push(run_summary(exp, exp_doc));
        }
        // Newest first.
        rows.sort_by(|a, b| {
            let ka = a
                .get("created_at_ms")
                .and_then(ApiValue::as_i64)
                .unwrap_or(0);
            let kb = b
                .get("created_at_ms")
                .and_then(ApiValue::as_i64)
                .unwrap_or(0);
            kb.cmp(&ka)
        });
        // Read len before moving `rows` into the payload — no need to
        // clone the whole vec just to also report its count (CQ-206).
        let count = rows.len();
        query_ok(api_value!({
            "runs": rows,
            "count": count,
        }))
    }
}

/// Build the `latest_run` pointer attached to `CompileStatus`. Picks
/// the most-recently-created experiment whose source doc matches.
/// Returns `null` when no run has been dispatched for the doc.
fn latest_run_for_doc(world: &World, doc_id: DocumentId) -> ApiValue {
    // CQ-114: reuse the most-recent-experiment-for-doc selection in
    // [`latest_experiment_id_for_doc`] instead of duplicating the scan.
    let Some(id) = latest_experiment_id_for_doc(world, doc_id) else {
        return ApiValue::Unit;
    };
    let Some(registry) = world.get_resource::<ExperimentRegistry>() else {
        return ApiValue::Unit;
    };
    match registry.get(id) {
        Some(exp) => api_value!({
            "experiment_id": exp.id.0.to_string(),
            "name": exp.name.clone(),
            "state": run_state_label(&exp.status),
        }),
        None => ApiValue::Unit,
    }
}

fn run_state_label(s: &RunStatus) -> &'static str {
    match s {
        RunStatus::Pending => "pending",
        RunStatus::Queued => "queued",
        RunStatus::Running { .. } => "running",
        RunStatus::Done { .. } => "done",
        RunStatus::Failed { .. } => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

/// Project an `Experiment` into the API's stable typed shape. The
/// flat `state` tag plus optional fields keeps clients simple — they
/// pattern-match on `state` and read the field they care about.
fn run_summary(exp: &lunco_experiments::Experiment, doc_id: Option<u64>) -> ApiValue {
    let mut obj = vec![
        ("experiment_id".into(), ApiValue::Str(exp.id.0.to_string())),
        ("name".into(), ApiValue::Str(exp.name.clone())),
        (
            "state".into(),
            ApiValue::Str(run_state_label(&exp.status).to_string()),
        ),
    ];
    if let Some(d) = doc_id {
        obj.push(("doc_id".into(), api_value!(d)));
    }
    obj.push(("has_result".into(), ApiValue::Bool(exp.result.is_some())));
    let created_ms = exp
        .created_at
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    obj.push(("created_at_ms".into(), api_value!(created_ms)));
    match &exp.status {
        RunStatus::Running { t_current } => {
            obj.push(("t_current".into(), api_value!(*t_current)));
        }
        RunStatus::Done { wall_time_ms } => {
            obj.push(("wall_time_ms".into(), api_value!(*wall_time_ms)));
        }
        RunStatus::Failed { error, partial } => {
            obj.push(("error".into(), ApiValue::Str(error.clone())));
            obj.push(("partial".into(), ApiValue::Bool(*partial)));
        }
        RunStatus::Pending | RunStatus::Queued | RunStatus::Cancelled => {}
    }
    // Self-describing rows: which parameter overrides produced this run,
    // and the bounds it ran under. Lets a sweep's runs be matched back to
    // their inputs (e.g. which Isp → which propUsed) without a side table.
    let mut ovr = Vec::new();
    for (k, v) in &exp.overrides {
        ovr.push((k.0.clone(), param_value_api_value(v)));
    }
    obj.push(("overrides".into(), ApiValue::Map(ovr)));
    obj.push((
        "bounds".into(),
        api_value!({
            "t_start": exp.bounds.t_start,
            "t_end": exp.bounds.t_end,
            "dt": exp.bounds.dt,
            "n_intervals": exp.bounds.n_intervals,
            "tolerance": exp.bounds.tolerance,
            "solver": exp.bounds.solver.as_ref().map(ToString::to_string),
        }),
    ));
    ApiValue::Map(obj)
}

/// Render a `ParamValue` as a typed API value for query rows.
fn param_value_api_value(v: &lunco_experiments::ParamValue) -> ApiValue {
    use lunco_experiments::ParamValue;
    match v {
        ParamValue::Real(x) => api_value!(*x),
        ParamValue::Int(i) => api_value!(*i),
        ParamValue::Bool(b) => api_value!(*b),
        ParamValue::String(s) | ParamValue::Enum(s) => api_value!(s),
        ParamValue::RealArray(a) => api_value!(a.clone()),
    }
}

fn parse_experiment_id(params: &ApiValue, field: &str) -> Option<ExperimentId> {
    params
        .get(field)
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .map(ExperimentId)
}

/// Resolve the most-recently-created experiment for `doc_id` to its id.
/// Mirrors [`latest_run_for_doc`] but returns the id so a caller can
/// look up the full result. `None` when the doc has no runs.
fn latest_experiment_id_for_doc(world: &World, doc_id: DocumentId) -> Option<ExperimentId> {
    let sources = world.get_resource::<ExperimentSources>()?;
    let registry = world.get_resource::<ExperimentRegistry>()?;
    let mut best: Option<&lunco_experiments::Experiment> = None;
    for (id, d) in &sources.0 {
        if *d != doc_id {
            continue;
        }
        if let Some(exp) = registry.get(*id) {
            best = match best {
                Some(prev) if prev.created_at >= exp.created_at => Some(prev),
                _ => Some(exp),
            };
        }
    }
    best.map(|e| e.id)
}

// ─── GetExperimentResult ───────────────────────────────────────────────
//
// Reads completed FastRun trajectory data (`times` + `series`) out of the
// `ExperimentRegistry` so clients can analyse runs without screenshotting
// plot widgets — the programmatic counterpart to the UI's CSV export.
//
// Params:
//   - `experiment_id` (string)  — run to read; OR
//   - `doc_id` (u64)            — read the doc's latest run (convenience).
//   - `variables` (string[])    — optional filter; default = all series.
//   - `max_points` (u64)        — optional cap; strided downsample, last
//                                 sample always kept. Default = uncapped.
struct GetExperimentResultProvider;

impl ApiQueryProvider for GetExperimentResultProvider {
    fn name(&self) -> &'static str {
        "GetExperimentResult"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        // Resolve target run: explicit id wins, else latest for `doc_id`.
        let id = match parse_experiment_id(params, "experiment_id") {
            Some(id) => id,
            None if params.get("experiment_id").is_some() => {
                return query_error(
                    ApiErrorCode::DeserializationError,
                    "GetExperimentResult: `experiment_id` must be a UUID string",
                );
            }
            None => match parse_doc_id(params, "doc_id") {
                Some(doc) => match latest_experiment_id_for_doc(world, doc) {
                    Some(id) => id,
                    None => {
                        return query_error(
                            ApiErrorCode::EntityNotFound,
                            format!("no runs for doc {}", doc.raw()),
                        );
                    }
                },
                None => {
                    return query_error(
                        ApiErrorCode::DeserializationError,
                        "provide `experiment_id` or `doc_id`".to_string(),
                    );
                }
            },
        };

        // Optional variable filter.
        let want: Option<Vec<String>> = match params.get("variables") {
            None => None,
            Some(_) => {
                let Some(values) = api_param_array(params, "variables") else {
                    return query_error(
                        ApiErrorCode::DeserializationError,
                        "GetExperimentResult: `variables` must be an array of strings",
                    );
                };
                let Some(names) = values
                    .iter()
                    .map(ApiValue::as_str)
                    .collect::<Option<Vec<_>>>()
                else {
                    return query_error(
                        ApiErrorCode::DeserializationError,
                        "GetExperimentResult: `variables` must be an array of strings",
                    );
                };
                Some(
                    names
                        .into_iter()
                        .map(str::to_owned)
                        .collect::<Vec<String>>(),
                )
            }
        };
        // Optional downsample cap.
        let max_points = match params.get("max_points") {
            None => None,
            Some(_) => match api_param_u64(params, "max_points") {
                Some(points) => Some(usize::try_from(points).unwrap_or(usize::MAX).max(2)),
                None => {
                    return query_error(
                        ApiErrorCode::DeserializationError,
                        "GetExperimentResult: `max_points` must be an unsigned integer",
                    );
                }
            },
        };

        let Some(registry) = world.get_resource::<ExperimentRegistry>() else {
            return query_error(
                ApiErrorCode::EntityNotFound,
                "experiment registry not installed".to_string(),
            );
        };
        let Some(exp) = registry.get(id) else {
            return query_error(
                ApiErrorCode::EntityNotFound,
                format!("experiment {id:?} not in registry"),
            );
        };
        let Some(result) = &exp.result else {
            // Run dispatched but not done (pending/running/failed-no-partial).
            return query_error(
                ApiErrorCode::DeserializationError,
                format!(
                    "experiment {} has no result (state: {})",
                    exp.id.0,
                    run_state_label(&exp.status)
                ),
            );
        };

        let total = result.times.len();
        // Strided downsample: stride 1 when uncapped or already small.
        let stride = match max_points {
            Some(cap) if total > cap => total.div_ceil(cap),
            _ => 1,
        };
        let sample = |v: &[f64]| -> Vec<f64> {
            if stride <= 1 {
                return v.to_vec();
            }
            let mut out: Vec<f64> = v.iter().step_by(stride).copied().collect();
            // Always keep the final sample so the horizon endpoint shows.
            if let (Some(&last), Some(&got)) = (v.last(), out.last()) {
                if last != got {
                    out.push(last);
                }
            }
            out
        };

        let times = sample(&result.times);
        let mut series = Vec::<(String, ApiValue)>::new();
        let mut missing: Vec<String> = Vec::new();
        match &want {
            Some(names) => {
                for n in names {
                    match result.series.get(n) {
                        Some(v) => {
                            series.push((n.clone(), api_value!(sample(v))));
                        }
                        None => missing.push(n.clone()),
                    }
                }
            }
            None => {
                for (n, v) in &result.series {
                    series.push((n.clone(), api_value!(sample(v))));
                }
            }
        }

        query_ok(api_value!({
            "experiment_id": exp.id.0.to_string(),
            "name": exp.name.clone(),
            "state": run_state_label(&exp.status),
            "total_points": total,
            "returned_points": times.len(),
            "downsampled": stride > 1,
            "variable_count": series.len(),
            "missing_variables": missing,
            "times": times,
            "series": ApiValue::Map(series),
        }))
    }
}

// ─── GetDocumentSource (spec 033 P0, US 1.6) ───────────────────────────

struct GetDocumentSourceProvider;

impl ApiQueryProvider for GetDocumentSourceProvider {
    fn name(&self) -> &'static str {
        "GetDocumentSource"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(doc_id) = parse_doc_id(params, "doc_id") else {
            return err_missing_field("doc_id");
        };

        // Modelica docs are the only kind in the `ModelicaDocuments`
        // today; future kinds (USD, SysML) will need their own registries
        // and a fan-out by `DocumentKindId` here. The cross-domain
        // workspace entry tells us which registry to query, so this
        // dispatch is centralised.
        let ws = world.resource::<WorkspaceResource>();
        let entry = ws.document(doc_id).cloned();
        if entry.is_none() {
            let registry = world.resource::<ModelicaDocuments>();
            let Some(host) = registry.host(doc_id) else {
                return err_doc_not_found(doc_id);
            };
            let document = host.document();
            if !is_generated_document(document) {
                return err_doc_not_found(doc_id);
            }
            return query_ok(api_value!({
                "doc_id": doc_id.raw(),
                "kind": "modelica",
                "source": document.source(),
                "generation": document.generation(),
                "dirty": document.is_dirty(),
                "origin": origin_api_value(document.origin()),
                "title": document.origin().display_name(),
            }));
        }
        let entry = entry.expect("checked above");

        match entry.kind.as_str() {
            "modelica" => {
                let registry = world.resource::<ModelicaDocuments>();
                let Some(host) = registry.host(doc_id) else {
                    return err_doc_not_found(doc_id);
                };
                let document = host.document();
                query_ok(api_value!({
                    "doc_id": doc_id.raw(),
                    "kind": "modelica",
                    "source": document.source(),
                    "generation": document.generation(),
                    "dirty": document.is_dirty(),
                    "origin": origin_api_value(document.origin()),
                    "title": entry.title,
                }))
            }
            other => {
                // Other kinds don't have a content registry hooked up
                // yet — return metadata + a stub so callers can detect
                // the gap programmatically rather than guess.
                query_error(
                    ApiErrorCode::InternalError,
                    format!(
                        "GetDocumentSource not yet implemented for kind `{}` — \
                         only Modelica docs expose source today.",
                        other,
                    ),
                )
            }
        }
    }
}

// ─── GetShareLink ────────────────────────────────────────────────────
//
// `CopyShareLink` is the interactive command: it copies the active model's
// URL to the clipboard. The API's read operation has its own name so the
// command and query namespaces cannot collide; it returns the URL without
// requiring a clipboard. Optional `doc_id` param; defaults to the active
// document. Both paths share the wire format + URL builder
// (`lunco_modelica_core::model_share::share_url`), so they can't drift.

struct GetShareLinkProvider;

impl ApiQueryProvider for GetShareLinkProvider {
    fn name(&self) -> &'static str {
        "GetShareLink"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let doc_id = match params.get("doc_id") {
            Some(_) => match parse_doc_id(params, "doc_id") {
                Some(doc_id) => Some(doc_id),
                None => {
                    return query_error(
                        ApiErrorCode::DeserializationError,
                        "GetShareLink: `doc_id` must be a non-zero unsigned integer",
                    );
                }
            },
            None => world.resource::<WorkspaceResource>().active_document,
        };
        let Some(doc_id) = doc_id else {
            return query_error(
                ApiErrorCode::EntityNotFound,
                "GetShareLink: no `doc_id` given and no active document".to_string(),
            );
        };
        let registry = world.resource::<ModelicaDocuments>();
        let Some(host) = registry.host(doc_id) else {
            return err_doc_not_found(doc_id);
        };
        let url = lunco_modelica_core::model_share::share_url(host.document().source());
        query_ok(api_value!({
            "doc_id": doc_id.raw(),
            "url": url,
        }))
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

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(doc_id) = parse_doc_id(params, "doc_id") else {
            return err_missing_field("doc_id");
        };
        let class_param = match params.get("class") {
            None => None,
            Some(ApiValue::Str(value)) => Some(value.clone()),
            Some(_) => {
                return query_error(
                    ApiErrorCode::DeserializationError,
                    "DescribeModel: `class` must be a string",
                );
            }
        }
        .filter(|value| !value.is_empty());

        // Resolve drilled-in class as the fallback target before we
        // borrow the modelica registry — `DrilledInClassNames` is a
        // separate resource and we need both. Reading them in
        // sequence keeps the borrow checker simple.
        // of going through the `DrilledInClassNames` cache. The
        // helper falls back to first-tab-for-doc when no
        // `TabRenderContext` is in scope (which is the case here —
        // API queries run off-render).
        let drilled_in = lunco_modelica_core::sim_default::drilled_class_for_doc(world, doc_id);

        let registry = world.resource::<ModelicaDocuments>();
        let Some(host) = registry.host(doc_id) else {
            return err_doc_not_found(doc_id);
        };
        let document = host.document();
        let Some(ast) = document.strict_ast() else {
            return query_error(
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
        let target_class_name = class_param.or(drilled_in).or_else(|| {
            // First non-package class via the per-doc Index — same
            // pattern used across the inspector / palette / canvas
            // drill-in code paths (see the Modelica UI panels).
            host.document()
                .index()
                .classes
                .values()
                .find(|c| !matches!(c.kind, lunco_modelica_index::index::ClassKind::Package))
                .map(|c| c.name.clone())
        });
        let Some(target_name) = target_class_name else {
            return query_error(
                ApiErrorCode::EntityNotFound,
                format!("doc {} has no non-package class to describe", doc_id.raw()),
            );
        };
        // The caller may pass `Foo.Bar` — try the short tail first.
        let short = target_name.rsplit('.').next().unwrap_or(&target_name);
        let Some(class) = lunco_modelica_ast::ast_extract::find_class_by_short_name(&ast, short)
        else {
            let candidates =
                lunco_modelica_ast::ast_extract::collect_non_package_classes_qualified(&ast);
            return query_error(
                ApiErrorCode::EntityNotFound,
                format!(
                    "class `{}` not found in doc {}. Candidates: [{}]",
                    target_name,
                    doc_id.raw(),
                    candidates.join(", ")
                ),
            );
        };

        let inputs = lunco_modelica_ast::ast_extract::extract_typed_inputs_for_class(class);
        let parameters = lunco_modelica_ast::ast_extract::extract_typed_parameters_for_class(class);
        let outputs = lunco_modelica_ast::ast_extract::extract_typed_outputs_for_class(class);
        let components = lunco_modelica_ast::ast_extract::extract_components_for_class(class);
        let connections = lunco_modelica_ast::ast_extract::extract_connections_for_class(class);
        let extends = lunco_modelica_ast::ast_extract::extract_extends_for_class(class);

        // Inheritance-merged member list via the long-lived workspace
        // [`ModelicaEngineHandle`]. The engine is kept in sync with
        // the document registry by `drive_engine_sync` so this query
        // sees every open document without a per-call upsert loop.
        let inherited_members = match world
            .get_resource::<lunco_modelica_core::engine_resource::ModelicaEngineHandle>()
            .and_then(lunco_modelica_core::engine_resource::ModelicaEngineHandle::try_lock)
        {
            Some(mut engine) => engine.inherited_members_typed(short),
            None => {
                return query_error(
                    ApiErrorCode::InternalError,
                    "Modelica engine is still indexing; retry DescribeModel",
                );
            }
        };

        query_ok(api_value!({
            "doc_id": doc_id.raw(),
            "class_name": short,
            "class_kind": lunco_modelica_ast::ast_extract::class_kind_label(class),
            "extends": extends,
            "components": components.iter().map(component_info_api_value).collect::<Vec<_>>(),
            "connections": connections
                .iter()
                .map(|(from, to)| api_value!({"from": from, "to": to}))
                .collect::<Vec<_>>(),
            "inputs": inputs.iter().map(typed_api_value).collect::<Vec<_>>(),
            "parameters": parameters.iter().map(typed_api_value).collect::<Vec<_>>(),
            "outputs": outputs.iter().map(typed_api_value).collect::<Vec<_>>(),
            "inherited_members": inherited_members
                .iter()
                .map(|m| api_value!({
                    "name": m.name.clone(),
                    "type_name": m.type_name.clone(),
                    "variability": class_member_variability_str(&m.variability),
                    "causality": class_member_causality_str(&m.causality),
                }))
                .collect::<Vec<_>>(),
        }))
    }
}

fn class_member_variability_str(
    v: &lunco_modelica_core::engine::InheritedVariability,
) -> &'static str {
    use lunco_modelica_core::engine::InheritedVariability;
    match v {
        InheritedVariability::Continuous => "continuous",
        InheritedVariability::Discrete => "discrete",
        InheritedVariability::Parameter => "parameter",
        InheritedVariability::Constant => "constant",
    }
}

fn class_member_causality_str(c: &lunco_modelica_core::engine::InheritedCausality) -> &'static str {
    use lunco_modelica_core::engine::InheritedCausality;
    match c {
        InheritedCausality::Internal => "none",
        InheritedCausality::Input => "input",
        InheritedCausality::Output => "output",
    }
}

fn typed_api_value(c: &lunco_modelica_ast::ast_extract::TypedComponent) -> ApiValue {
    api_value!({
        "name": c.name.clone(),
        "type": c.type_name.clone(),
        "unit": c.unit.clone(),
        "default": c.default,
        "min": c.min,
        "max": c.max,
        "description": if c.description.is_empty() { None } else { Some(c.description.clone()) },
    })
}

fn component_info_api_value(c: &lunco_modelica_ast::ast_extract::ComponentInfo) -> ApiValue {
    let mods: Vec<(String, ApiValue)> = c
        .modifications
        .iter()
        .map(|(k, v)| (k.clone(), ApiValue::Str(v.clone())))
        .collect();
    api_value!({
        "name": c.name.clone(),
        "type": c.type_name.clone(),
        "description": if c.description.is_empty() { None } else { Some(c.description.clone()) },
        "modifications": ApiValue::Map(mods),
    })
}

// ─── SnapshotVariables (spec 033 P1) ───────────────────────────────────

struct SnapshotVariablesProvider;

impl ApiQueryProvider for SnapshotVariablesProvider {
    fn name(&self) -> &'static str {
        "SnapshotVariables"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(doc_id) = parse_doc_id(params, "doc_id") else {
            return err_missing_field("doc_id");
        };
        // Optional `names` filter — when absent, return everything.
        // Accepts either an array of strings or null/missing.
        let name_filter: Option<Vec<String>> = match params.get("names") {
            None | Some(ApiValue::Unit) => None,
            Some(_) => {
                let Some(values) = api_param_array(params, "names") else {
                    return query_error(
                        ApiErrorCode::DeserializationError,
                        "SnapshotVariables: `names` must be an array of strings",
                    );
                };
                let Some(names) = values
                    .iter()
                    .map(ApiValue::as_str)
                    .collect::<Option<Vec<_>>>()
                else {
                    return query_error(
                        ApiErrorCode::DeserializationError,
                        "SnapshotVariables: `names` must be an array of strings",
                    );
                };
                Some(names.into_iter().map(str::to_owned).collect())
            }
        };

        // Doc must exist before we go fishing for the entity. A doc with
        // no linked entity (compile not run yet) is not an error per
        // spec 033 US 4 #3 — return an empty payload with `t: null` so
        // the agent can detect the gap programmatically.
        let registry = world.resource::<ModelicaDocuments>();
        if registry.host(doc_id).is_none() {
            return err_doc_not_found(doc_id);
        }
        let entities = registry.entities_linked_to(doc_id);
        let Some(entity) = entities.first().copied() else {
            return query_ok(api_value!({
                "doc_id": doc_id.raw(),
                "t": ApiValue::Unit,
                "compiled": false,
                "parameters": ApiValue::Map(Vec::new()),
                "inputs": ApiValue::Map(Vec::new()),
                "variables": ApiValue::Map(Vec::new()),
            }));
        };

        let Some(model) = world.get::<ModelicaModel>(entity) else {
            return query_ok(api_value!({
                "doc_id": doc_id.raw(),
                "t": ApiValue::Unit,
                "compiled": false,
                "parameters": ApiValue::Map(Vec::new()),
                "inputs": ApiValue::Map(Vec::new()),
                "variables": ApiValue::Map(Vec::new()),
            }));
        };

        // Project to typed values, optionally honoring the `names` filter.
        // Filter is applied uniformly across parameters/inputs/variables
        // because the agent does not always know which bucket a name
        // lives in (e.g. `valve` is an input on this model but might be
        // a parameter on the next one).
        let in_filter = |name: &str| -> bool {
            name_filter
                .as_ref()
                .is_none_or(|f| f.iter().any(|n| n == name))
        };
        let project = |map: &std::collections::HashMap<String, f64>| -> ApiValue {
            let inner: Vec<(String, ApiValue)> = map
                .iter()
                .filter(|(k, _)| in_filter(k))
                .map(|(k, v)| (k.clone(), api_value!(*v)))
                .collect();
            ApiValue::Map(inner)
        };

        query_ok(api_value!({
            "doc_id": doc_id.raw(),
            "t": model.current_time,
            "compiled": true,
            "model_name": model.model_name.clone(),
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
// Twin's documents, the source-library index, and currently-open documents,
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

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(query) = params.get("query").and_then(ApiValue::as_str) else {
            return query_error(
                ApiErrorCode::DeserializationError,
                "FindModel requires a non-empty `query` string",
            );
        };
        let query = query.trim().to_owned();
        if query.is_empty() {
            return query_error(
                ApiErrorCode::DeserializationError,
                "find_model requires a non-empty `query` string",
            );
        }
        let limit = match params.get("limit") {
            None => 20,
            Some(_) => match api_param_u64(params, "limit") {
                Some(limit) => limit.clamp(1, 200) as usize,
                None => {
                    return query_error(
                        ApiErrorCode::DeserializationError,
                        "FindModel: `limit` must be an unsigned integer",
                    );
                }
            },
        };
        let q = query.to_ascii_lowercase();
        let mut hits: Vec<FindHit> = Vec::new();

        // ── External example assets ──────────────────────────────
        let models = match bundled_models() {
            Ok(models) => models,
            Err(error) => return query_error(ApiErrorCode::InternalError, error),
        };
        for m in models {
            let label = m.filename.trim_end_matches(".mo").to_string();
            if let Some(score) = score(&q, &label, &m.tagline) {
                hits.push(FindHit {
                    uri: format!("bundled://{}", m.filename),
                    label,
                    source: "bundled",
                    description: m.tagline,
                    score,
                });
            }
        }

        // ── Active Twin folder ───────────────────────────────────
        let twin_files: Vec<(String, String)> = {
            world
                .get_resource::<WorkspaceResource>()
                .and_then(|ws| {
                    let twin = ws.active_twin.and_then(|id| ws.twin(id));
                    let root =
                        twin.and_then(|t| t.root_handle().as_file_path().map(|p| p.to_path_buf()));
                    twin.map(|t| {
                        t.files()
                            .iter()
                            .map(|f| {
                                let abs = root
                                    .as_ref()
                                    .map(|r| {
                                        r.join(&f.relative_path).to_string_lossy().into_owned()
                                    })
                                    .unwrap_or_else(|| {
                                        f.relative_path.to_string_lossy().into_owned()
                                    });
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

        // ── source-library index ────────────────────────────────────────────
        // Scan the cached library if it's been initialized; force
        // initialization here would block on the JSON parse, which
        // is acceptable since the result is cached after the first
        // call. Subsequent finds hit the warm cache.
        for c in library_class_library() {
            if let Some(score) = score(&q, c.short_name(), &c.name) {
                let label = c.short_name().to_string();
                hits.push(FindHit {
                    uri: c.name.clone(),
                    label,
                    source: "library",
                    description: if c.description.is_empty() {
                        c.name.clone()
                    } else {
                        c.description.clone()
                    },
                    score,
                });
            }
        }

        // ── Currently-open documents ─────────────────────────────
        let open_docs: Vec<(u64, String, String)> = {
            world
                .get_resource::<WorkspaceResource>()
                .map(|ws| {
                    ws.documents()
                        .iter()
                        .map(|e| {
                            let uri = match &e.origin {
                                DocumentOrigin::File { path, .. } => {
                                    path.to_string_lossy().into_owned()
                                }
                                DocumentOrigin::Bundled { filename } => {
                                    format!("bundled://{filename}")
                                }
                                DocumentOrigin::Untitled { name } => format!("mem://{name}"),
                            };
                            (e.id.raw(), e.title.clone(), uri)
                        })
                        .collect()
                })
                .unwrap_or_default()
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
        let items: Vec<ApiValue> = hits
            .into_iter()
            .map(|h| {
                api_value!({
                    "uri": h.uri,
                    "label": h.label,
                    "source": h.source,
                    "description": h.description,
                    "score": h.score,
                })
            })
            .collect();
        query_ok(api_value!({
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

// ─── Provider helpers ──────────────────────────────────────────────────

fn parse_doc_id(params: &ApiValue, field: &str) -> Option<DocumentId> {
    params
        .get(field)
        .and_then(|value| match value {
            ApiValue::Int(value) => u64::try_from(*value).ok(),
            _ => None,
        })
        .filter(|id| *id != 0)
        .map(DocumentId::new)
}

fn err_missing_field(field: &str) -> ApiQueryResult {
    query_error(
        ApiErrorCode::DeserializationError,
        format!("missing or invalid `{field}` field (must be a non-zero u64 doc_id)"),
    )
}

fn err_doc_not_found(doc_id: DocumentId) -> ApiQueryResult {
    query_error(
        ApiErrorCode::EntityNotFound,
        format!("doc_id {} not in registry", doc_id.raw()),
    )
}

/// Project a [`lunco_doc::DocumentOrigin`] onto a typed map. Untitled docs carry
/// only a name; File docs carry an absolute path + writability flag —
/// matches the discriminator the Twin Browser already shows in the UI.
fn origin_api_value(origin: &DocumentOrigin) -> ApiValue {
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
