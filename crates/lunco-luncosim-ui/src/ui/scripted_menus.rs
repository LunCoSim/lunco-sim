//! Generic egui host for application menus contributed by Rhai.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{Command, on_command, register_commands};
use lunco_hooks::HookValue;
use lunco_scripting_bridge_core::ToolHookBinding;
use lunco_scripting_rhai_core::ui_bridge::{
    ScriptUiRequest, ScriptViewportAction, ScriptViewportVisibility, ScriptWorkbenchMenuAction,
    ScriptWorkbenchMenuItem, ScriptWorkbenchNavigationEntry, ScriptWorkbenchSelection,
};
use lunco_workbench_core::{
    MenuCallback, MenuCtx, WorkbenchMenuRegistry,
    presentation::{ViewportPlaceholder, ViewportPlaceholderAction},
};
use lunco_workbench_runtime_ui::RuntimeUiActionScope;

#[derive(Resource, Default)]
pub(crate) struct ScriptedMenuBindings(BTreeMap<String, InstalledProvider>);

struct InstalledProvider {
    root: Entity,
    twin_id: Option<u64>,
    menus: Vec<(i64, bool, String, Arc<Vec<BoundMenuItem>>)>,
    settings_submenus: Vec<(i64, bool, String, Arc<Vec<BoundMenuItem>>)>,
    viewport_actions: Vec<ScriptViewportAction>,
    viewport_guidance: Option<String>,
    navigation_entries: Vec<ScriptWorkbenchNavigationEntry>,
}

struct BoundMenuItem {
    label: String,
    tooltip: Option<String>,
    enabled: bool,
    separator: bool,
    selection: Option<ScriptWorkbenchSelection>,
    action: Option<ScriptWorkbenchMenuAction>,
    children: Vec<BoundMenuItem>,
}

fn action_endpoint_available(
    action: &ScriptWorkbenchMenuAction,
    provider_twin_id: Option<u64>,
) -> bool {
    ToolHookBinding::for_owner(
        action.tool.clone(),
        action.hook.clone(),
        provider_twin_id,
        lunco_scripting_bridge_core::ToolHookBindingRetention::Surface,
    )
    .is_ok()
}

type ActionAvailabilityCache = HashMap<(Option<u64>, String, String), bool>;

fn cached_action_endpoint_available(
    action: &ScriptWorkbenchMenuAction,
    provider_twin_id: Option<u64>,
    cache: &mut ActionAvailabilityCache,
) -> bool {
    let key = (provider_twin_id, action.tool.clone(), action.hook.clone());
    *cache
        .entry(key)
        .or_insert_with(|| action_endpoint_available(action, provider_twin_id))
}

/// Inspect installed script menu and Settings trees, live action availability,
/// and typed arguments. Application and Twin contributions retain their exact owner.
#[Command(default)]
pub(crate) struct InspectScriptedMenus {}

#[on_command(InspectScriptedMenus)]
fn on_inspect_scripted_menus(
    _trigger: On<InspectScriptedMenus>,
    providers: Res<ScriptedMenuBindings>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
) -> Result<Ack, String> {
    let active_twin_id = workspace
        .as_deref()
        .and_then(|workspace| workspace.active_twin)
        .map(lunco_workspace::TwinId::raw);
    let providers = providers
        .0
        .iter()
        .map(|(name, provider)| {
            let owner_active = provider.twin_id.is_none_or(|id| Some(id) == active_twin_id);
            let menus = provider
                .menus
                .iter()
                .map(|(order, separator_before, label, items)| {
                    HookValue::map([
                        ("label", HookValue::str(label)),
                        ("order", HookValue::Int(*order)),
                        ("separator_before", HookValue::Bool(*separator_before)),
                        (
                            "items",
                            HookValue::Array(inspect_items(items, provider.twin_id, owner_active)),
                        ),
                    ])
                })
                .collect::<Vec<_>>();
            let viewport_actions = provider
                .viewport_actions
                .iter()
                .map(|action| inspect_viewport_action(action, provider.twin_id, owner_active))
                .collect::<Vec<_>>();
            let settings_submenus = provider
                .settings_submenus
                .iter()
                .map(|(order, separator_before, label, items)| {
                    HookValue::map([
                        ("label", HookValue::str(label)),
                        ("order", HookValue::Int(*order)),
                        ("separator_before", HookValue::Bool(*separator_before)),
                        (
                            "items",
                            HookValue::Array(inspect_items(items, provider.twin_id, owner_active)),
                        ),
                    ])
                })
                .collect::<Vec<_>>();
            HookValue::map([
                ("provider", HookValue::str(name)),
                (
                    "twin_id",
                    provider.twin_id.map_or(HookValue::Unit, HookValue::UInt),
                ),
                ("owner_active", HookValue::Bool(owner_active)),
                ("menus", HookValue::Array(menus)),
                ("settings_submenus", HookValue::Array(settings_submenus)),
                ("viewport_actions", HookValue::Array(viewport_actions)),
            ])
        })
        .collect::<Vec<_>>();
    Ok(Ack::with_data(
        OpId::new(),
        HookValue::map([("providers", HookValue::Array(providers))]),
    ))
}

