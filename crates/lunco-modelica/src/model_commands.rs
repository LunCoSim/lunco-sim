//! Core (UI-free) Modelica command helpers shared by the egui workbench AND the
//! headless API server.
//!
//! These carry no egui — they read document/registry/runner state and mutate a
//! `ModelicaModel` — but used to live under `ui::commands`. A headless Modelica
//! compile/run server (`lunica --no-ui`, `lunco-usd-sim` on a server) needs
//! `apply_set_model_input` (the `SetModelInput` API command) and the simulation-
//! bounds resolution, so they belong in the core, not behind the `ui` feature.
//! The egui command *structs*/observers that wrap them stay in `ui::commands`.

use bevy::prelude::*;
use lunco_doc::DocumentId;

use crate::state::ModelicaDocumentRegistry;

// ─── SetModelInput ───────────────────────────────────────────────────────────

/// Why [`apply_set_model_input`] could not apply the value.
#[derive(Debug, Clone)]
pub enum SetModelInputError {
    /// No `doc` passed and no active document to fall back to.
    NoActiveDocument,
    /// The document has no compiled/linked entity yet.
    NoLinkedEntity {
        /// Raw document id.
        doc: u64,
    },
    /// The linked entity is missing its `ModelicaModel` component.
    EntityMissingModel {
        /// Raw document id.
        doc: u64,
    },
    /// The named input isn't declared on the model.
    UnknownInput {
        /// Raw document id.
        doc: u64,
        /// The rejected input name.
        name: String,
        /// The model the lookup ran against.
        model_name: String,
        /// Inputs that *are* declared (for a helpful error).
        known_inputs: Vec<String>,
    },
}

impl SetModelInputError {
    /// Human-readable, API-friendly message.
    pub fn message(&self) -> String {
        match self {
            Self::NoActiveDocument => "no active document (pass `doc` explicitly)".into(),
            Self::NoLinkedEntity { doc } => format!(
                "doc {doc} has no linked entity — compile the model before setting inputs"
            ),
            Self::EntityMissingModel { doc } => format!(
                "doc {doc}'s linked entity has no `ModelicaModel` component"
            ),
            Self::UnknownInput { name, model_name, known_inputs, .. } => format!(
                "input `{name}` not declared on `{model_name}`. \
                 Known inputs: [{}]",
                known_inputs.join(", ")
            ),
        }
    }
}

/// Push a runtime input value into a compiled model's stepper. `doc_raw`
/// unassigned ⇒ fall back to the workspace's active document.
pub fn apply_set_model_input(
    world: &mut World,
    doc_raw: DocumentId,
    name: &str,
    value: f64,
) -> Result<DocumentId, SetModelInputError> {
    let doc = if doc_raw.is_unassigned() {
        // Active-document fallback (UI-free: reads the workspace resource).
        world
            .get_resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|ws| ws.active_document)
            .ok_or(SetModelInputError::NoActiveDocument)?
    } else {
        doc_raw
    };
    let entity = {
        let registry = world.resource::<ModelicaDocumentRegistry>();
        let entities = registry.entities_linked_to(doc);
        match entities.first().copied() {
            Some(e) => e,
            None => return Err(SetModelInputError::NoLinkedEntity { doc: doc.raw() }),
        }
    };

    // Port-first (doc 34, Decision 2). Route the write through the shared
    // `PortRegistry` so it lands in `SimComponent.inputs` — the source of truth
    // the co-sim sync (`sync_modelica_inputs`) copies into `ModelicaModel.inputs`
    // every tick. A *direct* `ModelicaModel.inputs` write would be clobbered
    // within one frame on any co-sim'd entity (wired lander, rover, …). Bare
    // workbench / batch models have no registered port, so `write_port` returns
    // false and we fall through to the direct write below (which also owns the
    // friendly `UnknownInput` validation for the no-cosim case).
    if let Some(registry) = world.get_resource::<lunco_core::ports::PortRegistry>().cloned() {
        if registry.write_port(world, entity, name, value) {
            bevy::log::debug!("[SetModelInput] doc={} {}={} (via port)", doc.raw(), name, value);
            return Ok(doc);
        }
    }

    let Some(mut model) = world.get_mut::<crate::ModelicaModel>(entity) else {
        return Err(SetModelInputError::EntityMissingModel { doc: doc.raw() });
    };
    if !model.inputs.contains_key(name) {
        let known: Vec<String> = model.inputs.keys().cloned().collect();
        return Err(SetModelInputError::UnknownInput {
            doc: doc.raw(),
            name: name.to_string(),
            model_name: model.model_name.clone(),
            known_inputs: known,
        });
    }
    model.inputs.insert(name.to_string(), value);
    bevy::log::debug!("[SetModelInput] doc={} {}={}", doc.raw(), name, value);
    Ok(doc)
}

