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
use crate::backend::ScriptBackends;
// Used by both backends (rhai scenario docs + the RunPython handler), so import
// it once for either feature — importing under each cfg collides when both are on.
#[cfg(any(feature = "rhai", feature = "python"))]
use crate::doc::ScriptLanguage;
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
    doc::{ScriptDocument, ScriptedModel},
    ScriptRegistry,
};
#[cfg(feature = "rhai")]
use lunco_doc::DocumentId;
// Pause/stop scenario commands are language-agnostic (`any(rhai, python)`) and
// touch `ScriptedModel`; rhai already imports it above, so a python-only build
// needs its own import.
#[cfg(all(feature = "python", not(feature = "rhai")))]
use crate::doc::ScriptedModel;

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
    guard: Option<Res<lunco_core::session::SyncApplyGuard>>,
) -> Result<Ack, String> {
    let id = active.get().unwrap_or(0);
    // §3.4: gate the snippet's cmd()s against the submitting session. `Some`
    // only when this RunRhai arrived from the wire (a remote peer); `None` for a
    // local / host-issued snippet → host-trusted (ungated).
    let authority = guard.and_then(|g| g.0);
    pending.queue.push((id, cmd.code.clone(), authority));
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
// `reflect_default` registers `ReflectDefault` (+ the manual `Default` below) so
// the reflect deserializer fills a MISSING `params` from the default — existing
// `{target,source}` callers keep working when they omit the new field. (Can't use
// `#[Command(default)]`: it *derives* Default, which `Entity` doesn't implement.)
#[Command(reflect_default)]
pub struct RunScenario {
    #[authz_target]
    pub target: Entity,
    pub source: String,
    /// Optional scenario parameters as a JSON object string (e.g.
    /// `{"speed":1.5,"target":"rover_b"}`), readable in the script as the
    /// `params` constant. Omitted → none.
    pub params: String,
}

#[cfg(feature = "rhai")]
impl Default for RunScenario {
    fn default() -> Self {
        Self {
            target: Entity::PLACEHOLDER,
            source: String::new(),
            params: String::new(),
        }
    }
}

#[cfg(feature = "rhai")]
#[on_command(RunScenario)]
fn on_run_scenario(
    _t: On<RunScenario>,
    mut registry: ResMut<ScriptRegistry>,
    mut alloc: ResMut<ScenarioDocAllocator>,
    q_existing: Query<&ScriptedModel>,
    guard: Option<Res<lunco_core::session::SyncApplyGuard>>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let (doc_id_raw, generation) = attach_rhai_scenario(
        cmd.target,
        cmd.source.clone(),
        cmd.params.clone(),
        guard.and_then(|g| g.0),
        &mut registry,
        &mut alloc,
        &q_existing,
        &mut commands,
    );
    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({ "document_id": doc_id_raw, "generation": generation });
    Ok(ack)
}

/// Register a rhai source as a `ScriptDocument` and attach a `ScriptedModel` to
/// `target`, reusing the doc id (hot-reload, generation bump) if one already
/// exists. Shared by `RunScenario` and `RunTimeline`. Returns `(doc_id, generation)`.
#[cfg(feature = "rhai")]
pub(crate) fn attach_rhai_scenario(
    target: Entity,
    source: String,
    params: String,
    authority: Option<lunco_core::SessionId>,
    registry: &mut ScriptRegistry,
    alloc: &mut ScenarioDocAllocator,
    q_existing: &Query<&ScriptedModel>,
    commands: &mut Commands,
) -> (u64, u64) {
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

    // Execution scope from a `// @scope client|both` directive in the source, so
    // it works identically for API-attached (`RunScenario`) and USD-embedded
    // scenarios (both funnel through here) with no wire/schema change.
    let scope = crate::scenario::ScriptScope::from_source(&source);
    let mut doc = ScriptDocument::new(doc_id_raw, ScriptLanguage::Rhai, source);
    // Hot-reload reuses the doc id and bumps generation; `new` resets it to 0,
    // so carry the computed generation through.
    doc.generation = generation;
    // Scenario parameters (JSON object string) — the runtime exposes them to the
    // script as a `params` constant, so the same source serves many entities.
    doc.params = params;
    // USD-embedded persistence: the LOAD half is done — a prim's `lunco:script`
    // is read by lunco-usd-bevy into `EmbeddedScenarioSource` and attached by
    // `attach_embedded_scenarios` below, so scene-authored scenarios run on
    // spawn.
    //
    // TODO(save) — BLOCKED on a USD bridge, not a scripting task: writing a
    // live-edited scenario back onto its prim means `ApplyUsdOp` /
    // `SetAttribute(path, "lunco:script", "string", <usd-escaped src>)`, which
    // edits an EDITABLE document in `UsdDocumentRegistry`. But a runtime entity's
    // `UsdPrimPath` references a read-only `Handle<UsdStageAsset>` (a flattened,
    // composed `Arc<TextReader>`) — there is no mapping from that stage asset to
    // a savable source document/layer. That asset↔document bridge must be built
    // in the USD subsystem first. Until then, durable scenarios live as Twin
    // files (see `crate::timelines` for the same pattern applied to timelines) or
    // are authored directly in the `.usda` source. Ref: project_tools_architecture
    // Phase 2 incr #3.
    // The one insert funnel — attaches a journal recorder when the Twin journal
    // is wired, so this live edit (and every hot-reload SetSource) auto-records.
    registry.insert_document(DocumentId::new(doc_id_raw), doc);

    commands.entity(target).insert((
        ScriptedModel {
            document_id: Some(doc_id_raw),
            language: Some(ScriptLanguage::Rhai),
            ..default()
        },
        // §3.4: the session this scenario's cmd()s are gated against. Always
        // (re)inserted so a hot-reload relaunch refreshes it; `None` = ungated.
        crate::scenario::ScriptAuthority(authority),
        // Where this scenario ticks (host / client / both) — the scenario driver
        // gates each entity on it per peer.
        scope,
    ));

    (doc_id_raw, generation)
}

/// LOAD half of USD-embedded scenario persistence: drain entities the USD loader
/// stamped with [`lunco_core::EmbeddedScenarioSource`] (a `lunco:script`
/// attribute on their prim), attaching each as a running rhai scenario and
/// removing the marker. Attaches by `Entity` directly — no gid round-trip — so it
/// works the instant the prim spawns. The loader (`lunco-usd-bevy`) and this
/// runtime stay decoupled via the lunco-core marker.
#[cfg(feature = "rhai")]
pub fn attach_embedded_scenarios(
    q: Query<(Entity, &lunco_core::EmbeddedScenarioSource), Without<ScriptedModel>>,
    mut registry: ResMut<ScriptRegistry>,
    mut alloc: ResMut<ScenarioDocAllocator>,
    q_existing: Query<&ScriptedModel>,
    mut commands: Commands,
) {
    for (entity, embedded) in q.iter() {
        attach_rhai_scenario(
            entity,
            embedded.0.clone(),
            String::new(),
            // Scene-authored (loaded by the host from USD) → host-trusted, ungated.
            None,
            &mut registry,
            &mut alloc,
            &q_existing,
            &mut commands,
        );
        commands
            .entity(entity)
            .remove::<lunco_core::EmbeddedScenarioSource>();
    }
}

/// LOAD half for FILE-backed scenarios: entities the USD loader stamped with
/// [`lunco_core::EmbeddedScenarioPath`] (a `lunco:scriptPath` attribute). Loads
/// the `.rhai` asset through the `AssetServer` (wasm-safe — no `std::fs`) and,
/// once ready, swaps the path marker for an [`lunco_core::EmbeddedScenarioSource`]
/// so [`attach_embedded_scenarios`] runs the normal attach path next. Keeps the
/// USD loader and scripting runtime decoupled via the lunco-core markers (same
/// pattern as the inline path). The `Local` map holds a strong handle per entity
/// so the asset isn't dropped mid-load; it's cleared on swap/failure.
#[cfg(feature = "rhai")]
pub fn resolve_embedded_scenario_paths(
    q: Query<
        (Entity, &lunco_core::EmbeddedScenarioPath),
        Without<lunco_core::EmbeddedScenarioSource>,
    >,
    sources: Res<Assets<crate::source_asset::RhaiSource>>,
    asset_server: Res<AssetServer>,
    mut pending: Local<std::collections::HashMap<Entity, Handle<crate::source_asset::RhaiSource>>>,
    mut commands: Commands,
) {
    for (entity, path) in q.iter() {
        let handle = pending.entry(entity).or_insert_with(|| {
            // USD authors may write an `assets/` prefix; the AssetServer root is
            // already `assets/`, so strip it (mirrors lunco-usd-sim).
            let rel = path.0.strip_prefix("assets/").unwrap_or(&path.0).to_string();
            // TODO(scenario-resolve): a `.rhai` fetched into a peer's scenario cache
            // (`scenario://<id>/…`) is NOT found here — this loads against the DEFAULT
            // asset source, not the loaded scene's source. So a twin/imported policy
            // or scenario script syncs (whole-twin content plane) but fails to load on
            // the peer. Fix (pick one): (1) author the ref as a USD `asset` attribute
            // (`@…rhai@`) read through the resolver's `canonicalize`, which anchors it
            // to the scene's `scenario://<id>/` source (like `lunco:resolvedAsset`); or
            // (2) prefix `rel` with the active `scenario://<id>/` when a
            // `RemoteScenarioManifest` is loaded (needs that id threaded to scripting —
            // a networking→scripting coupling to avoid, so (1) is preferred). Inline
            // `lunco:script` / `LuncoPolicy` sources are unaffected (they ride the doc).
            asset_server.load(rel)
        });
        if asset_server.load_state(&*handle).is_failed() {
            warn!("[scripting] failed to load scenario `{}` via AssetServer", path.0);
            commands.entity(entity).remove::<lunco_core::EmbeddedScenarioPath>();
            pending.remove(&entity);
            continue;
        }
        if let Some(src) = sources.get(&*handle) {
            commands
                .entity(entity)
                .insert(lunco_core::EmbeddedScenarioSource(src.text.clone()))
                .remove::<lunco_core::EmbeddedScenarioPath>();
            pending.remove(&entity);
        }
    }
}

/// Register (or hot-replace) a named rhai **tool library** — a reusable bundle
/// of selection / behaviour policy callable from any scenario as
/// `name::fn(...)` (see [`crate::tool_libs`]). The scenario-authoring counterpart
/// to RunScenario: RunScenario attaches a program to ONE entity; this publishes
/// shared library code every scenario can call, with no Rust rebuild. Idempotent
/// + hot-reload — re-registering a name replaces it and the runtime picks it up
/// on the next tick.
#[cfg(feature = "rhai")]
#[Command(default)]
pub struct RegisterToolLibrary {
    pub name: String,
    pub source: String,
}

#[cfg(feature = "rhai")]
#[on_command(RegisterToolLibrary)]
#[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
fn on_register_tool_library(
    _t: On<RegisterToolLibrary>,
    // Optional: present only when the workspace plugin is installed. Used to
    // persist the library to the active Twin's `tools/` dir. `None` (headless /
    // no-twin) just keeps the in-memory registration.
    ws: Option<Res<lunco_workspace::WorkspaceResource>>,
    // Journal handle (present once wired). Records the registration as a
    // `DomainKind::ToolLibrary` op so it syncs to peers + persists cross-platform.
    // The command isn't on the command bus, so this only fires for LOCAL
    // registrations; remote peers' registrations arrive via the replay leg
    // (which calls `register_tool_library` directly, not this command).
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) -> Result<Ack, String> {
    if cmd.name.is_empty() {
        return Err("RegisterToolLibrary: `name` must not be empty".to_string());
    }
    crate::tool_libs::register_tool_library(&cmd.name, &cmd.source);
    if let Some(journal) = journal.as_ref() {
        crate::registration_journal::record_tool_library(journal, &cmd.name, &cmd.source);
    }
    // Twin persistence: mirror the in-memory registration to
    // `<twin>/tools/<name>.rhai` so it survives a restart (loaded back by the
    // TwinAdded observer). Native only — no filesystem on wasm.
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(root) = ws
        .as_ref()
        .and_then(|ws| ws.active_twin.and_then(|id| ws.twin(id)))
        .map(|twin| twin.root.clone())
    {
        if let Err(e) = crate::tool_libs::save_tool_library_file(&root, &cmd.name, &cmd.source) {
            warn!("[tool_libs] could not persist '{}' to Twin: {e}", cmd.name);
        }
    }
    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({
        "name": cmd.name,
        "libraries": crate::tool_libs::library_names(),
    });
    Ok(ack)
}

/// Run a declarative **mission timeline** on an entity — Layer 2 of the
/// sequencer. The timeline is pure DATA (`timeline` is a JSON string: either a
/// `[ ...steps ]` array or `{ "name": ..., "steps": [ ... ] }`), so a mission is
/// authorable/storable/shippable without writing rhai. The handler lowers it to
/// the generic executor (a `const TIMELINE` + the three hooks that call the
/// prelude's `compile_timeline`/`run_steps`/`seq_note_event`) and attaches it via
/// the same path as `RunScenario` — so hot-reload, per-entity state, and
/// `STEP_COMPLETE`/`SEQUENCE_COMPLETE` telemetry all come for free.
///
/// Step vocabulary (see prelude `timeline_step`): `{move_to,speed,radius}`,
/// `{cmd,params}`, `{emit,value}`, `{wait}`, `{wait_event}`.
#[cfg(feature = "rhai")]
#[Command]
pub struct RunTimeline {
    #[authz_target]
    pub target: Entity,
    /// JSON: a steps array, or an object with a `steps` array (and optional `name`).
    pub timeline: String,
}

/// Serialise a `serde_json::Value` as a rhai literal (object→`#{}`, array→`[]`,
/// string→quoted+escaped, null→`()`). Keys are quoted so reserved words / odd
/// names are safe. Used to embed timeline DATA into the generated executor.
#[cfg(feature = "rhai")]
fn json_to_rhai_literal(v: &serde_json::Value, out: &mut String) {
    use serde_json::Value;
    match v {
        Value::Null => out.push_str("()"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => push_rhai_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json_to_rhai_literal(it, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push_str("#{");
            for (i, (k, val)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                push_rhai_string(k, out);
                out.push(':');
                json_to_rhai_literal(val, out);
            }
            out.push('}');
        }
    }
}

/// Push a rhai string literal with the necessary escapes.
#[cfg(feature = "rhai")]
fn push_rhai_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
}

/// Parse a timeline JSON string into its `steps` array value + step count.
/// Accepts a bare `[ ...steps ]` array or an object with a `steps` array. Shared
/// by `RunTimeline` (execute), `RunStoredTimeline`, and `RegisterTimeline`
/// (validate-before-store). Errors are caller-prefixed.
#[cfg(feature = "rhai")]
fn parse_timeline_steps(timeline: &str) -> Result<(serde_json::Value, usize), String> {
    let parsed: serde_json::Value = serde_json::from_str(timeline)
        .map_err(|e| format!("`timeline` is not valid JSON: {e}"))?;
    let steps = match &parsed {
        serde_json::Value::Array(_) => parsed.clone(),
        serde_json::Value::Object(o) => o
            .get("steps")
            .cloned()
            .ok_or_else(|| "object form needs a `steps` array".to_string())?,
        _ => return Err("`timeline` must be an array or object".to_string()),
    };
    let count = steps
        .as_array()
        .ok_or_else(|| "`steps` must be an array".to_string())?
        .len();
    Ok((steps, count))
}

/// Lower a timeline `steps` array into the generic rhai executor source — a
/// `const TIMELINE` plus the three hooks that call the prelude's
/// `compile_timeline` / `run_steps` / `seq_note_event`. Attaching the result via
/// `attach_rhai_scenario` gives the timeline hot-reload, per-entity state, and
/// `STEP_COMPLETE`/`SEQUENCE_COMPLETE` telemetry for free.
#[cfg(feature = "rhai")]
fn timeline_executor_source(steps: &serde_json::Value) -> String {
    let mut steps_lit = String::new();
    json_to_rhai_literal(steps, &mut steps_lit);
    format!(
        "const TIMELINE = #{{ steps: {steps_lit} }};\n\
         fn on_start(me) {{ this.cur = seq_init(); this.steps = compile_timeline(TIMELINE.steps); }}\n\
         fn on_tick(me) {{ this.cur = run_steps(me, this.steps, this.cur); }}\n\
         fn on_event(me, evt) {{ this.cur = seq_note_event(this.cur, evt); }}\n"
    )
}

#[cfg(feature = "rhai")]
#[on_command(RunTimeline)]
fn on_run_timeline(
    _t: On<RunTimeline>,
    mut registry: ResMut<ScriptRegistry>,
    mut alloc: ResMut<ScenarioDocAllocator>,
    q_existing: Query<&ScriptedModel>,
    guard: Option<Res<lunco_core::session::SyncApplyGuard>>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let (steps, step_count) =
        parse_timeline_steps(&cmd.timeline).map_err(|e| format!("RunTimeline: {e}"))?;
    let source = timeline_executor_source(&steps);
    let (doc_id_raw, generation) = attach_rhai_scenario(
        cmd.target,
        source,
        // Timelines are pure data; the generated executor doesn't read `params`.
        String::new(),
        guard.and_then(|g| g.0),
        &mut registry,
        &mut alloc,
        &q_existing,
        &mut commands,
    );
    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({
        "document_id": doc_id_raw,
        "generation": generation,
        "steps": step_count,
    });
    Ok(ack)
}

/// Save a named mission **timeline** to the Twin — the storage counterpart of
/// `RunTimeline` (which runs an inline one). Validates the JSON parses as a
/// timeline, stores it in the [`crate::timelines::TimelineStore`], and mirrors it
/// to `<twin>/timelines/<name>.json` so it survives a restart (reloaded by the
/// `TwinAdded` observer). Discover with `ListTimelines`/`GetTimeline`, run with
/// `RunStoredTimeline`. Idempotent (re-registering a name replaces it).
#[cfg(feature = "rhai")]
#[Command(default)]
pub struct RegisterTimeline {
    pub name: String,
    /// JSON: a steps array, or an object with a `steps` array (and optional `name`).
    pub timeline: String,
}

#[cfg(feature = "rhai")]
#[on_command(RegisterTimeline)]
#[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
fn on_register_timeline(
    _t: On<RegisterTimeline>,
    mut store: ResMut<crate::timelines::TimelineStore>,
    // Optional: present only with the workspace plugin; used to persist to the
    // active Twin's `timelines/` dir. `None` (headless / no-twin) keeps it in-memory.
    ws: Option<Res<lunco_workspace::WorkspaceResource>>,
    // Journal handle (present once wired). Records the registration as a
    // `DomainKind::Timeline` op so it syncs + persists via the journal plane;
    // fires for LOCAL registrations only (remote ones arrive via the replay leg).
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) -> Result<Ack, String> {
    if cmd.name.is_empty() {
        return Err("RegisterTimeline: `name` must not be empty".to_string());
    }
    // Reject malformed timelines at store time, not at run time.
    parse_timeline_steps(&cmd.timeline).map_err(|e| format!("RegisterTimeline: {e}"))?;
    store.insert(cmd.name.clone(), cmd.timeline.clone());
    if let Some(journal) = journal.as_ref() {
        crate::registration_journal::record_timeline(journal, &cmd.name, &cmd.timeline);
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(root) = ws
        .as_ref()
        .and_then(|ws| ws.active_twin.and_then(|id| ws.twin(id)))
        .map(|twin| twin.root.clone())
    {
        if let Err(e) = crate::timelines::save_timeline_file(&root, &cmd.name, &cmd.timeline) {
            warn!("[timelines] could not persist '{}' to Twin: {e}", cmd.name);
        }
    }
    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({ "name": cmd.name, "timelines": store.names() });
    Ok(ack)
}

/// Run a stored mission timeline on an entity by name (resolved from the
/// [`crate::timelines::TimelineStore`]) — the one-step "fetch + run" for a
/// `RegisterTimeline`d / file-authored mission, sparing callers a
/// `GetTimeline`→`RunTimeline` round-trip. Same execution path as `RunTimeline`.
#[cfg(feature = "rhai")]
#[Command]
pub struct RunStoredTimeline {
    #[authz_target]
    pub target: Entity,
    pub name: String,
}

#[cfg(feature = "rhai")]
#[on_command(RunStoredTimeline)]
fn on_run_stored_timeline(
    _t: On<RunStoredTimeline>,
    store: Res<crate::timelines::TimelineStore>,
    mut registry: ResMut<ScriptRegistry>,
    mut alloc: ResMut<ScenarioDocAllocator>,
    q_existing: Query<&ScriptedModel>,
    guard: Option<Res<lunco_core::session::SyncApplyGuard>>,
    mut commands: Commands,
) -> Result<Ack, String> {
    // Own the JSON so the store borrow is released before we touch the registry.
    let timeline = store
        .get(&cmd.name)
        .ok_or_else(|| format!("RunStoredTimeline: no timeline named '{}'", cmd.name))?
        .to_string();
    let (steps, step_count) =
        parse_timeline_steps(&timeline).map_err(|e| format!("RunStoredTimeline: {e}"))?;
    let source = timeline_executor_source(&steps);
    let (doc_id_raw, generation) = attach_rhai_scenario(
        cmd.target,
        source,
        String::new(),
        guard.and_then(|g| g.0),
        &mut registry,
        &mut alloc,
        &q_existing,
        &mut commands,
    );
    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({
        "name": cmd.name,
        "document_id": doc_id_raw,
        "generation": generation,
        "steps": step_count,
    });
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

/// Pause or resume the scenario attached to `target` (sets `ScriptedModel.paused`).
/// Paused scenarios skip `on_tick` (rhai) / execution (python) but keep their
/// state — resume continues where they left off. The clean API form of toggling
/// the `paused` field; language-agnostic.
#[cfg(any(feature = "rhai", feature = "python"))]
#[Command]
pub struct SetScenarioPaused {
    #[authz_target]
    pub target: Entity,
    pub paused: bool,
}

#[cfg(any(feature = "rhai", feature = "python"))]
#[on_command(SetScenarioPaused)]
fn on_set_scenario_paused(
    _t: On<SetScenarioPaused>,
    mut q: Query<&mut ScriptedModel>,
) -> Result<Ack, String> {
    let mut model = q
        .get_mut(cmd.target)
        .map_err(|_| "SetScenarioPaused: target has no ScriptedModel".to_string())?;
    model.paused = cmd.paused;
    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({ "paused": cmd.paused });
    Ok(ack)
}

/// Stop & detach the scenario from `target` — removes its `ScriptedModel` so it
/// stops ticking. A rhai scenario runs its `on_stop` teardown hook on the next
/// runtime tick (the prune in `tick_rhai_models`). The `ScriptDocument` stays in
/// the registry, so the scenario can be re-attached / re-run later.
#[cfg(any(feature = "rhai", feature = "python"))]
#[Command]
pub struct StopScenario {
    #[authz_target]
    pub target: Entity,
}

#[cfg(any(feature = "rhai", feature = "python"))]
#[on_command(StopScenario)]
fn on_stop_scenario(
    _t: On<StopScenario>,
    mut commands: Commands,
    q: Query<(), With<ScriptedModel>>,
) -> Result<Ack, String> {
    if q.get(cmd.target).is_err() {
        return Err("StopScenario: target has no ScriptedModel".to_string());
    }
    commands.entity(cmd.target).remove::<ScriptedModel>();
    Ok(Ack::new(OpId::new()))
}

/// Declare data-driven RBAC policies for the script-execution commands in the
/// shared [`lunco_core::session::CommandPolicyRegistry`], so script submission is
/// gated through the **same authorization seam** as every other command —
/// instead of sitting at the registry's OPEN default while only the node-role
/// [`crate::scripts_run_here`] condition (which decides *where* scripts run, not
/// *who* may submit them) guards execution.
///
/// Rationale (design §3.4 "Security"): a script body reaches the *entire*
/// `cmd()` surface and executes under host authority, so a script-executing
/// command is a privilege amplifier — a networked `Observer` that may not
/// `SetPorts` directly could otherwise submit a scenario that does. We
/// therefore declare an **`Operator`** floor for the script-executing /
/// disk-persisting commands, and ownership-gated control for scenario lifecycle
/// (which acts on a single `#[authz_target]` entity, exactly like `SetPorts`).
/// Deployments relax or tighten any of these at runtime via
/// [`lunco_core::session::CommandPolicyRegistry::set_override`] with no recompile.
///
/// Scope: only *networked client* submissions reach [`lunco_core::session::authorize`]
/// (the host gate in `lunco-networking`); local host / standalone API + MCP
/// commands never do, so single-player and host-local tooling are unaffected.
#[cfg(any(feature = "rhai", feature = "python"))]
pub(crate) fn register_command_policies(app: &mut App) {
    use lunco_core::session::{AuthorityRole, CommandPolicy, CommandPolicyRegistry};

    // The registry is a `LunCoCorePlugin` resource; init defensively in case the
    // scripting plugin is added first (`init_resource` is idempotent and keeps
    // the existing instance + its baseline `SetPorts` entry).
    app.init_resource::<CommandPolicyRegistry>();
    let mut reg = app.world_mut().resource_mut::<CommandPolicyRegistry>();

    // Executes a script body (full `cmd()` reach under host authority) or
    // persists an authoring artifact to the twin dir → `Operator` floor.
    const EXEC: CommandPolicy =
        CommandPolicy { min_role: AuthorityRole::Operator, ownership_gated: false };

    #[cfg(feature = "rhai")]
    {
        reg.register("RunRhai", EXEC);
        reg.register("RunScenario", EXEC);
        reg.register("RunTimeline", EXEC);
        reg.register("RegisterTimeline", EXEC);
        reg.register("RunStoredTimeline", EXEC);
        reg.register("RegisterToolLibrary", EXEC);
    }
    #[cfg(feature = "python")]
    reg.register("RunPython", EXEC);

    // Scenario lifecycle acts on one `#[authz_target]` entity → ownership-gated
    // control (owner acts at the Observer floor; a non-owner needs `Operator`
    // and is then rejected by the ownership check), mirroring `SetPorts`.
    reg.register("SetScenarioPaused", CommandPolicy::OWNED_CONTROL);
    reg.register("StopScenario", CommandPolicy::OWNED_CONTROL);

    // The structural mutation verbs (`add`/`remove`/`despawn`) restructure a
    // target entity directly via reflection rather than through a command, but
    // are gated through this SAME registry under a well-known capability key
    // (`bridge_core::capability::STRUCTURAL_MUTATE`): ownership-gated control, so
    // a remote script may only restructure entities its launching session owns.
    reg.register(
        crate::bridge_core::capability::STRUCTURAL_MUTATE,
        CommandPolicy::OWNED_CONTROL,
    );
}

// Generates `register_all_commands` for the compiled-in script commands. One
// cfg-exclusive invocation per feature combo so exactly one
// `register_all_commands` is emitted (covers the script-free build too).
#[cfg(all(feature = "rhai", feature = "python"))]
register_commands!(
    on_run_rhai,
    on_run_scenario,
    on_run_timeline,
    on_register_timeline,
    on_run_stored_timeline,
    on_register_tool_library,
    on_run_python,
    on_set_scenario_paused,
    on_stop_scenario
);
#[cfg(all(feature = "rhai", not(feature = "python")))]
register_commands!(
    on_run_rhai,
    on_run_scenario,
    on_run_timeline,
    on_register_timeline,
    on_run_stored_timeline,
    on_register_tool_library,
    on_set_scenario_paused,
    on_stop_scenario
);
#[cfg(all(not(feature = "rhai"), feature = "python"))]
register_commands!(on_run_python, on_set_scenario_paused, on_stop_scenario);
#[cfg(all(not(feature = "rhai"), not(feature = "python")))]
register_commands!();

#[cfg(all(test, feature = "rhai"))]
mod tests {
    //! The JSON→rhai-literal serialiser that `RunTimeline` embeds into the
    //! generated executor. It must produce valid rhai that round-trips the data.

    fn lit(v: &serde_json::Value) -> String {
        let mut s = String::new();
        super::json_to_rhai_literal(v, &mut s);
        s
    }

    #[test]
    fn serialises_scalars_and_nesting() {
        assert_eq!(lit(&serde_json::json!(null)), "()");
        assert_eq!(lit(&serde_json::json!(true)), "true");
        assert_eq!(lit(&serde_json::json!(3)), "3");
        assert_eq!(lit(&serde_json::json!(2.5)), "2.5");
        assert_eq!(lit(&serde_json::json!("hi")), "\"hi\"");
        assert_eq!(lit(&serde_json::json!([1, 2])), "[1,2]");
        // object keys are quoted; one key so order is stable
        assert_eq!(lit(&serde_json::json!({ "wait": 5.0 })), "#{\"wait\":5.0}");
    }

    #[test]
    fn escapes_strings_so_embedding_is_safe() {
        // A value containing a quote/backslash must not break out of the literal.
        let s = lit(&serde_json::json!("a\"b\\c\n"));
        assert_eq!(s, "\"a\\\"b\\\\c\\n\"");
    }

    #[test]
    fn generated_timeline_literal_parses_as_rhai() {
        // The serialised steps array, dropped into the executor template, must
        // compile (proves the literal + template are syntactically valid rhai).
        let steps = serde_json::json!([
            { "move_to": [12.0, 0.0, 0.0], "speed": 1.0, "radius": 2.0 },
            { "wait": 5.0 },
            { "cmd": "SetPorts", "params": {} },
            { "wait_event": "GO" },
        ]);
        let mut steps_lit = String::new();
        super::json_to_rhai_literal(&steps, &mut steps_lit);
        let source = format!("const TIMELINE = #{{ steps: {steps_lit} }};");
        rhai::Engine::new()
            .compile(&source)
            .expect("generated timeline literal must be valid rhai");
    }

    #[test]
    fn script_commands_carry_rbac_policies() {
        use bevy::prelude::*;
        use lunco_core::session::{AuthorityRole, CommandPolicy, CommandPolicyRegistry};

        let mut app = App::new();
        super::register_command_policies(&mut app);
        let reg = app.world().resource::<CommandPolicyRegistry>();

        // Script-executing / disk-persisting commands carry an Operator floor:
        // a body reaches the whole cmd() surface under host authority.
        let exec = CommandPolicy { min_role: AuthorityRole::Operator, ownership_gated: false };
        for c in [
            "RunRhai",
            "RunScenario",
            "RunTimeline",
            "RegisterTimeline",
            "RunStoredTimeline",
            "RegisterToolLibrary",
        ] {
            assert_eq!(reg.policy_for(c), exec, "{c} should require Operator");
        }

        // Scenario lifecycle is ownership-gated control, like SetPorts.
        assert_eq!(reg.policy_for("SetScenarioPaused"), CommandPolicy::OWNED_CONTROL);
        assert_eq!(reg.policy_for("StopScenario"), CommandPolicy::OWNED_CONTROL);

        // The structural mutation verbs share the registry under a capability key,
        // ownership-gated so a remote script only restructures what it owns.
        assert_eq!(
            reg.policy_for(crate::bridge_core::capability::STRUCTURAL_MUTATE),
            CommandPolicy::OWNED_CONTROL,
        );

        // The baseline core entries survive our defensive init_resource.
        assert_eq!(reg.policy_for("SetPorts"), CommandPolicy::OWNED_CONTROL);
        // An undeclared command stays OPEN (the RBAC-readiness invariant).
        assert_eq!(reg.policy_for("SomeUngatedQuery"), CommandPolicy::OPEN);
    }
}