fn inspect_viewport_action(
    action: &ScriptViewportAction,
    provider_twin_id: Option<u64>,
    owner_active: bool,
) -> HookValue {
    let available = action_endpoint_available(&action.action, provider_twin_id);
    HookValue::map([
        ("label", HookValue::str(&action.label)),
        ("description", HookValue::str(&action.description)),
        (
            "enabled",
            HookValue::Bool(action.enabled && owner_active && available),
        ),
        ("action_available", HookValue::Bool(available)),
        ("primary", HookValue::Bool(action.primary)),
        ("tool", HookValue::str(&action.action.tool)),
        ("hook", HookValue::str(&action.action.hook)),
        (
            "args",
            lunco_scripting_bridge_core::telemetry_value(
                &lunco_scripting_bridge_core::ApiValueBuilder,
                &action.action.args,
            ),
        ),
    ])
}

pub(crate) fn register_navigation(world: &mut World) {
    world.init_resource::<lunco_workbench_core::navigation::NavigationRegistry>();
    let mut registry = world.resource_mut::<lunco_workbench_core::navigation::NavigationRegistry>();
    registry.register(&["Scenes"], scripted_navigation_entries);
}

fn scripted_navigation_entries(
    world: &World,
) -> Result<Vec<lunco_workbench_core::navigation::NavigationEntry>, String> {
    let providers = world
        .get_resource::<ScriptedMenuBindings>()
        .ok_or("scripted menu bindings are unavailable")?;
    let active_twin_id = world
        .get_resource::<lunco_workspace::WorkspaceResource>()
        .and_then(|workspace| workspace.active_twin)
        .map(lunco_workspace::TwinId::raw);
    let mut entries = Vec::new();
    for provider in providers.0.values() {
        if provider
            .twin_id
            .is_some_and(|owner| Some(owner) != active_twin_id)
        {
            continue;
        }
        let root = provider.root;
        let provider_twin_id = provider.twin_id;
        for entry in &provider.navigation_entries {
            if !action_endpoint_available(&entry.action, provider_twin_id) {
                continue;
            }
            let action = entry.action.clone();
            let id = entry.id.clone();
            entries.push(lunco_workbench_core::navigation::NavigationEntry {
                id,
                label: entry.label.clone(),
                detail: entry.detail.clone(),
                category: entry.category.clone(),
                activate: Arc::new(move |ctx| {
                    let workspace = ctx.resource::<lunco_workspace::WorkspaceResource>();
                    let scope = RuntimeUiActionScope::capture_owner(workspace);
                    if provider_twin_id.is_some_and(|owner| scope.owner_twin_id != Some(owner)) {
                        warn!("[navigation] quick-open action belongs to an inactive Twin");
                        return;
                    }
                    ctx.trigger(ScriptedMenuInvoked {
                        provider_root: root,
                        provider_twin_id,
                        action: action.clone(),
                        scope,
                    });
                }),
            });
        }
    }
    Ok(entries)
}

