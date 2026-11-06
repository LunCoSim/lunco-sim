//! Project and Twin settings editor.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_twin::TwinSettingValue;
use lunco_workbench_widgets::{UiIcon, icon_button, text_editor};
use lunco_workspace::{
    ResetTwinSetting, SceneLoadingCli, SceneLoadingSettings, SetProjectSceneLoading,
    SetTwinSetting, TwinClosed, TwinId, TwinSettingInput, WorkspaceResource,
};

use crate::{MenuCtx, Panel, PanelCtx, PanelId, PanelMenuGroup, PanelSlot, WorkbenchMenuRegistry};

/// Stable panel id for the Twin-scoped settings editor.
pub(crate) const TWIN_SETTINGS_PANEL_ID: PanelId = PanelId("lunco.workbench.twin_settings");

#[derive(Clone, Debug, PartialEq)]
struct TwinSettingRow {
    key: String,
    value: TwinSettingValue,
}

/// Change-driven view of the active Twin's generic `[settings]` map.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub(crate) struct TwinSettingsView {
    active_twin: Option<TwinId>,
    rows: Vec<TwinSettingRow>,
}

/// Rebuild the small settings snapshot after workspace/Twin changes.
pub(crate) fn refresh_view(workspace: Res<WorkspaceResource>, mut view: ResMut<TwinSettingsView>) {
    let active_twin = workspace.active_twin.filter(|id| {
        workspace.scope_kind(*id) == Some(lunco_workspace::WorkspaceScopeKind::AuthoredTwin)
    });
    let rows = active_twin
        .and_then(|id| workspace.twin(id))
        .and_then(|twin| twin.manifest.as_ref())
        .map(|manifest| {
            manifest
                .settings
                .iter()
                .filter(|(key, _)| {
                    key.as_str() != lunco_workspace::scene_settings::SCENE_LOAD_DEFAULT_KEY
                })
                .map(|(key, value)| TwinSettingRow {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    let next = TwinSettingsView { active_twin, rows };
    if *view != next {
        *view = next;
    }
}

/// Clear settings immediately when their active Twin is closed.
pub(crate) fn clear_on_twin_closed(trigger: On<TwinClosed>, mut view: ResMut<TwinSettingsView>) {
    if view.active_twin == Some(trigger.event().twin) {
        *view = TwinSettingsView::default();
    }
}

/// Workbench panel for inspecting and editing all generic settings authored by
/// the active Twin.
#[derive(Default)]
pub(crate) struct TwinSettingsPanel {
    active_twin: Option<TwinId>,
    query: String,
}

impl Panel for TwinSettingsPanel {
    fn id(&self) -> PanelId {
        TWIN_SETTINGS_PANEL_ID
    }

    fn title(&self) -> String {
        "Twin settings".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspector
    }

    fn menu_group(&self) -> PanelMenuGroup {
        PanelMenuGroup::Lunica
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let view = ctx.resource_expect::<TwinSettingsView>().clone();
        if self.active_twin != view.active_twin {
            self.active_twin = view.active_twin;
            self.query.clear();
        }
        ctx.panel_content_frame().show(ui, |ui| {
            ui.heading("Twin settings");
            ui.label(
                egui::RichText::new("Twin scope · twin.toml [settings]")
                    .weak()
                    .small(),
            );

            let Some(active_twin) = view.active_twin else {
                ui.separator();
                ui.label("No Twin is open.");
                return;
            };
            ui.label(
                egui::RichText::new(format!("Active Twin {}", active_twin.raw()))
                    .weak()
                    .small(),
            );
            render_shared_scene_setting(ui, ctx, active_twin);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Find").strong());
                ui.add(
                    text_editor::singleline(&mut self.query)
                        .hint_text("key or namespace")
                        .desired_width(150.0),
                );
                if !self.query.trim().is_empty()
                    && icon_button(ui, UiIcon::Close, "Clear settings filter").clicked()
                {
                    self.query.clear();
                }
            });
            ui.separator();

            if view.rows.is_empty() {
                ui.label("This Twin has no generic settings.");
                return;
            }

            let query = self.query.clone();
            let mut group = None;
            let mut shown = false;
            for row in &view.rows {
                if !matches_query(&row.key, &query) {
                    continue;
                }
                let next_group = row
                    .key
                    .split_once('.')
                    .map_or("General", |(prefix, _)| prefix);
                if group != Some(next_group) {
                    if group.is_some() {
                        ui.add_space(6.0);
                    }
                    ui.label(egui::RichText::new(next_group).strong());
                    group = Some(next_group);
                }
                render_setting_row(ui, ctx, row);
                shown = true;
            }
            if !shown {
                ui.label("No settings match the current filter.");
            }
        });
    }
}

/// Register the app-wide opening preference under Settings → General.
pub(crate) fn register_scene_loading_settings_menu(world: &mut World) {
    if let Some(mut menus) = world.get_resource_mut::<WorkbenchMenuRegistry>() {
        menus.register_settings_submenu("General", render_global_scene_setting);
        menus.register_settings_submenu("Project", render_project_scene_setting);
    }
}

fn render_global_scene_setting(ui: &mut egui::Ui, ctx: &mut MenuCtx) {
    let Some(mut settings) = ctx.resource::<SceneLoadingSettings>().cloned() else {
        return;
    };
    if ui
        .checkbox(
            &mut settings.load_default,
            "Load the default scene when opening a Twin",
        )
        .changed()
    {
        ctx.set_resource(settings);
    }
}

fn render_project_scene_setting(ui: &mut egui::Ui, ctx: &mut MenuCtx) {
    let Some(workspace) = ctx.resource::<WorkspaceResource>() else {
        return;
    };
    let Some(id) = workspace.active_twin else {
        ui.label("Open a project to change its settings.");
        return;
    };
    let Some(twin) = workspace.twin(id) else {
        return;
    };
    if twin.manifest.is_none() {
        ui.label("No project is open.");
        return;
    }
    let name = twin
        .manifest
        .as_ref()
        .map(|manifest| manifest.name.as_str())
        .or_else(|| twin.root.file_name().and_then(|name| name.to_str()))
        .unwrap_or("Project")
        .to_owned();
    let shared =
        lunco_workspace::scene_settings::twin_scene_loading_setting(twin.manifest.as_ref());
    let Some(project) = workspace.project_scene_settings(id).cloned() else {
        ui.label("Project settings are unavailable.");
        return;
    };
    let pending = workspace.project_settings_pending(id);
    let error = workspace.project_settings_error(id).map(str::to_owned);
    let Some(global) = ctx.resource::<SceneLoadingSettings>().cloned() else {
        return;
    };
    let Some(cli) = ctx.resource::<SceneLoadingCli>().copied() else {
        return;
    };
    ui.label(egui::RichText::new(name).weak());
    let inherited = format!(
        "Use app setting ({})",
        scene_choice_label(global.load_default)
    );
    let selected = match project.load_default {
        None => inherited.as_str(),
        Some(value) => scene_choice_label(value),
    };
    let mut value = project.load_default;
    let changed = ui
        .add_enabled_ui(!pending, |ui| {
            crate::render::settings_choice_menu(
                ui,
                format!("Default scene on open: {selected}"),
                &mut value,
                [
                    (None, inherited.clone()),
                    (Some(true), "Load default scene".into()),
                    (Some(false), "Start empty".into()),
                ],
            )
        })
        .inner;
    if changed {
        ctx.trigger_command(
            SetProjectSceneLoading {
                twin: id.raw(),
                value,
            },
            local_user(),
        );
    }
    if pending {
        ui.label("Saving…");
    }
    if let Some(error) = error {
        ui.label(format!("Could not save project setting: {error}"));
    }
    match shared {
        Ok(Some(value)) => {
            ui.label(format!(
                "The shared Twin setting overrides this project: {}.",
                scene_choice_label(value)
            ));
            if ui.button("Edit Twin setting…").clicked() {
                ctx.trigger(lunco_workbench_core::commands::FocusPanel {
                    id: TWIN_SETTINGS_PANEL_ID.as_str().into(),
                });
                ui.close();
            }
        }
        Ok(None) => {}
        Err(error) => {
            ui.label(error);
        }
    }
    if let Some(value) = cli.0 {
        ui.label(format!("A command-line flag controls this run: {}. The saved project setting applies when higher-priority overrides are absent.", scene_choice_label(value)));
    }
}

fn scene_choice_label(value: bool) -> &'static str {
    if value {
        "Load default scene"
    } else {
        "Start empty"
    }
}

fn scene_choice(ui: &mut egui::Ui, id: &str, value: &mut Option<bool>, inherited: String) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(match value {
            None => inherited.as_str(),
            Some(value) => scene_choice_label(*value),
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, inherited);
            ui.selectable_value(value, Some(true), "Load default scene");
            ui.selectable_value(value, Some(false), "Start empty");
        });
}

