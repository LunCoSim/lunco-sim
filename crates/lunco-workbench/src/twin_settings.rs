//! Generic Twin-scoped settings editor.
//!
//! The Twin manifest's `[settings]` map is the only settings inventory here.
//! This panel does not maintain a second per-setting registry: it snapshots
//! the typed values from the workspace and sends edits through the generic
//! workspace commands.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_twin::TwinSettingValue;
use lunco_workspace::{
    ResetTwinSetting, SetTwinSetting, TwinClosed, TwinId, TwinSettingInput, WorkspaceResource,
};

use crate::{Panel, PanelCtx, PanelId, PanelMenuGroup, PanelSlot};

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
    let active_twin = workspace.active_twin;
    let rows = active_twin
        .and_then(|id| workspace.twin(id))
        .and_then(|twin| twin.manifest.as_ref())
        .map(|manifest| {
            manifest
                .settings
                .iter()
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
pub(crate) struct TwinSettingsPanel;

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
        PanelMenuGroup::Tools
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let view = ctx.resource_expect::<TwinSettingsView>().clone();
        ctx.panel_content_frame().show(ui, |ui| {
            ui.heading("Twin settings");
            ui.label(
                egui::RichText::new("Twin scope · twin.toml [settings]")
                    .weak()
                    .small(),
            );

            let Some(active_twin) = view.active_twin else {
                ui.separator();
                ui.label("Open a Twin to inspect its settings.");
                return;
            };
            ui.label(
                egui::RichText::new(format!("Active Twin {}", active_twin.raw()))
                    .weak()
                    .small(),
            );
            ui.separator();

            if view.rows.is_empty() {
                ui.label("This Twin has no generic settings.");
                return;
            }

            let mut group = None;
            for row in &view.rows {
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
            }
        });
    }
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
                    .add(egui::TextEdit::singleline(&mut value).desired_width(150.0))
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
