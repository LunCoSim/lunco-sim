//! Scripting's adapter onto the unified diagnostics substrate.
//!
//! The store + diagnostic type + status JSON are shared
//! ([`lunco_doc_bevy::DocumentDiagnostics`]); the only scripting-specific part
//! is resolving an *entity* to its scenario document and source. This provider
//! is the rhai analogue of Modelica's `CompileStatus` query — same JSON shape,
//! so any caller (HTTP API, MCP, UI) polls scenario health the same way.

#![cfg(feature = "rhai")]

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_core::GlobalEntityId;
use lunco_doc::{status_json, DocumentId};
use lunco_doc_bevy::DocumentDiagnostics;

use crate::bridge_core::JsonBuilder;
use crate::doc::ScriptedModel;
use crate::scenario::ScenarioDriver;
use crate::world_bridge::RhaiScenarioRuntime;

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

    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let Some(gid) = params.get("target").and_then(serde_json::Value::as_u64) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "ScriptStatus: `target` (entity id) required".to_string(),
            );
        };

        let Some(entity) = world
            .get_resource::<ApiEntityRegistry>()
            .and_then(|r| r.resolve(&GlobalEntityId::from_raw(gid)))
        else {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("ScriptStatus: no entity with id {gid}"),
            );
        };

        // No scenario attached → idle (not an error — the entity simply isn't scripted).
        let Some(doc_raw) = world.get::<ScriptedModel>(entity).and_then(|m| m.document_id) else {
            return ApiResponse::ok(status_json(None));
        };
        let doc = DocumentId::new(doc_raw);
        let entry = world
            .get_resource::<DocumentDiagnostics>()
            .and_then(|s| s.get(doc));
        ApiResponse::ok(status_json(entry))
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

    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let Some(gid) = params.get("target").and_then(serde_json::Value::as_u64) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "ScriptInspect: `target` (entity id) required".to_string(),
            );
        };

        let Some(entity) = world
            .get_resource::<ApiEntityRegistry>()
            .and_then(|r| r.resolve(&GlobalEntityId::from_raw(gid)))
        else {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("ScriptInspect: no entity with id {gid}"),
            );
        };

        // Not scripted → say so plainly (not an error: a bare entity is valid).
        let Some(model) = world.get::<ScriptedModel>(entity) else {
            return ApiResponse::ok(serde_json::json!({ "scripted": false }));
        };
        let paused = model.paused;
        let language = model.language;
        let doc_raw = model.document_id;

        // Live FSM + per-entity state from the rhai scenario driver. The
        // JsonBuilder is the serialization seam — the driver/backend build state
        // natively and JSON appears only here, at the API boundary. (Other
        // backends would each contribute their own driver; rhai is the only one
        // with a lifecycle today — see scenario.rs Python TODO.)
        let intro = world
            .get_resource::<ScenarioDriver<RhaiScenarioRuntime>>()
            .and_then(|d| d.introspect(entity, &JsonBuilder));

        // Compile/runtime health, the SAME block ScriptStatus returns.
        let status = match doc_raw {
            Some(raw) => {
                let entry = world
                    .get_resource::<DocumentDiagnostics>()
                    .and_then(|s| s.get(DocumentId::new(raw)));
                status_json(entry)
            }
            None => status_json(None),
        };

        let mut out = serde_json::json!({
            "scripted": true,
            "language": language.map(|l| format!("{l:?}")),
            "paused": paused,
            "status": status,
        });
        match intro {
            Some(i) => {
                out["running"] = serde_json::json!(i.compiled && i.started && !paused);
                out["compiled"] = serde_json::json!(i.compiled);
                out["started"] = serde_json::json!(i.started);
                out["generation"] = serde_json::json!(i.generation);
                out["hooks"] = serde_json::json!(i.hooks);
                out["state"] = i.state;
            }
            // Tracked-but-not-yet-driven (or a non-rhai backend): attached but the
            // driver hasn't compiled/started it this run.
            None => {
                out["running"] = serde_json::json!(false);
            }
        }
        ApiResponse::ok(out)
    }
}

/// Register the scripting diagnostics + introspection query providers.
pub fn register_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let mut reg = app.world_mut().resource_mut::<ApiQueryRegistry>();
    reg.register(ScriptStatusProvider);
    reg.register(ScriptInspectProvider);
}