/// Convert currently applicable policy actions into empty-viewport callbacks.
/// Twin contributions are visible only while that exact Twin is active.
pub(crate) fn viewport_actions_for(
    providers: &ScriptedMenuBindings,
    active_twin_id: Option<u64>,
) -> Vec<ViewportPlaceholderAction> {
    providers
        .0
        .values()
        .filter(|provider| provider.twin_id.is_none() || provider.twin_id == active_twin_id)
        .flat_map(|provider| {
            let root = provider.root;
            let twin_id = provider.twin_id;
            provider
                .viewport_actions
                .iter()
                .filter(move |action| match action.visibility {
                    ScriptViewportVisibility::Always => true,
                    ScriptViewportVisibility::NoActiveTwin => active_twin_id.is_none(),
                })
                .map(move |action| {
                    let action = action.clone();
                    let action_for_availability = action.action.clone();
                    let is_available = Arc::new(move || {
                        action_endpoint_available(&action_for_availability, twin_id)
                    });
                    ViewportPlaceholderAction {
                        label: action.label.clone(),
                        description: action.description.clone(),
                        enabled: action.enabled,
                        is_available,
                        primary: action.primary,
                        activate: Arc::new(move |ctx| {
                            let workspace = ctx.resource::<lunco_workspace::WorkspaceResource>();
                            let scope = RuntimeUiActionScope::capture_owner(workspace);
                            ctx.trigger(ScriptedMenuInvoked {
                                provider_root: root,
                                provider_twin_id: twin_id,
                                action: action.action.clone(),
                                scope,
                            });
                        }),
                    }
                })
        })
        .collect()
}

pub(crate) fn viewport_guidance_for(
    providers: &ScriptedMenuBindings,
    active_twin_id: Option<u64>,
) -> Option<String> {
    providers
        .0
        .values()
        .filter(|provider| provider.twin_id.is_none() || provider.twin_id == active_twin_id)
        .find_map(|provider| provider.viewport_guidance.clone())
}

fn inspect_items(
    items: &[BoundMenuItem],
    provider_twin_id: Option<u64>,
    owner_active: bool,
) -> Vec<HookValue> {
    items
        .iter()
        .map(|item| {
            let available = item
                .action
                .as_ref()
                .is_none_or(|action| action_endpoint_available(action, provider_twin_id));
            let action = match &item.action {
                None => HookValue::Unit,
                Some(action) => HookValue::map([
                    ("tool", HookValue::str(&action.tool)),
                    ("hook", HookValue::str(&action.hook)),
                    (
                        "args",
                        lunco_scripting_bridge_core::telemetry_value(
                            &lunco_scripting_bridge_core::ApiValueBuilder,
                            &action.args,
                        ),
                    ),
                ]),
            };
            HookValue::map([
                ("label", HookValue::str(&item.label)),
                ("separator", HookValue::Bool(item.separator)),
                (
                    "enabled",
                    HookValue::Bool(item.enabled && owner_active && available),
                ),
                (
                    "action_available",
                    item.action
                        .as_ref()
                        .map_or(HookValue::Unit, |_| HookValue::Bool(available)),
                ),
                (
                    "selection",
                    match &item.selection {
                        None => HookValue::Unit,
                        Some(ScriptWorkbenchSelection::Radio(selected)) => HookValue::map([
                            ("kind", HookValue::str("radio")),
                            ("selected", HookValue::Bool(*selected)),
                        ]),
                        Some(ScriptWorkbenchSelection::Checkbox(selected)) => HookValue::map([
                            ("kind", HookValue::str("checkbox")),
                            ("selected", HookValue::Bool(*selected)),
                        ]),
                    },
                ),
                ("action", action),
                (
                    "children",
                    HookValue::Array(inspect_items(
                        &item.children,
                        provider_twin_id,
                        owner_active,
                    )),
                ),
            ])
        })
        .collect()
}

register_commands!(on_inspect_scripted_menus);

#[derive(Event)]
pub(crate) struct ScriptedMenuInvoked {
    provider_root: Entity,
    provider_twin_id: Option<u64>,
    action: ScriptWorkbenchMenuAction,
    scope: RuntimeUiActionScope,
}

pub(crate) fn invoke_scripted_menu(
    trigger: On<ScriptedMenuInvoked>,
    providers: Res<ScriptedMenuBindings>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    scene: Option<Res<lunco_core::SceneTransitionCoordinator>>,
    mut commands: Commands,
) {
    let invocation = trigger.event();
    let result = (|| {
        if !providers
            .0
            .values()
            .any(|provider| provider.root == invocation.provider_root)
        {
            return Err("workbench menu callback has been retired".to_owned());
        }
        invocation
            .scope
            .validate(workspace.as_deref(), scene.as_deref())?;
        let binding = ToolHookBinding::for_owner(
            invocation.action.tool.clone(),
            invocation.action.hook.clone(),
            invocation.provider_twin_id,
            lunco_scripting_bridge_core::ToolHookBindingRetention::Surface,
        )?;
        let selected = binding.select_for_owner(invocation.scope.owner_twin_id)?;
        let entity = commands
            .spawn((selected, ChildOf(invocation.provider_root)))
            .id();
        commands.trigger(lunco_scripting_rhai_runtime::commands::RunRhaiToolHook {
            binding: Some(entity),
            tool: invocation.action.tool.clone(),
            hook: invocation.action.hook.clone(),
            args: invocation.action.args.clone(),
            owner_twin_id: invocation.scope.owner_twin_id,
        });
        Ok::<_, String>(())
    })();
    if let Err(error) = result {
        warn!("[workbench] {error}");
    }
}

