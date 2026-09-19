//! Commands owned by the language-neutral scripting host.
//!
//! These commands operate on [`ScriptedModel`] and [`ScriptRegistry`] without
//! depending on a particular interpreter.  Interpreter-specific commands live
//! in the corresponding runtime package.

#![cfg(any(feature = "rhai", feature = "python"))]

#[cfg(feature = "python")]
use crate::backend::ScriptBackends;
#[cfg(feature = "python")]
use crate::doc::ScriptLanguage;
use crate::ScriptRegistry;
use bevy::prelude::*;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{on_command, register_commands, Command};
use lunco_doc_bevy::{RedoDocument, UndoDocument};
use lunco_hooks::HookValue;

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
    Ok(Ack::with_data(
        OpId::new(),
        HookValue::map([("stdout", HookValue::Str(stdout))]),
    ))
}

/// Script documents own their history just like USD and Modelica documents.
/// This observer is intentionally a no-op for other document domains; their
/// owners receive the same generic command and apply their own host stack.
#[on_command(UndoDocument)]
fn on_undo_script_document(
    trigger: On<UndoDocument>,
    mut registry: ResMut<ScriptRegistry>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) {
    let doc = trigger.event().doc_id;
    if registry.documents.get(&doc).is_none() {
        return;
    }
    let mut apply = || {
        registry
            .documents
            .get_mut(&doc)
            .map_or(Ok(false), |host| host.undo())
    };
    let outcome = match journal {
        Some(journal) => journal
            .as_ref()
            .change_set(format!("Undo script document {doc}"), apply),
        None => apply(),
    };
    match outcome {
        Ok(true) => info!("[script] undo applied on {doc}"),
        Ok(false) => info!("[script] nothing to undo on {doc}"),
        Err(error) => warn!("[script] undo failed on {doc}: {error:?}"),
    }
}

#[on_command(RedoDocument)]
fn on_redo_script_document(
    trigger: On<RedoDocument>,
    mut registry: ResMut<ScriptRegistry>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) {
    let doc = trigger.event().doc_id;
    if registry.documents.get(&doc).is_none() {
        return;
    }
    let mut apply = || {
        registry
            .documents
            .get_mut(&doc)
            .map_or(Ok(false), |host| host.redo())
    };
    let outcome = match journal {
        Some(journal) => journal
            .as_ref()
            .change_set(format!("Redo script document {doc}"), apply),
        None => apply(),
    };
    match outcome {
        Ok(true) => info!("[script] redo applied on {doc}"),
        Ok(false) => info!("[script] nothing to redo on {doc}"),
        Err(error) => warn!("[script] redo failed on {doc}: {error:?}"),
    }
}

/// Pause or resume the scenario attached to `target`.
#[Command]
pub struct SetScenarioPaused {
    #[authz_target]
    pub target: Entity,
    pub paused: bool,
}

#[on_command(SetScenarioPaused)]
fn on_set_scenario_paused(
    _t: On<SetScenarioPaused>,
    mut q: Query<&mut crate::doc::ScriptedModel>,
) -> Result<Ack, String> {
    let mut model = q
        .get_mut(cmd.target)
        .map_err(|_| "SetScenarioPaused: target has no ScriptedModel".to_string())?;
    model.paused = cmd.paused;
    Ok(Ack::with_data(
        OpId::new(),
        HookValue::map([("paused", HookValue::Bool(cmd.paused))]),
    ))
}

/// Stop and detach the scenario from `target`.
#[Command]
pub struct StopScenario {
    #[authz_target]
    pub target: Entity,
}

#[on_command(StopScenario)]
fn on_stop_scenario(
    _t: On<StopScenario>,
    mut commands: Commands,
    q: Query<(), With<crate::doc::ScriptedModel>>,
) -> Result<Ack, String> {
    if q.get(cmd.target).is_err() {
        return Err("StopScenario: target has no ScriptedModel".to_string());
    }
    commands
        .entity(cmd.target)
        .remove::<crate::doc::ScriptedModel>();
    Ok(Ack::new(OpId::new()))
}

#[cfg(feature = "python")]
register_commands!(
    on_undo_script_document,
    on_redo_script_document,
    on_run_python,
    on_set_scenario_paused,
    on_stop_scenario
);

#[cfg(not(feature = "python"))]
register_commands!(
    on_undo_script_document,
    on_redo_script_document,
    on_set_scenario_paused,
    on_stop_scenario
);

pub(crate) fn register_command_policies(app: &mut App) {
    #[cfg(feature = "python")]
    use lunco_core_session::AuthorityRole;
    use lunco_core_session::{CommandPolicy, CommandPolicyRegistry};

    app.init_resource::<CommandPolicyRegistry>();
    let mut reg = app.world_mut().resource_mut::<CommandPolicyRegistry>();
    #[cfg(feature = "python")]
    reg.register(
        "RunPython",
        CommandPolicy {
            min_role: AuthorityRole::Operator,
            ownership_gated: false,
        },
    );
    reg.register("SetScenarioPaused", CommandPolicy::OWNED_CONTROL);
    reg.register("StopScenario", CommandPolicy::OWNED_CONTROL);
}
