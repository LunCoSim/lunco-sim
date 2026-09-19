//! Scripting's adapter onto the unified diagnostics substrate.
//!
//! The store + diagnostic type + typed status projection are shared
//! ([`lunco_doc_bevy::DocumentDiagnostics`]); the only scripting-specific part
//! is resolving an *entity* to its scenario document and source. This provider
//! is the rhai analogue of Modelica's `CompileStatus` query — same API shape,
//! so any caller (HTTP API, MCP, UI) polls scenario health the same way.

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::{ApiQueryError, ApiQueryResult, api_param_u64, api_param_u64_or_string};
use lunco_api_core::{ApiErrorCode, ApiValue, api_value};
use lunco_core::GlobalEntityId;
use lunco_doc::{DocDiagnostics, Document, DocumentId, document_status};
use lunco_doc_bevy::DocumentDiagnostics;

use lunco_scripting::doc::{ScriptLanguage, ScriptedModel};
use lunco_scripting::scenario::ScenarioDriver;
use lunco_scripting_bridge_core::ApiValueBuilder;
use lunco_scripting_rhai_world::world_bridge::RhaiScenarioRuntime;

/// `ScriptStatus { target }` → `{ state, ok, diagnostics: [{severity,message,line,col}] }`
/// for the scenario attached to entity `target` (a `GlobalEntityId`). Returns an
/// idle status if the entity has no scenario; the same shape Modelica's
/// `CompileStatus` returns, so authors poll scenario compile/runtime health
/// uniformly instead of grepping logs.
struct ScriptStatusProvider;

impl ApiQueryProvider for ScriptStatusProvider {
    fn name(&self) -> &'static str {
        "ScriptStatus"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(gid) = api_param_u64(params, "target") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "ScriptStatus: `target` (entity id) required",
            ));
        };

        let Some(entity) = world
            .get_resource::<ApiEntityRegistry>()
            .and_then(|r| r.resolve(&GlobalEntityId::from_raw(gid)))
        else {
            return Err(ApiQueryError::new(
                ApiErrorCode::EntityNotFound,
                format!("ScriptStatus: no entity with id {gid}"),
            ));
        };

        // No scenario attached → idle (not an error — the entity simply isn't scripted).
        let Some(doc_raw) = world
            .get::<ScriptedModel>(entity)
            .and_then(|m| m.document_id)
        else {
            return Ok(Some(document_status_api_value(None)));
        };
        let doc = DocumentId::new(doc_raw);
        let entry = world
            .get_resource::<DocumentDiagnostics>()
            .and_then(|s| s.get(doc));
        Ok(Some(document_status_api_value(entry)))
    }
}

/// `InspectScriptDocument { doc_id }` → source identity, generation, origin
/// and the shared compile/diagnostic snapshot for one explicit script file.
struct InspectScriptDocumentProvider;

impl ApiQueryProvider for InspectScriptDocumentProvider {
    fn name(&self) -> &'static str {
        "InspectScriptDocument"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(raw) = api_param_u64_or_string(params, "doc_id") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "InspectScriptDocument requires an explicit numeric `doc_id`",
            ));
        };
        let doc_id = DocumentId::new(raw);
        if doc_id.is_unassigned() {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "InspectScriptDocument requires an assigned `doc_id`",
            ));
        }
        let Some(host) = world
            .get_resource::<lunco_scripting::ScriptRegistry>()
            .and_then(|registry| registry.documents.get(&doc_id))
        else {
            return Err(ApiQueryError::new(
                ApiErrorCode::EntityNotFound,
                format!("script document {doc_id} is not open"),
            ));
        };
        let document = host.document();
        let origin = document.origin();
        let status = world
            .get_resource::<DocumentDiagnostics>()
            .and_then(|diagnostics| diagnostics.get(doc_id));
        let kind = match document.language {
            ScriptLanguage::Rhai => "rhai",
            ScriptLanguage::Python => "python",
        };
        let status = document_status_api_value(status);
        Ok(Some(api_value!({
            "doc_id": raw,
            "kind": kind,
            "language": format!("{:?}", document.language),
            "source": document.source.clone(),
            "generation": document.generation(),
            "dirty": document.is_dirty(),
            "read_only": origin.is_read_only(),
            "origin": {
                "uri": origin.session_uri(),
                "title": origin.display_name(),
                "writable": origin.is_writable(),
            },
            "asset_id": document.asset_id.clone(),
            "inputs": document.inputs.clone(),
            "outputs": document.outputs.clone(),
            "status": status,
        })))
    }
}

