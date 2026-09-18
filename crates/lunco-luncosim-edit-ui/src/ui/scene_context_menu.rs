//! Generic scene context-menu host.
//!
//! Scripts author the labels and typed tool-hook actions. This module only
//! anchors a popup at the pointer location and dispatches the selected generic
//! hook, so the editor does not acquire waypoint, vehicle, or program policy.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use lunco_scripting_rhai_core::ui_bridge::{ScriptMenuItem, ScriptUiRequest};

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
    }
}

pub fn draw_scene_context_menu(
    mut egui_contexts: EguiContexts,
    mut state: ResMut<SceneContextMenuState>,
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

    if let Some(item) = selected {
        commands.trigger(lunco_scripting_rhai_runtime::commands::RunRhaiToolHook {
            tool: item.tool,
            hook: item.hook,
            args: item.args,
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