fn local_user() -> lunco_core::CommandOrigin {
    lunco_core::CommandOrigin::LocalUser {
        session_id: lunco_command_contracts::SessionId::LOCAL,
    }
}

fn render_shared_scene_setting(ui: &mut egui::Ui, ctx: &mut PanelCtx, id: TwinId) {
    let Some(workspace) = ctx.resource::<WorkspaceResource>() else {
        return;
    };
    let Some(twin) = workspace.twin(id) else {
        return;
    };
    let Some(manifest) = twin.manifest.as_ref() else {
        ui.label("No Twin is open.");
        return;
    };
    let current = match lunco_workspace::scene_settings::twin_scene_loading_setting(Some(manifest))
    {
        Ok(value) => value,
        Err(error) => {
            ui.label(error);
            return;
        }
    };
    let project = workspace
        .project_scene_settings(id)
        .and_then(|settings| settings.load_default);
    let Some(global) = ctx.resource::<SceneLoadingSettings>() else {
        return;
    };
    let inherited = project.unwrap_or(global.load_default);
    ui.label(egui::RichText::new("Shared with this Twin").strong());
    ui.label("Default scene on open");
    let mut selected = current;
    scene_choice(
        ui,
        "twin_scene_loading",
        &mut selected,
        format!("Use project setting ({})", scene_choice_label(inherited)),
    );
    if selected != current {
        match selected {
            Some(value) => ctx.trigger_command(
                SetTwinSetting {
                    key: lunco_workspace::scene_settings::SCENE_LOAD_DEFAULT_KEY.into(),
                    value: TwinSettingInput::Bool(value),
                },
                local_user(),
            ),
            None => ctx.trigger_command(
                ResetTwinSetting {
                    key: lunco_workspace::scene_settings::SCENE_LOAD_DEFAULT_KEY.into(),
                },
                local_user(),
            ),
        }
    }
    ui.label("This setting is saved with the Twin and takes precedence over project settings.");
}