fn materialize_items(items: &[ScriptWorkbenchMenuItem]) -> Vec<BoundMenuItem> {
    items
        .iter()
        .map(|item| BoundMenuItem {
            label: item.label.clone(),
            tooltip: item.tooltip.clone(),
            enabled: item.enabled,
            separator: item.separator,
            selection: item.selection.clone(),
            action: item.action.clone(),
            children: materialize_items(&item.children),
        })
        .collect()
}

const SCRIPT_MENU_MAX_WIDTH: f32 = 280.0;
const SCRIPT_MENU_MAX_HEIGHT: f32 = 360.0;
const SHELL_MENU_LABELS: &[&str] = &["File", "Edit", "View", "Settings", "Help", "Time", "More"];

pub(crate) fn on_script_ui_request(
    trigger: On<ScriptUiRequest>,
    mut menus: ResMut<WorkbenchMenuRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut providers: ResMut<ScriptedMenuBindings>,
    mut placeholder: ResMut<ViewportPlaceholder>,
    mut commands: Commands,
) {
    let ScriptUiRequest::WorkbenchMenus {
        provider,
        twin_id,
        menus: contributions,
        settings_submenus: settings_contributions,
        viewport_actions,
        viewport_guidance,
        navigation_entries,
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
    let provider_twin_id = *twin_id;
    let installed = contributions
        .iter()
        .map(|menu| {
            let items = Arc::new(materialize_items(&menu.items));
            let callback = scripted_menu_callback(root, provider_twin_id, items.clone());
            (
                menu.order,
                menu.separator_before,
                menu.label.clone(),
                items,
                callback,
            )
        })
        .collect::<Vec<_>>();
    let installed_settings = settings_contributions
        .iter()
        .map(|submenu| {
            let items = Arc::new(materialize_items(&submenu.items));
            let callback = scripted_menu_callback(root, provider_twin_id, items.clone());
            (
                submenu.order,
                submenu.separator_before,
                submenu.label.clone(),
                items,
                callback,
            )
        })
        .collect::<Vec<_>>();
    let mut trees = Vec::with_capacity(installed.len());
    let mut callbacks = Vec::with_capacity(installed.len());
    for (order, separator_before, label, items, callback) in installed {
        trees.push((order, separator_before, label.clone(), items));
        callbacks.push((order, separator_before, label, callback));
    }
    let mut settings_trees = Vec::with_capacity(installed_settings.len());
    let mut settings_callbacks = Vec::with_capacity(installed_settings.len());
    for (order, separator_before, label, items, callback) in installed_settings {
        settings_trees.push((order, separator_before, label.clone(), items));
        settings_callbacks.push((order, separator_before, label, callback));
    }
    // Track reserved roots synchronously as well as committed ECS entities:
    // several replacements in one flush must retire every outgoing tree.
    if let Some(previous) = providers.0.remove(provider) {
        commands.entity(previous.root).try_despawn();
    }
    if contributions.is_empty()
        && settings_contributions.is_empty()
        && viewport_actions.is_empty()
        && viewport_guidance.is_none()
        && navigation_entries.is_empty()
    {
        commands.entity(root).try_despawn();
    } else {
        providers.0.insert(
            provider.clone(),
            InstalledProvider {
                root,
                twin_id: *twin_id,
                menus: trees,
                settings_submenus: settings_trees,
                viewport_actions: viewport_actions.clone(),
                viewport_guidance: viewport_guidance.clone(),
                navigation_entries: navigation_entries.clone(),
            },
        );
    }
    menus.replace_scripted_menus(provider.clone(), *twin_id, callbacks);
    menus.replace_scripted_settings_submenus(provider.clone(), *twin_id, settings_callbacks);
    if let Some(workspace) = workspace.as_deref() {
        super::refresh_viewport_placeholder(workspace, &providers, &mut placeholder);
    }
}

fn scripted_menu_callback(
    root: Entity,
    provider_twin_id: Option<u64>,
    rendered: Arc<Vec<BoundMenuItem>>,
) -> MenuCallback {
    Arc::new(move |ui, ctx| {
        // An immutable installation has its own pointer identity. A release
        // from a replaced contribution cannot activate its successor.
        let workspace = ctx.resource::<lunco_workspace::WorkspaceResource>();
        let scene = ctx.resource::<lunco_core::SceneTransitionCoordinator>();
        let scope = RuntimeUiActionScope::capture_owner(workspace);
        let enabled = scope.validate(workspace, scene).is_ok();
        ui.push_id((root, scope.owner_twin_id, scope.lifetime), |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                let mut availability = ActionAvailabilityCache::new();
                draw_items(
                    ui,
                    ctx,
                    &rendered,
                    scope,
                    root,
                    provider_twin_id,
                    &mut availability,
                )
            });
        });
    })
}

