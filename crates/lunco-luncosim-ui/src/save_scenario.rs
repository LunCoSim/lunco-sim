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
/// Only doc-backed Twin scenes have an editable document; a raw-file scene is
/// refused (logged, not silently dropped), matching the rule that the builder
/// must only edit doc-backed scenes or it eats work on the next reload.
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
    backed: Res<lunco_usd::twin_projection::DocBackedTwinScenes>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    let target = trigger.event().target;

    // 1. The live source the runtime is currently running for this entity.
    let Ok(model) = q_model.get(target) else {
        warn!("[save-scenario] entity {target} has no scenario attached");
        return;
    };
    let Some(doc_id) = model.document_id else {
        warn!("[save-scenario] entity {target}'s scenario has no document");
        return;
    };
    let Some(host) = registry.documents.get(&lunco_doc::DocumentId::new(doc_id)) else {
        warn!("[save-scenario] no script document {doc_id} for entity {target}");
        return;
    };
    let source = host.document().source.clone();

    // 2. The prim to author onto + the editable scene document behind it.
    let Ok(upp) = q_prim.get(target) else {
        warn!("[save-scenario] entity {target} is not a USD-backed prim — nothing to save onto");
        return;
    };
    let Some(scene_doc) = lunco_usd::twin_projection::scene_document_for(
        &backed,
        &asset_server,
        upp.stage_handle.id(),
    ) else {
        warn!(
            "[save-scenario] the scene backing {target} is a raw-file scene (not doc-backed) — \
             open it as a Twin to save scenarios in place"
        );
        return;
    };

    // 3. Convert the PROGRAM prim to the selected inline `sourceCode` arm
    // (root layer → durable in the `.usda` on SaveDocument). The shared
    // lowering clears the previous id/asset arms before selecting the new
    // source, so the composed program has one unambiguous implementation.
    let Ok(program) = q_program.get(target) else {
        warn!(
            "[save-scenario] entity {target} runs a scenario that came from no program prim \
             (it was started at runtime, not authored in the scene) — nothing to save onto"
        );
        return;
    };
    commands.trigger(lunco_usd_core::commands::ApplyUsdOps {
        doc_id: scene_doc,
        parent_gen: None,
        label: "Save scenario source".into(),
        ops: lunco_usd_core::program::inline_program_source_ops(
            lunco_usd_core::LayerId::root(),
            program.0.clone(),
            source,
        ),
    });
    info!(
        "[save-scenario] {target}: scenario source written onto `{}` (doc {}) — journals; SaveDocument persists to disk",
        program.0, scene_doc.0
    );
}

lunco_core::register_commands!(on_save_scenario);
