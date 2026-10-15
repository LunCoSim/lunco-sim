//! Generic scene context-menu host.
//!
//! Scripts author the labels and typed tool-hook actions. This module only
//! anchors a popup at the pointer location and dispatches the selected generic
//! hook, so the editor does not acquire waypoint, vehicle, or program policy.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use lunco_scripting_rhai_core::ui_bridge::{ScriptMenuItem, ScriptUiRequest};
use lunco_workbench_core::scene_pick::ScenePickGate;

#[derive(Clone, Debug)]
struct PendingMenu {
    screen_position: [f32; 2],
    items: Vec<ScriptMenuItem>,
    dismiss_on_next_input: bool,
}

#[derive(Resource, Default)]
pub struct SceneContextMenuState {
    pending: Option<PendingMenu>,
}

impl lunco_core::SceneResource for SceneContextMenuState {}

pub fn on_script_ui_request(
    trigger: On<ScriptUiRequest>,
    mut state: ResMut<SceneContextMenuState>,
) {
    match trigger.event() {
        ScriptUiRequest::ContextMenu {
            screen_position,
            items,
        } => {
            state.pending = Some(PendingMenu {
                screen_position: *screen_position,
                items: items.clone(),
                // The request is raised from the same pointer transaction
                // that opened the menu. Do not treat that transaction as an
                // outside click when the popup is first painted.
                dismiss_on_next_input: false,
            });
        }
        _ => {}
    }
}

pub fn draw_scene_context_menu(
    mut egui_contexts: EguiContexts,
    mut state: ResMut<SceneContextMenuState>,
    mut scene_pick: ResMut<ScenePickGate>,
    mut commands: Commands,
) {
    let Some(menu) = state.pending.clone() else {
        return;
    };
    let dismiss_on_input = menu.dismiss_on_next_input;
    if !dismiss_on_input {
        if let Some(pending) = state.pending.as_mut() {
            pending.dismiss_on_next_input = true;
        }
    }
    let Ok(ctx) = egui_contexts.ctx_mut() else {
        return;
    };

    let mut selected: Option<ScriptMenuItem> = None;
    let response = egui::Area::new(egui::Id::new("lunco_scene_context_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(menu.screen_position[0], menu.screen_position[1]))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                for item in &menu.items {
                    if ui.button(&item.label).clicked() {
                        selected = Some(item.clone());
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

    if let Some(item) = selected {
        commands.trigger(lunco_scripting_rhai_runtime::commands::RunRhaiToolHook {
            tool: item.tool,
            hook: item.hook,
            args: item.args,
            owner_twin_id: None,
        });
        state.pending = None;
        return;
    }

    let clicked_elsewhere = dismiss_on_input
        && ctx.input(|input| input.pointer.any_click())
        && !response.response.hovered();
    if clicked_elsewhere {
        state.pending = None;
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