pub(crate) fn clear_scripted_menus_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    mut menus: ResMut<WorkbenchMenuRegistry>,
    mut providers: ResMut<ScriptedMenuBindings>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut placeholder: ResMut<ViewportPlaceholder>,
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
    if let Some(workspace) = workspace.as_deref() {
        super::refresh_viewport_placeholder(workspace, &providers, &mut placeholder);
    }
}

fn draw_items(
    ui: &mut egui::Ui,
    ctx: &mut MenuCtx,
    items: &[BoundMenuItem],
    scope: RuntimeUiActionScope,
    provider_root: Entity,
    provider_twin_id: Option<u64>,
    availability: &mut ActionAvailabilityCache,
) {
    let max_width = lunco_workbench::menu_popup_max_width(
        ui.ctx().content_rect().width(),
        SCRIPT_MENU_MAX_WIDTH,
    );
    ui.set_max_width(max_width);
    egui::ScrollArea::vertical()
        .max_width(max_width)
        .max_height(SCRIPT_MENU_MAX_HEIGHT)
        .auto_shrink([true, true])
        .show(ui, |ui| {
            draw_items_inner(
                ui,
                ctx,
                items,
                scope,
                provider_root,
                provider_twin_id,
                availability,
            )
        });
}

fn draw_items_inner(
    ui: &mut egui::Ui,
    ctx: &mut MenuCtx,
    items: &[BoundMenuItem],
    scope: RuntimeUiActionScope,
    provider_root: Entity,
    provider_twin_id: Option<u64>,
    availability: &mut ActionAvailabilityCache,
) {
    if items.is_empty() {
        ui.label(egui::RichText::new("No entries available").weak().italics());
        return;
    }
    for item in items {
        if item.separator {
            ui.separator();
            continue;
        }
        let action_available = item.action.as_ref().is_none_or(|action| {
            cached_action_endpoint_available(action, provider_twin_id, availability)
        });
        let enabled = item.enabled && action_available;
        if item.children.is_empty() {
            let response = match &item.selection {
                Some(ScriptWorkbenchSelection::Radio(selected)) => {
                    ui.add_enabled(enabled, egui::RadioButton::new(*selected, &item.label))
                }
                Some(ScriptWorkbenchSelection::Checkbox(selected)) => {
                    let mut checked = *selected;
                    ui.add_enabled(enabled, egui::Checkbox::new(&mut checked, &item.label))
                }
                None => ui.add_enabled(enabled, egui::Button::new(&item.label).wrap()),
            };
            let response = if !action_available {
                response.on_hover_text("The Rhai action is currently unavailable.")
            } else if let Some(tooltip) = item.tooltip.as_deref() {
                response.on_hover_text(tooltip)
            } else {
                response
            };
            if response.clicked() {
                if let Some(action) = &item.action {
                    ctx.trigger(ScriptedMenuInvoked {
                        provider_root,
                        provider_twin_id,
                        action: action.clone(),
                        scope,
                    });
                }
                ui.close();
            }
        } else {
            let response = ui.menu_button(&item.label, |ui| {
                draw_items(
                    ui,
                    ctx,
                    &item.children,
                    scope,
                    provider_root,
                    provider_twin_id,
                    availability,
                );
            });
            if let Some(tooltip) = item.tooltip.as_deref() {
                response.response.on_hover_text(tooltip);
            }
        }
    }
}
