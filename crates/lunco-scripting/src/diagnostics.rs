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
use lunco_doc::DocumentId;
use lunco_doc_bevy::{status_json, DocumentDiagnostics};

use crate::doc::ScriptedModel;
use crate::ScriptRegistry;

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
            return ApiResponse::ok(status_json(None, None));
        };
        let doc = DocumentId::new(doc_raw);
        let source = world
            .get_resource::<ScriptRegistry>()
            .and_then(|r| r.documents.get(&doc))
            .map(|h| h.document().source.clone());
        let entry = world
            .get_resource::<DocumentDiagnostics>()
            .and_then(|s| s.get(doc));
        ApiResponse::ok(status_json(entry, source.as_deref()))
    }
}

/// Register the scripting diagnostics query provider.
pub fn register_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let mut reg = app.world_mut().resource_mut::<ApiQueryRegistry>();
    reg.register(ScriptStatusProvider);
}