/// `ScriptInspect { target }` → live introspection of the scenario running on
/// entity `target`. Where `ScriptStatus` answers "is it healthy?", this answers
/// "what is it *doing*?" — the running per-entity state object, which lifecycle
/// hooks it defines, the compiled generation, started/paused flags, plus the
/// same `status` block so one call gives the full runtime picture (no log
/// grepping). `{ "scripted": false }` if the entity has no scenario.
struct ScriptInspectProvider;

impl ApiQueryProvider for ScriptInspectProvider {
    fn name(&self) -> &'static str {
        "ScriptInspect"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(gid) = api_param_u64(params, "target") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "ScriptInspect: `target` (entity id) required",
            ));
        };

        let Some(entity) = world
            .get_resource::<ApiEntityRegistry>()
            .and_then(|r| r.resolve(&GlobalEntityId::from_raw(gid)))
        else {
            return Err(ApiQueryError::new(
                ApiErrorCode::EntityNotFound,
                format!("ScriptInspect: no entity with id {gid}"),
            ));
        };

        // Not scripted → say so plainly (not an error: a bare entity is valid).
        let Some(model) = world.get::<ScriptedModel>(entity) else {
            return Ok(Some(api_value!({ "scripted": false })));
        };
        let paused = model.paused;
        let language = model.language;
        let doc_raw = model.document_id;

        // Live FSM + per-entity state from the Rhai scenario driver. The driver
        // builds the typed API value; JSON is introduced only below when this
        // provider constructs its external API response.
        let intro = world
            .get_resource::<ScenarioDriver<RhaiScenarioRuntime>>()
            .and_then(|d| d.introspect(entity, &ApiValueBuilder));

        // Compile/runtime health, the SAME block ScriptStatus returns.
        let status = match doc_raw {
            Some(raw) => {
                let entry = world
                    .get_resource::<DocumentDiagnostics>()
                    .and_then(|s| s.get(DocumentId::new(raw)));
                document_status_api_value(entry)
            }
            None => document_status_api_value(None),
        };

        match intro {
            Some(i) => Ok(Some(api_value!({
                "scripted": true,
                "language": language.map(|language| format!("{language:?}")),
                "paused": paused,
                "status": status,
                "running": i.compiled && i.started && !paused,
                "compiled": i.compiled,
                "started": i.started,
                "generation": i.generation,
                "hooks": i.hooks,
                "state": i.state,
            }))),
            // Tracked-but-not-yet-driven (or a non-rhai backend): attached but the
            // driver hasn't compiled/started it this run.
            None => Ok(Some(api_value!({
                "scripted": true,
                "language": language.map(|language| format!("{language:?}")),
                "paused": paused,
                "status": status,
                "running": false,
            }))),
        }
    }
}

fn document_status_api_value(entry: Option<&DocDiagnostics>) -> ApiValue {
    let status = document_status(entry);
    let diagnostics = status
        .diagnostics
        .into_iter()
        .map(|diagnostic| {
            api_value!({
                "severity": diagnostic.severity,
                "message": diagnostic.message,
                "line": diagnostic.line,
                "col": diagnostic.col,
            })
        })
        .collect::<Vec<_>>();
    api_value!({
        "state": status.state,
        "ok": status.ok,
        "diagnostics": diagnostics,
    })
}

/// Register the scripting diagnostics + introspection query providers.
pub fn register_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let mut reg = app.world_mut().resource_mut::<ApiQueryRegistry>();
    reg.register(ScriptStatusProvider);
    reg.register(ScriptInspectProvider);
    reg.register(InspectScriptDocumentProvider);
}
