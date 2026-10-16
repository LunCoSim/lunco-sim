//! Generic scene context-menu host.
//!
//! Scripts author the labels and typed tool-hook actions. This module only
//! anchors a popup at the pointer location and dispatches the selected generic
//! hook, so the editor does not acquire waypoint, vehicle, or program policy.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use lunco_scripting_bridge_core::{ToolHookBinding, ToolHookBindingRetention};
use lunco_scripting_rhai_core::ui_bridge::{ScriptMenuItem, ScriptUiRequest};
use lunco_workbench_core::scene_pick::ScenePickGate;
use lunco_workbench_runtime_ui::RuntimeUiActionScope;

#[derive(Clone, Debug)]
struct PendingMenu {
    root: Entity,
    scope: RuntimeUiActionScope,
    screen_position: [f32; 2],
    items: Vec<(Entity, ScriptMenuItem)>,
    dismiss_armed: bool,
}

#[derive(Resource, Default)]
pub struct SceneContextMenuState {
    pending: Option<PendingMenu>,
}

impl lunco_core::SceneResource for SceneContextMenuState {}
impl SceneContextMenuState {
    fn clear(&mut self, commands: &mut Commands) {
        if let Some(menu) = self.pending.take() {
            commands.entity(menu.root).try_despawn();
        }
    }
}

pub fn on_script_ui_request(
    trigger: On<ScriptUiRequest>,
    mut state: ResMut<SceneContextMenuState>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    scene: Option<Res<lunco_core::SceneTransitionCoordinator>>,
    mut commands: Commands,
) {
    match trigger.event() {
        ScriptUiRequest::ContextMenu {
            screen_position,
            items,
        } => {
            let scope = RuntimeUiActionScope::capture(workspace.as_deref(), scene.as_deref());
            if let Err(error) = scope.validate(workspace.as_deref(), scene.as_deref()) {
                warn!("[scene-menu] {error}");
                return;
            }
            let bindings: Result<Vec<_>, _> = items
                .iter()
                .map(|item| {
                    ToolHookBinding::for_scene(
                        item.tool.clone(),
                        item.hook.clone(),
                        scope.owner_twin_id,
                        ToolHookBindingRetention::Invocation,
                        scene.as_deref(),
                    )
                })
                .collect();
            let bindings = match bindings {
                Ok(bindings) => bindings,
                Err(error) => {
                    warn!("[scene-menu] {error}");
                    return;
                }
            };
            state.clear(&mut commands);
            let root = commands
                .spawn((lunco_core::Provenance::Local, lunco_core::SceneOwned))
                .id();
            let items = items
                .iter()
                .cloned()
                .zip(bindings)
                .map(|(item, binding)| {
                    let entity = commands
                        .spawn((binding, ChildOf(root), lunco_core::Provenance::Local))
                        .id();
                    (entity, item)
                })
                .collect();
            state.pending = Some(PendingMenu {
                root,
                scope,
                screen_position: *screen_position,
                items,
                // The request is raised from the same pointer transaction
                // that opened the menu. Do not treat that transaction as an
                // outside click when the popup is first painted.
                dismiss_armed: false,
            });
        }
        _ => {}
    }
}

pub fn clear_on_scene_transition(
    _: On<lunco_core::SceneTransitionStarted>,
    mut state: ResMut<SceneContextMenuState>,
    mut commands: Commands,
) {
    state.clear(&mut commands);
}

pub fn clear_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    mut state: ResMut<SceneContextMenuState>,
    mut commands: Commands,
) {
    if state
        .pending
        .as_ref()
        .is_some_and(|menu| menu.scope.owner_twin_id == Some(trigger.event().twin.raw()))
    {
        state.clear(&mut commands);
    }
}

