//! Command that persists a live Rhai scenario edit onto its USD program prim.

use bevy::prelude::*;

/// Save a live-edited Rhai scenario's current source back onto the
/// `LunCoProgramAPI` prim it came from — the other half of scenario authoring.
///
/// The shared USD lowering selects `info:sourceCode` and clears the old
/// `info:id` and `info:sourceAsset` arms. The `string` value is authored RAW,
/// so the whole Rhai source round-trips verbatim, journals like any edit, and
/// reaches the `.usda` on `SaveDocument`.
///
/// It authors onto the PROGRAM, not onto the vessel running it
/// ([`lunco_core::ScenarioProgramPrim`] carries the path): a vessel can run
/// several programs, and a source written onto the vessel would sit on a prim
/// that runs nothing.
///
/// Only doc-backed Twin scenes have an editable document. A raw-file scene is
/// rejected terminally. Completion returns the USD owner's applied document and
/// generation, or its authoring error; `SaveDocument` owns disk persistence.
#[lunco_core::Command]
pub struct SaveScenario {
    /// The scripted entity whose live scenario source to persist onto its prim.
    /// Ownership-gated (same as `RunScenario`): saving a scenario is editing it.
    #[authz_target]
    pub target: Entity,
}

impl Default for SaveScenario {
    // `#[Command]` needs a Default for Reflect; `Entity` has none. The
    // placeholder is never dispatched — a real save carries the selected entity.
    fn default() -> Self {
        Self {
            target: Entity::PLACEHOLDER,
        }
    }
}

#[lunco_core::on_command(SaveScenario)]
fn on_save_scenario(
    trigger: On<SaveScenario>,
    q_model: Query<&lunco_scripting::doc::ScriptedModel>,
    q_prim: Query<&lunco_usd_bevy_scene::UsdPrimPath>,
    q_program: Query<&lunco_core::ScenarioProgramPrim>,
    registry: Res<lunco_scripting::ScriptRegistry>,
    backed: Res<lunco_usd_bevy_twin::DocBackedTwinScenes>,
    mut commands: Commands,
    active: Res<lunco_core::ActiveCommandId>,
    pending: Option<Res<lunco_api::executor::PendingApiRequest>>,
) -> Result<lunco_core::CommandOutcome, String> {
    let target = trigger.event().target;

    // 1. The live source the runtime is currently running for this entity.
    let model = q_model
        .get(target)
        .map_err(|_| "Selected entity has no scenario attached".to_owned())?;
    let doc_id = model
        .document_id
        .ok_or_else(|| "Selected scenario has no source document".to_owned())?;
    let host = registry
        .documents
        .get(&lunco_doc::DocumentId::new(doc_id))
        .ok_or_else(|| "Selected scenario source document is no longer open".to_owned())?;
    let source = host.document().source.clone();

    // 2. The prim to author onto + the editable scene document behind it.
    let upp = q_prim
        .get(target)
        .map_err(|_| "Selected scenario is not backed by a USD prim".to_owned())?;
    let scene_doc = lunco_usd_bevy_twin::scene_document_for(&backed, upp.stage_handle.id())
        .ok_or_else(|| "Open this scene as a Twin before applying behaviour source".to_owned())?;
    let program = q_program
        .get(target)
        .map_err(|_| "Selected scenario has no authored program prim".to_owned())?;
    let apply = lunco_usd_core::commands::ApplyUsdOps {
        doc_id: scene_doc,
        parent_gen: None,
        label: "Save scenario source".into(),
        ops: lunco_usd_core::program::inline_program_source_ops(
            lunco_usd_document::document::LayerId::root(),
            program.0.clone(),
            source,
        ),
    };
    let active = active.as_ref().clone();
    let correlation = pending
        .map(|pending| pending.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        lunco_api::executor::trigger_command_with_context(world, apply, active, correlation);
    });
    Ok(lunco_core::CommandOutcome::Pending)
}

lunco_core::register_commands!(on_save_scenario);