// ─── Simulation bounds resolution ────────────────────────────────────────────

/// Read the `experiment(...)` annotation bounds for a model from live document
/// state. `None` if the class or annotation is absent.
///
/// Callers are the egui workbench (`ui::commands::compile`) and the API query
/// path (`api_queries`, behind modelica's `lunco-api`); a pure compile-core
/// build (e.g. the physics sandbox server) links neither, hence `allow(dead_code)`.
/// Generic over the resource-read context (see
/// [`crate::sim_default::ResourceRead`]) so the egui panels (`PanelCtx`) and
/// the `&World` callers resolve annotation bounds through one body, not
/// hand-synced copies.
#[allow(dead_code)]
pub(crate) fn bounds_from_annotation_in<R: crate::sim_default::ResourceRead>(
    ctx: &R,
    doc: DocumentId,
    model_ref: &lunco_experiments::ModelRef,
) -> Option<lunco_experiments::RunBounds> {
    let reg = ctx.read_resource::<crate::state::ModelicaDocumentRegistry>()?;
    let host = reg.host(doc)?;
    let index = host.document().index();
    let class = index
        .classes
        .get(&model_ref.0)
        .or_else(|| index.classes.values().find(|c| c.name == model_ref.0))?;
    let exp = class.experiment.as_ref()?;
    // World-gathering done; the annotation→bounds mapping is pure.
    crate::sim_target::bounds_from_experiment(exp)
}

/// `&World` reader for the `experiment(...)` annotation bounds — see
/// [`bounds_from_annotation_in`]. `None` if the class or annotation is absent.
///
/// Callers are the egui workbench (`ui::commands::compile`) and the API query
/// path (`api_queries`, behind modelica's `lunco-api`); a pure compile-core
/// build (e.g. the physics sandbox server) links neither, hence `allow(dead_code)`.
#[allow(dead_code)]
pub(crate) fn bounds_from_annotation(
    world: &World,
    doc: DocumentId,
    model_ref: &lunco_experiments::ModelRef,
) -> Option<lunco_experiments::RunBounds> {
    bounds_from_annotation_in(world, doc, model_ref)
}

/// Single source of truth for the simulation bounds shown in BOTH the Fast Run
/// popup and the Experiments-tab Setup form, so the two surfaces never disagree.
/// Precedence: saved draft override → AST `experiment(...)` annotation → runner
/// annotation cache (`default_bounds`) → `sim_target::DEFAULT_STOP_TIME` (1 s,
/// the Modelica spec default) fallback.
///
/// The fresh AST annotation outranks the async runner cache deliberately: the
/// cache is populated by a background worker callback, so letting it win would
/// make a run's snapshotted bounds depend on dispatch timing (the flaky-
/// terminator race). See [`crate::sim_target::resolve_bounds`].
///
/// Generic over the resource-read context (see
/// [`crate::sim_default::ResourceRead`]) so the Experiments Setup form resolves
/// through `PanelCtx` during paint without a second inlined copy of this
/// precedence — exactly the disagreement this helper exists to prevent.
#[allow(dead_code)]
pub(crate) fn resolve_setup_bounds_in<R: crate::sim_default::ResourceRead>(
    ctx: &R,
    doc: DocumentId,
    model_ref: &lunco_experiments::ModelRef,
) -> lunco_experiments::RunBounds {
    use lunco_experiments::ExperimentRunner;
    let draft = ctx
        .read_resource::<crate::experiments_runner::ExperimentDrafts>()
        .and_then(|d| d.get(doc, model_ref).and_then(|dr| dr.bounds_override.clone()));
    let annotation = bounds_from_annotation_in(ctx, doc, model_ref);
    let runner_cached = ctx
        .read_resource::<crate::ModelicaRunnerResource>()
        .and_then(|r| r.0.default_bounds(model_ref));
    crate::sim_target::resolve_bounds(draft, annotation, runner_cached)
}

/// `&World` reader for the canonical setup bounds — see [`resolve_setup_bounds_in`].
#[allow(dead_code)] // see `bounds_from_annotation` — no caller in a pure compile-core build
pub(crate) fn resolve_setup_bounds(
    world: &World,
    doc: DocumentId,
    model_ref: &lunco_experiments::ModelRef,
) -> lunco_experiments::RunBounds {
    resolve_setup_bounds_in(world, doc, model_ref)
}
