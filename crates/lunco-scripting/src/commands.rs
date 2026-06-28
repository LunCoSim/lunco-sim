//! One-shot script-execution commands.
//!
//! `RunRhai` / `RunPython` are typed `#[Command]`s — discoverable on every
//! transport (HTTP API, MCP, scripts) like any other command. `RunRhai` is
//! always present (pure-Rust, wasm-clean). `RunPython` is `#[cfg]`-gated on the
//! `python` feature, so it only appears in the API schema when the runtime is
//! actually compiled in. This is the fix for the original gap: the old
//! `ExecuteScript` was always advertised but silently no-op'd when no scripting
//! plugin handled it.
//!
//! The handler returns `Result<Ack, String>`; the `#[on_command]` macro records
//! the outcome under the request id, so callers poll `QueryCommandResult` for
//! the script's stdout (in `Ack.assigned.stdout`) or its error message.
//!
//! Adding another language later = a new `#[cfg(feature = "…")]` command here +
//! a backend in `backend.rs` + one line in the registration list.

#[cfg(feature = "python")]
use crate::{backend::ScriptBackends, doc::ScriptLanguage};
#[cfg(any(feature = "rhai", feature = "python"))]
use bevy::prelude::*;
use lunco_core::register_commands;
#[cfg(any(feature = "rhai", feature = "python"))]
use lunco_core::{on_command, Ack, Command, OpId};
#[cfg(feature = "rhai")]
use lunco_core::ActiveCommandId;
#[cfg(feature = "rhai")]
use crate::world_bridge::PendingWorldScripts;
#[cfg(feature = "rhai")]
use crate::{
    doc::{ScriptDocument, ScriptLanguage, ScriptedModel},
    ScriptRegistry,
};
#[cfg(feature = "rhai")]
use lunco_doc::{DocumentHost, DocumentId};

/// Mints unique `ScriptDocument` ids for scenarios attached via [`RunScenario`].
/// Based high (1<<40) so it never collides with hand-authored document ids
/// (tests, fixtures) in the same `ScriptRegistry`.
#[cfg(feature = "rhai")]
#[derive(Resource)]
pub struct ScenarioDocAllocator(u64);

#[cfg(feature = "rhai")]
impl Default for ScenarioDocAllocator {
    fn default() -> Self {
        Self(1 << 40)
    }
}

#[cfg(feature = "rhai")]
impl ScenarioDocAllocator {
    fn next(&mut self) -> u64 {
        let id = self.0;
        self.0 += 1;
        id
    }
}

#[cfg(feature = "rhai")]
#[Command(default)]
pub struct RunRhai {
    pub code: String,
}

// rhai runs with full World access (`cmd`/`world_pos`/`get`/...), which an
// observer can't hold. So the handler ENQUEUES the snippet under the active
// request id; the exclusive `drain_world_scripts` system runs it next
// FixedUpdate and overwrites this provisional outcome with the real stdout.
#[cfg(feature = "rhai")]
#[on_command(RunRhai)]
fn on_run_rhai(
    _t: On<RunRhai>,
    active: Res<ActiveCommandId>,
    mut pending: ResMut<PendingWorldScripts>,
) -> Result<Ack, String> {
    let id = active.get().unwrap_or(0);
    pending.queue.push((id, cmd.code.clone()));
    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({ "status": "queued" });
    Ok(ack)
}

/// Attach a persistent rhai scenario to an entity — the scenario-loading entry
/// point for the API / MCP / UI / ROS2. Registers the source as a
/// `ScriptDocument` and attaches a `ScriptedModel { Rhai }` to `target`, so the
/// per-entity runtime starts calling its `on_start`/`on_tick`/`on_event` hooks.
///
/// Idempotent + HOT-RELOAD: re-running on an entity that already has a scenario
/// reuses its document id and bumps the generation, so `tick_rhai_models`
/// recompiles in place (state reset) instead of leaking documents.
#[cfg(feature = "rhai")]
#[Command]
pub struct RunScenario {
    #[authz_target]
    pub target: Entity,
    pub source: String,
}

#[cfg(feature = "rhai")]
#[on_command(RunScenario)]
fn on_run_scenario(
    _t: On<RunScenario>,
    mut registry: ResMut<ScriptRegistry>,
    mut alloc: ResMut<ScenarioDocAllocator>,
    q_existing: Query<&ScriptedModel>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let target = cmd.target;

    // Reuse the doc id if a scenario is already attached (hot-reload), else mint.
    let existing = q_existing.get(target).ok().and_then(|m| m.document_id);
    let (doc_id_raw, generation) = match existing {
        Some(id) => {
            let next_gen = registry
                .documents
                .get(&DocumentId::new(id))
                .map(|h| h.document().generation + 1)
                .unwrap_or(0);
            (id, next_gen)
        }
        None => (alloc.next(), 0),
    };

    let doc = ScriptDocument {
        id: doc_id_raw,
        generation,
        language: ScriptLanguage::Rhai,
        source: cmd.source.clone(),
        inputs: vec![],
        outputs: vec![],
    };
    registry
        .documents
        .insert(DocumentId::new(doc_id_raw), DocumentHost::new(doc));

    commands.entity(target).insert(ScriptedModel {
        document_id: Some(doc_id_raw),
        language: Some(ScriptLanguage::Rhai),
        ..default()
    });

    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({ "document_id": doc_id_raw, "generation": generation });
    Ok(ack)
}

#[cfg(feature = "python")]
#[Command(default)]
pub struct RunPython {
    pub code: String,
}

#[cfg(feature = "python")]
#[on_command(RunPython)]
fn on_run_python(_t: On<RunPython>, backends: Res<ScriptBackends>) -> Result<Ack, String> {
    let backend = backends
        .get(ScriptLanguage::Python)
        .ok_or_else(|| "python backend not registered".to_string())?;
    let stdout = backend.eval(&cmd.code)?;
    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({ "stdout": stdout });
    Ok(ack)
}

// Generates `register_all_commands` for the compiled-in script commands. One
// cfg-exclusive invocation per feature combo so exactly one
// `register_all_commands` is emitted (covers the script-free build too).
#[cfg(all(feature = "rhai", feature = "python"))]
register_commands!(on_run_rhai, on_run_scenario, on_run_python);
#[cfg(all(feature = "rhai", not(feature = "python")))]
register_commands!(on_run_rhai, on_run_scenario);
#[cfg(all(not(feature = "rhai"), feature = "python"))]
register_commands!(on_run_python,);
#[cfg(all(not(feature = "rhai"), not(feature = "python")))]
register_commands!();