pub fn draw_scene_context_menu(
    mut egui_contexts: EguiContexts,
    mut state: ResMut<SceneContextMenuState>,
    mut scene_pick: ResMut<ScenePickGate>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    scene: Option<Res<lunco_core::SceneTransitionCoordinator>>,
    bindings: Query<&ToolHookBinding>,
    mut commands: Commands,
) {
    let Some(menu) = state.pending.clone() else {
        return;
    };
    if menu
        .scope
        .validate(workspace.as_deref(), scene.as_deref())
        .is_err()
        || menu
            .items
            .iter()
            .any(|(binding, _)| !bindings.contains(*binding))
    {
        state.clear(&mut commands);
        return;
    }
    let dismiss_armed = menu.dismiss_armed;
    let Ok(ctx) = egui_contexts.ctx_mut() else {
        return;
    };
    // The complete opening gesture belongs to the caller, including a release
    // delivered several frames after this popup first appeared.
    if !dismiss_armed
        && ctx.input(|input| !input.pointer.any_down() && !input.pointer.any_released())
    {
        if let Some(pending) = state.pending.as_mut() {
            pending.dismiss_armed = true;
        }
    }

    let mut selected: Option<(Entity, ScriptMenuItem)> = None;
    let response = egui::Area::new(egui::Id::new(("lunco_scene_context_menu", menu.root)))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(menu.screen_position[0], menu.screen_position[1]))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                for (binding, item) in &menu.items {
                    if ui.button(&item.label).clicked() {
                        selected = Some((*binding, item.clone()));
                    }
                }
                if menu.items.is_empty() {
                    ui.label("No actions available");
                }
            });
        });
    // A foreground egui Area can sit inside the 3D scene leaf. Register its
    // exact painted bounds as chrome so ordered scene/gizmo hits cannot also
    // claim a menu click.
    let menu_rect = response.response.rect;
    scene_pick.record_chrome_panel(menu_rect, menu_rect);

    if let Some((binding, item)) = selected {
        // Transfer only the selected lease to the queue before retiring the popup.
        commands.entity(binding).remove::<ChildOf>();
        state.clear(&mut commands);
        commands.trigger(lunco_scripting_rhai_runtime::commands::RunRhaiToolHook {
            binding: Some(binding),
            tool: item.tool,
            hook: item.hook,
            args: item.args,
            owner_twin_id: menu.scope.owner_twin_id,
        });
        return;
    }

    let clicked_elsewhere = dismiss_armed && response.response.clicked_elsewhere();
    if clicked_elsewhere {
        state.clear(&mut commands);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_core::AppSceneExt;
    use lunco_interaction_core::{
        ArmedScriptTool, DragModeActive, SpawnToolActive, TerrainToolActive,
    };
    use lunco_luncosim_edit_core::{
        SpawnState,
        terrain_tools::{TerrainTool, TerrainToolState},
    };

    #[test]
    fn scene_lifecycle_disarms_editor_intent_and_drops_menu_actions() {
        let mut app = App::new();
        app.init_scene_resource::<SceneContextMenuState>()
            .init_scene_resource::<ArmedScriptTool>()
            .init_scene_resource::<SpawnState>()
            .init_scene_resource::<TerrainToolState>()
            .init_scene_resource::<DragModeActive>()
            .init_scene_resource::<SpawnToolActive>()
            .init_scene_resource::<TerrainToolActive>()
            .add_observer(on_script_ui_request);
        for _ in 0..3 {
            app.world_mut().trigger(ScriptUiRequest::ContextMenu {
                screen_position: [10.0, 10.0],
                items: vec![ScriptMenuItem {
                    label: "outgoing action".into(),
                    tool: "reused_tool_name".into(),
                    hook: "on_click".into(),
                    args: Default::default(),
                }],
            });
            app.world_mut().resource_mut::<ArmedScriptTool>().0 = Some("reused_tool_name".into());
            *app.world_mut().resource_mut::<SpawnState>() = SpawnState::Selecting {
                entry_id: "reused_asset_name".into(),
            };
            *app.world_mut().resource_mut::<TerrainToolState>() = TerrainToolState {
                tool: TerrainTool::Sculpt,
                radius: 19.0,
                strength: 3.0,
            };
            app.world_mut().resource_mut::<DragModeActive>().active = true;
            app.world_mut().resource_mut::<SpawnToolActive>().0 = true;
            app.world_mut().resource_mut::<TerrainToolActive>().0 = true;
            lunco_core::run_scene_teardown(app.world_mut());
            assert!(
                app.world()
                    .resource::<SceneContextMenuState>()
                    .pending
                    .is_none()
            );
            assert!(!app.world().resource::<ArmedScriptTool>().armed());
            assert!(matches!(
                *app.world().resource::<SpawnState>(),
                SpawnState::Idle
            ));
            let brush = app.world().resource::<TerrainToolState>();
            assert!(!brush.armed());
            assert_eq!((brush.radius, brush.strength), (19.0, 3.0));
            assert!(!app.world().resource::<DragModeActive>().active);
            assert!(!app.world().resource::<SpawnToolActive>().0);
            assert!(!app.world().resource::<TerrainToolActive>().0);
        }
    }
}
