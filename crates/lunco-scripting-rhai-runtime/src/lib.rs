//! Production Rhai runtime integration for LunCoSim.
//!
//! The language-neutral scripting package owns documents, backend-neutral
//! lifecycle state, and Python support. This package owns the high-churn Rhai
//! authored source graph, commands, tools, and timelines. The reusable world
//! bridge and policy runtime lives in `lunco-scripting-rhai-world`, so changes
//! to application composition do not rebuild its high-churn world closure.

#[cfg(feature = "rhai")]
use bevy::prelude::*;
#[cfg(feature = "rhai")]
use lunco_api::executor::DeferredCommandAppExt;

#[cfg(feature = "rhai")]
pub mod commands;
#[cfg(feature = "rhai")]
pub mod registration_journal;
#[cfg(feature = "rhai")]
pub mod timelines;

/// Install the Rhai runtime and its authored policy/application seams.
///
/// The language-neutral [`lunco_scripting::LunCoScriptingPlugin`] is installed
/// automatically when it is not already present, so standalone hosts cannot
/// accidentally register only half of the scripting lifecycle.
#[cfg(feature = "rhai")]
#[derive(Default)]
pub struct LunCoScriptingRhaiRuntimePlugin;

#[cfg(feature = "rhai")]
impl Plugin for LunCoScriptingRhaiRuntimePlugin {
    fn build(&self, app: &mut App) {
        // Rhai's `cmd()` uses the transport-free reflected command core. Keep
        // this runtime usable without an HTTP/API transport while composing
        // safely with applications that already installed the same core.
        lunco_api::ensure_command_core(app);
        if !app.is_plugin_added::<lunco_scripting::LunCoScriptingPlugin>() {
            app.add_plugins(lunco_scripting::LunCoScriptingPlugin);
        }
        if !app.is_plugin_added::<lunco_core_runtime::AsyncWorkAdmissionPlugin>() {
            app.add_plugins(lunco_core_runtime::AsyncWorkAdmissionPlugin);
        }
        app.init_resource::<lunco_core_runtime::SimulationProgress>();

        if !app.is_plugin_added::<lunco_scripting_rhai_world::source_asset::RhaiSourceAssetPlugin>()
        {
            app.add_plugins(lunco_scripting_rhai_world::source_asset::RhaiSourceAssetPlugin);
        }
        lunco_scripting_rhai_world::tool_libs::register_native_builtins();
        app.init_resource::<lunco_doc_bevy::DocumentDiagnostics>()
            .init_resource::<lunco_scripting_rhai_world::policy::ScriptedPolicyRegistry>()
            .init_resource::<lunco_scripting_rhai_world::policy::PendingTwinPolicyCommands>()
            .init_resource::<lunco_scripting_rhai_world::world_bridge::PendingWorldScripts>()
            .init_resource::<lunco_scripting_rhai_world::world_bridge::WorldScriptExecutionLimits>()
            .init_resource::<lunco_scripting_rhai_world::world_bridge::RhaiRuntimeStatus>();
        #[cfg(feature = "native-plugins")]
        app.init_resource::<lunco_scripting_rhai_world::native_plugins::NativeTwinPlugins>();
        app.add_systems(
            PreStartup,
            lunco_scripting_rhai_world::policy::load_application_policies_on_startup,
        )
        .add_observer(dispatch_workbench_menu_action)
        .add_observer(lunco_scripting_rhai_world::policy::handle_application_json_scope_loading)
        .add_observer(lunco_scripting_rhai_world::policy::handle_application_json_scope_changed)
        .add_observer(lunco_scripting_rhai_world::policy::handle_application_scene_asset_lifecycle)
        .add_observer(lunco_scripting_rhai_world::policy::sync_policies_on_twin_added)
        .add_observer(lunco_scripting_rhai_world::policy::plan_twin_asset_loading)
        .add_observer(lunco_scripting_rhai_world::policy::wind_down_policies_on_twin_closed)
        .register_deferred_command::<commands::RunRhai>()
        .register_deferred_command::<commands::RunRhaiTool>()
        .register_deferred_command::<commands::RunRhaiToolHook>()
        .init_resource::<lunco_scripting::scenario::ScenarioDriver<
            lunco_scripting_rhai_world::world_bridge::RhaiScenarioRuntime,
        >>();

        let sources = app
            .world()
            .resource::<lunco_scripting::scenario::ScenarioDriver<
                lunco_scripting_rhai_world::world_bridge::RhaiScenarioRuntime,
            >>()
            .runtime
            .script_sources();
        app.insert_resource(sources)
            .init_resource::<lunco_scripting::scenario::ScriptEventInbox>()
            .add_observer(lunco_scripting::scenario::collect_script_events);

        lunco_scripting_rhai_world::tool_libs::register_queries(app);
        app.init_resource::<lunco_scripting_rhai_world::tool_libs::TwinToolLibraries>()
            .init_resource::<timelines::TimelineStore>();
        lunco_scripting_rhai_world::tool_libs::register_twin_tool_loading(app);
        timelines::register_queries(app);
        timelines::register_twin_timeline_loading(app);
        app.add_systems(lunco_core::SceneTeardown, stop_scene_owned_scripts)
            .add_systems(
                Update,
                lunco_scripting_rhai_world::world_bridge::prepare_builtin_rhai_assets
                    .in_set(lunco_scripting_rhai_world::world_bridge::RhaiBuiltinPreparationSet)
                    .after(lunco_scripting_rhai_world::source_asset::RhaiSourceAssetSet)
                    .before(lunco_core::RuntimeCycleSet::Repl),
            )
            .add_systems(
                Update,
                lunco_scripting_rhai_world::world_bridge::drain_world_scripts
                    .in_set(lunco_core::RuntimeCycleSet::Repl)
                    .run_if(scripts_run_here)
                    .run_if(world_scripts_are_queued),
            )
            .add_systems(
                Update,
                lunco_scripting_rhai_world::policy::apply_twin_policy_commands
                    .in_set(lunco_core::RuntimeCycleSet::Command),
            )
            .add_systems(
                PreUpdate,
                (
                    commands::resolve_embedded_scenario_paths,
                    commands::attach_requested_scenarios,
                    commands::attach_embedded_scenarios,
                )
                    .chain(),
            )
            .add_systems(
                PreUpdate,
                lunco_scripting_rhai_world::world_bridge::prepare_rhai_scenario_compiles
                    .after(commands::attach_embedded_scenarios)
                    .after(lunco_scripting::scenario::open_scenarios_when_scene_ready)
                    .before(lunco_time::TimeSpineSet),
            )
            .add_systems(
                FixedUpdate,
                lunco_scripting_rhai_world::world_bridge::tick_rhai_scenarios
                    .in_set(lunco_core::RuntimeCycleSet::Simulation)
                    .in_set(lunco_scripting::ScriptingSet)
                    .run_if(lunco_scripting::scenario::scenario_execution_enabled)
                    .run_if(lunco_scripting_rhai_world::world_bridge::rhai_runtime_ready)
                    .run_if(lunco_scripting::scenario::simulation_is_running),
            )
            .add_systems(
                Update,
                lunco_scripting_rhai_world::world_bridge::tick_rhai_scenarios_while_paused
                    .in_set(lunco_core::RuntimeCycleSet::Lifecycle)
                    .run_if(lunco_scripting::scenario::scenario_execution_enabled)
                    .run_if(lunco_scripting_rhai_world::world_bridge::rhai_runtime_ready)
                    .run_if(lunco_scripting::scenario::simulation_is_paused),
            );

        commands::register_all_commands(app);
        commands::register_command_policies(app);
    }
}

