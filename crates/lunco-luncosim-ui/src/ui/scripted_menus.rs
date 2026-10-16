//! Generic egui host for application menus contributed by Rhai.

use std::{collections::BTreeMap, sync::Arc};

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_scripting_bridge_core::ToolHookBinding;
use lunco_scripting_rhai_core::ui_bridge::{ScriptUiRequest, ScriptWorkbenchMenuItem};
use lunco_telemetry_core::TelemetryValue;
use lunco_workbench_core::{MenuCallback, MenuCtx, WorkbenchMenuRegistry};
use lunco_workbench_runtime_ui::RuntimeUiActionScope;

#[derive(Resource, Default)]
pub(crate) struct ScriptedMenuBindings(BTreeMap<String, InstalledProvider>);

struct InstalledProvider {
    root: Entity,
    twin_id: Option<u64>,
}

struct BoundMenuItem {
    label: String,
    tooltip: Option<String>,
    enabled: bool,
    action: Option<(Entity, TelemetryValue)>,
    children: Vec<BoundMenuItem>,
}

#[derive(Event)]
pub(crate) struct ScriptedMenuInvoked {
    binding: Entity,
    args: TelemetryValue,
    scope: RuntimeUiActionScope,
}

pub(crate) fn invoke_scripted_menu(
    trigger: On<ScriptedMenuInvoked>,
    bindings: Query<&ToolHookBinding>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    scene: Option<Res<lunco_core::SceneTransitionCoordinator>>,
    mut commands: Commands,
) {
    let invocation = trigger.event();
    let result = (|| {
        invocation
            .scope
            .validate(workspace.as_deref(), scene.as_deref())?;
        let binding = bindings
            .get(invocation.binding)
            .map_err(|_| "workbench menu callback has been retired".to_owned())?;
        let selected = binding.select_for_scene(
            invocation.scope.owner_twin_id,
            invocation.scope.scene_generation,
        )?;
        let entity = commands.spawn((selected, ChildOf(invocation.binding))).id();
        commands.trigger(lunco_scripting_rhai_runtime::commands::RunRhaiToolHook {
            binding: Some(entity),
            tool: binding.tool.clone(),
            hook: binding.hook.clone(),
            args: invocation.args.clone(),
            owner_twin_id: invocation.scope.owner_twin_id,
        });
        Ok::<_, String>(())
    })();
    if let Err(error) = result {
        warn!("[workbench] {error}");
    }
}

fn bind_items(
    items: &[ScriptWorkbenchMenuItem],
    twin_id: Option<u64>,
    root: Entity,
    commands: &mut Commands,
) -> Result<Vec<BoundMenuItem>, String> {
    items
        .iter()
        .map(|item| {
            let action = item
                .action
                .as_ref()
                .map(|action| {
                    let binding = ToolHookBinding::for_provider(
                        action.tool.clone(),
                        action.hook.clone(),
                        twin_id,
                    )?;
                    Ok::<_, String>((
                        commands.spawn((binding, ChildOf(root))).id(),
                        action.args.clone(),
                    ))
                })
                .transpose()?;
            Ok(BoundMenuItem {
                label: item.label.clone(),
                tooltip: item.tooltip.clone(),
                enabled: item.enabled,
                action,
                children: bind_items(&item.children, twin_id, root, commands)?,
            })
        })
        .collect()
}

const SCRIPT_MENU_MAX_WIDTH: f32 = 420.0;
const SCRIPT_MENU_MAX_HEIGHT: f32 = 360.0;
const SHELL_MENU_LABELS: &[&str] = &["File", "Edit", "View", "Settings", "Help", "Time", "More"];

pub(crate) fn on_script_ui_request(
    trigger: On<ScriptUiRequest>,
    mut menus: ResMut<WorkbenchMenuRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut providers: ResMut<ScriptedMenuBindings>,
    mut commands: Commands,
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
        return;
    }
    let root = commands.spawn_empty().id();
    let callbacks = contributions
        .iter()
        .map(|menu| {
            let items = bind_items(&menu.items, *twin_id, root, &mut commands)?;
            let callback: MenuCallback = Arc::new(move |ui, ctx| {
                // An immutable installation has its own pointer identity. A release
                // from a replaced contribution cannot activate its successor.
                let workspace = ctx.resource::<lunco_workspace::WorkspaceResource>();
                let scene = ctx.resource::<lunco_core::SceneTransitionCoordinator>();
                let scope = RuntimeUiActionScope::capture(workspace, scene);
                let enabled = scope.validate(workspace, scene).is_ok();
                ui.push_id((root, scope.owner_twin_id, scope.scene_generation), |ui| {
                    ui.add_enabled_ui(enabled, |ui| draw_items(ui, ctx, &items, scope));
                });
            });
            Ok::<_, String>((menu.label.clone(), callback))
        })
        .collect::<Result<Vec<_>, _>>();
    let callbacks = match callbacks {
        Ok(callbacks) => callbacks,
        Err(error) => {
            commands.entity(root).try_despawn();
            warn!("[workbench] menu provider `{provider}` rejected: {error}");
            return;
        }
    };
    // Track reserved roots synchronously as well as committed ECS entities:
    // several replacements in one flush must retire every outgoing tree.
    if let Some(previous) = providers.0.remove(provider) {
        commands.entity(previous.root).try_despawn();
    }
    if contributions.is_empty() {
        commands.entity(root).try_despawn();
    } else {
        providers.0.insert(
            provider.clone(),
            InstalledProvider {
                root,
                twin_id: *twin_id,
            },
        );
    }
    menus.replace_scripted_menus(provider.clone(), *twin_id, callbacks);
}

pub(crate) fn clear_scripted_menus_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    mut menus: ResMut<WorkbenchMenuRegistry>,
    mut providers: ResMut<ScriptedMenuBindings>,
    mut commands: Commands,
) {
    let twin = trigger.event().twin.raw();
    providers.0.retain(|_, provider| {
        if provider.twin_id == Some(twin) {
            commands.entity(provider.root).try_despawn();
            false
        } else {
            true
        }
    });
    menus.clear_scripted_twin(twin);
}

fn draw_items(
    ui: &mut egui::Ui,
    ctx: &mut MenuCtx,
    items: &[BoundMenuItem],
    scope: RuntimeUiActionScope,
) {
    ui.set_width(lunco_workbench::menu_popup_max_width(
        ui.ctx().content_rect().width(),
        SCRIPT_MENU_MAX_WIDTH,
    ));
    egui::ScrollArea::vertical()
        .max_height(SCRIPT_MENU_MAX_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| draw_items_inner(ui, ctx, items, scope));
}

fn draw_items_inner(
    ui: &mut egui::Ui,
    ctx: &mut MenuCtx,
    items: &[BoundMenuItem],
    scope: RuntimeUiActionScope,
) {
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
                if let Some((binding, args)) = &item.action {
                    ctx.trigger(ScriptedMenuInvoked {
                        binding: *binding,
                        args: args.clone(),
                        scope,
                    });
                }
                ui.close();
            }
        } else {
            let response = ui.menu_button(&item.label, |ui| {
                draw_items(ui, ctx, &item.children, scope);
            });
            if let Some(tooltip) = item.tooltip.as_deref() {
                response.response.on_hover_text(tooltip);
            }
        }
    }
}