fn matches_query(key: &str, query: &str) -> bool {
    let query = query.trim();
    query.is_empty() || key.to_lowercase().contains(&query.to_lowercase())
}

fn render_setting_row(ui: &mut egui::Ui, ctx: &mut PanelCtx, row: &TwinSettingRow) {
    ui.horizontal(|ui| {
        let label = ui.label(&row.key);
        label.on_hover_text("Twin-scoped value persisted in twin.toml [settings]");
        let mut changed = None;
        match &row.value {
            TwinSettingValue::Bool(current) => {
                let mut value = *current;
                if ui.checkbox(&mut value, "enabled").changed() {
                    changed = Some(TwinSettingInput::Bool(value));
                }
            }
            TwinSettingValue::Integer(current) => {
                let mut value = *current;
                if ui.add(egui::DragValue::new(&mut value)).changed() {
                    changed = Some(TwinSettingInput::Integer(value));
                }
            }
            TwinSettingValue::Number(current) => {
                let mut value = *current;
                if ui.add(egui::DragValue::new(&mut value)).changed() {
                    changed = Some(TwinSettingInput::Number(value));
                }
            }
            TwinSettingValue::Text(current) => {
                let mut value = current.clone();
                if ui
                    .add(text_editor::singleline(&mut value).desired_width(150.0))
                    .changed()
                {
                    changed = Some(TwinSettingInput::Text(value));
                }
            }
        }
        if ui
            .small_button("Reset")
            .on_hover_text("Remove this value and use the setting's authored default")
            .clicked()
        {
            ctx.trigger(ResetTwinSetting {
                key: row.key.clone(),
            });
        } else if let Some(value) = changed {
            ctx.trigger(SetTwinSetting {
                key: row.key.clone(),
                value,
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::matches_query;

    #[test]
    fn settings_filter_matches_keys_and_namespaces_case_insensitively() {
        assert!(matches_query("ui.camera_status", "CAMERA"));
        assert!(matches_query("ui.camera_status", "ui."));
        assert!(!matches_query("ui.camera_status", "modelica"));
        assert!(matches_query("ui.camera_status", "  "));
    }
}