#[cfg(feature = "rhai")]
fn dispatch_workbench_menu_action(
    trigger: On<lunco_scripting_rhai_core::ui_bridge::ScriptUiRequest>,
    mut commands: Commands,
) {
    let lunco_scripting_rhai_core::ui_bridge::ScriptUiRequest::WorkbenchMenuAction {
        tool,
        hook,
        args,
    } = trigger.event()
    else {
        return;
    };
    commands.trigger(commands::RunRhaiToolHook {
        tool: tool.clone(),
        hook: hook.clone(),
        args: args.clone(),
    });
}

/// Stop and close scripts owned by the outgoing USD scene before its entities
/// are reclaimed.
#[cfg(feature = "rhai")]
pub fn stop_scene_owned_scripts(world: &mut World) {
    let targets: Vec<(Entity, Option<u64>)> = {
        let mut query = world
            .query_filtered::<(Entity, Option<&lunco_scripting::doc::ScriptedModel>),
                With<lunco_scripting::SceneOwnedScript>>();
        query
            .iter(world)
            .map(|(entity, model)| (entity, model.and_then(|model| model.document_id)))
            .collect()
    };

    for (entity, document_id) in targets {
        lunco_scripting::scenario::ScenarioDriver::<
            lunco_scripting_rhai_world::world_bridge::RhaiScenarioRuntime,
        >::stop_entity(world, entity);
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<lunco_scripting::doc::ScriptedModel>();
        }
        if let Some(document_id) = document_id {
            if let Some(mut registry) = world.get_resource_mut::<lunco_scripting::ScriptRegistry>()
            {
                registry
                    .documents
                    .remove(&lunco_doc::DocumentId::new(document_id));
            }
        }
    }
}

/// Run scripts only on the authoritative host or in standalone mode.
#[cfg(feature = "rhai")]
fn scripts_run_here(role: Option<Res<lunco_core_session::NetworkRole>>) -> bool {
    !matches!(
        role.as_deref(),
        Some(lunco_core_session::NetworkRole::Client)
    )
}

/// Avoid scheduling the exclusive live-world drain on frames with no REPL work.
#[cfg(feature = "rhai")]
fn world_scripts_are_queued(
    pending: Res<lunco_scripting_rhai_world::world_bridge::PendingWorldScripts>,
) -> bool {
    pending.has_pending()
}

#[cfg(all(test, feature = "rhai"))]
mod tests {
    use bevy::prelude::{App, IntoScheduleConfigs, ResMut, Resource, Update};
    use lunco_scripting_rhai_world::world_bridge::{
        PendingWorldScript, PendingWorldScripts, WorldScriptExecutionLimits,
    };

    use super::world_scripts_are_queued;

    #[derive(Resource, Default)]
    struct Scheduled(usize);

    fn count_schedule_run(mut count: ResMut<Scheduled>) {
        count.0 += 1;
    }

    #[test]
    fn empty_repl_queue_does_not_schedule_its_exclusive_drain() {
        let mut app = App::new();
        app.init_resource::<PendingWorldScripts>();
        app.init_resource::<WorldScriptExecutionLimits>();
        app.init_resource::<Scheduled>();
        app.add_systems(Update, count_schedule_run.run_if(world_scripts_are_queued));

        app.update();
        assert_eq!(app.world().resource::<Scheduled>().0, 0);

        app.world_mut()
            .resource_mut::<PendingWorldScripts>()
            .enqueue(
                PendingWorldScript::Code {
                    id: 0,
                    code: String::new(),
                    authority: None,
                    correlation_id: None,
                },
                WorldScriptExecutionLimits::default(),
            )
            .expect("one request fits the default queue");
        app.update();
        assert_eq!(app.world().resource::<Scheduled>().0, 1);
    }
}
