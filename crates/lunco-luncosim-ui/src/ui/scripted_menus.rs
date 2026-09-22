//! Generic egui host for application menus contributed by Rhai.

use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_scripting_rhai_core::ui_bridge::{ScriptUiRequest, ScriptWorkbenchMenuItem};
use lunco_workbench_core::{MenuCallback, MenuCtx, WorkbenchMenuRegistry};

const SCRIPT_MENU_MAX_WIDTH: f32 = 420.0;
const SCRIPT_MENU_MAX_HEIGHT: f32 = 360.0;
const SHELL_MENU_LABELS: &[&str] = &["File", "Edit", "View", "Settings", "Help", "Time", "More"];

pub(crate) fn on_script_ui_request(
    trigger: On<ScriptUiRequest>,
    mut menus: ResMut<WorkbenchMenuRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
) {
    let ScriptUiRequest::WorkbenchMenus {
        provider,
        twin_id,
        menus: contributions,
    } = trigger.event()
    else {
        return;
    };
    if let Some(twin_id) = twin_id {
        let Some(workspace) = workspace.as_deref() else {
            return;
        };
        if workspace
            .twin(lunco_workspace::TwinId::new(*twin_id))
            .is_none()
        {
            return;
        }
    }
    if let Some(menu) = contributions
        .iter()
        .find(|menu| SHELL_MENU_LABELS.contains(&menu.label.as_str()))
    {
        warn!(
            "[workbench] Rhai menu provider `{provider}` cannot replace shell menu `{}`",
            menu.label
        );
        menus.replace_scripted_menus(provider.clone(), *twin_id, Vec::new());
        return;
    }
    let callbacks = contributions
        .iter()
        .map(|menu| {
            let label = menu.label.clone();
            let items = menu.items.clone();
            let callback: MenuCallback = Arc::new(move |ui, ctx| {
                draw_items(ui, ctx, &items);
            });
            (label, callback)
        })
        .collect();
    menus.replace_scripted_menus(provider.clone(), *twin_id, callbacks);
}

pub(crate) fn clear_scripted_menus_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    mut menus: ResMut<WorkbenchMenuRegistry>,
) {
    menus.clear_scripted_twin(trigger.event().twin.raw());
}

fn draw_items(ui: &mut egui::Ui, ctx: &mut MenuCtx, items: &[ScriptWorkbenchMenuItem]) {
    ui.set_width(lunco_workbench::menu_popup_max_width(
        ui.ctx().content_rect().width(),
        SCRIPT_MENU_MAX_WIDTH,
    ));
    egui::ScrollArea::vertical()
        .max_height(SCRIPT_MENU_MAX_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| draw_items_inner(ui, ctx, items));
}

fn draw_items_inner(ui: &mut egui::Ui, ctx: &mut MenuCtx, items: &[ScriptWorkbenchMenuItem]) {
    if items.is_empty() {
        ui.label(egui::RichText::new("No entries available").weak().italics());
        return;
    }
    for item in items {
        if item.children.is_empty() {
            let response = ui.add_enabled(item.enabled, egui::Button::new(&item.label).wrap());
            let response = if let Some(tooltip) = item.tooltip.as_deref() {
                response.on_hover_text(tooltip)
            } else {
                response
            };
            if response.clicked() {
                if let Some(action) = &item.action {
                    ctx.trigger(ScriptUiRequest::WorkbenchMenuAction {
                        tool: action.tool.clone(),
                        hook: action.hook.clone(),
                        args: action.args.clone(),
                    });
                }
                ui.close();
            }
        } else {
            let response = ui.menu_button(&item.label, |ui| {
                draw_items(ui, ctx, &item.children);
            });
            if let Some(tooltip) = item.tooltip.as_deref() {
                response.response.on_hover_text(tooltip);
            }
        }
    }
}
