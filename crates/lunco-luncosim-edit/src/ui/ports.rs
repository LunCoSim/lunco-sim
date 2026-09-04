//! Universal runtime port inspection and control.
//!
//! The registry is the only source of port identity and value semantics. This
//! module projects it into a small, change-gated view-model so the panel can
//! browse every port-bearing entity without scanning the ECS during egui paint.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_core::ports::{PortDirection, PortInfo, PortMetadata, PortRegistry};
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

/// A port row shown by [`PortPanel`].
#[derive(Clone)]
pub struct PortRow {
    /// The entity that owns the port surface.
    pub entity: Entity,
    /// Port identity.
    pub info: PortInfo,
    /// Whether an authored/runtime wire currently targets or sources this name.
    pub wired: bool,
    /// The persisted manual setpoint, if the operator has taken this input.
    pub held: Option<f64>,
}

/// One port-bearing entity in the universal browser.
#[derive(Clone)]
pub struct PortEntity {
    /// Entity used by the typed command surface.
    pub entity: Entity,
    /// Human-readable name or authored prim path.
    pub label: String,
    /// Stable API identity when the entity has one.
    pub api_id: Option<u64>,
    /// Its complete shared port surface.
    pub ports: Vec<PortRow>,
}

/// Change-gated render state for the universal port panel.
#[derive(Resource, Default, Clone)]
pub struct PortView {
    /// Port-bearing entities sorted by label.
    pub entities: Vec<PortEntity>,
    /// Last world time at which the live table was sampled.
    pub sampled_at: f64,
}

/// Rebuild the table at operator-readable cadence. Avian state ports can change
/// without a common marker component, so a bounded 10 Hz sample is the honest
/// shared gate for this diagnostic/control surface; it avoids an O(n) rebuild on
/// every render frame while keeping live values responsive.
pub fn port_view_due(mut first: Local<bool>, time: Res<Time>, view: Res<PortView>) -> bool {
    let now = time.elapsed_secs_f64();
    let due = !*first || now - view.sampled_at >= 0.1;
    *first = true;
    due
}

/// Project all registered port backends into the panel's render model.
pub fn populate_port_view(world: &mut World) {
    let Some(registry) = world.get_resource::<PortRegistry>().cloned() else {
        world.resource_mut::<PortView>().entities.clear();
        return;
    };
    let holds = world
        .get_resource::<lunco_cosim::PortHolds>()
        .map(lunco_cosim::PortHolds::snapshot)
        .unwrap_or_default();
    let wired: HashSet<(Entity, String)> = world
        .query::<&lunco_cosim::SimConnection>()
        .iter(world)
        .flat_map(|connection| {
            [
                (connection.start_element, connection.start_connector.clone()),
                (connection.end_element, connection.end_connector.clone()),
            ]
        })
        .collect();

    let entities: Vec<(Entity, String, Option<u64>)> = world
        .query::<(Entity, Option<&Name>, Option<&lunco_core::GlobalEntityId>)>()
        .iter(world)
        .map(|(entity, name, global_id)| {
            let label = name
                .map(|name| name.as_str().to_owned())
                .or_else(|| global_id.map(|id| format!("Entity {}", id.get())))
                .unwrap_or_else(|| format!("{entity:?}"));
            (
                entity,
                label,
                global_id.map(lunco_core::GlobalEntityId::get),
            )
        })
        .collect();

    let mut rows = Vec::new();
    for (entity, label, api_id) in entities {
        let infos = registry.entity_port_infos(world, entity);
        if infos.is_empty() {
            continue;
        }
        let mut ports: Vec<_> = infos
            .into_iter()
            .map(|info| PortRow {
                entity,
                wired: wired.contains(&(entity, info.name.clone())),
                held: holds.get(&(entity, info.name.clone())).copied(),
                info,
            })
            .collect();
        ports.sort_by(|a, b| a.info.name.cmp(&b.info.name));
        rows.push(PortEntity {
            entity,
            label,
            api_id,
            ports,
        });
    }
    rows.sort_by(|a, b| a.label.cmp(&b.label));

    let sampled_at = world.resource::<Time>().elapsed_secs_f64();
    let mut view = world.resource_mut::<PortView>();
    view.entities = rows;
    view.sampled_at = sampled_at;
}

#[derive(Default)]
pub struct PortPanel {
    filter: String,
    drafts: HashMap<(Entity, String), String>,
}

impl Panel for PortPanel {
    fn id(&self) -> PanelId {
        PanelId("port_inspector")
    }

    fn title(&self) -> String {
        "Ports".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }

    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }

    fn transparent_background(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let Some(view) = ctx.resource::<PortView>().cloned() else {
            ui.label("Port view is not active.");
            return;
        };

        ctx.panel_content_frame().show(ui, |ui| {
            ui.heading("Ports");
            ui.label(
                egui::RichText::new(
                    "Inspect every registered vehicle/system port. Writes use the shared SetPorts command.",
                )
                .small()
                .weak(),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Filter");
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("entity, port, source, or authority")
                        .desired_width(ui.available_width()),
                );
            });
            ui.label(
                egui::RichText::new(format!(
                    "{} entities · {} ports · sampled at {:.1} Hz",
                    view.entities.len(),
                    view.entities.iter().map(|entity| entity.ports.len()).sum::<usize>(),
                    10.0,
                ))
                .small()
                .weak(),
            );

            let filter = self.filter.trim().to_lowercase();
            for entity in &view.entities {
                let matching_ports: Vec<_> = entity
                    .ports
                    .iter()
                    .filter(|row| port_matches(row, &entity.label, &filter))
                    .collect();
                if matching_ports.is_empty() {
                    continue;
                }
                let title = format!("{}  ({})", entity.label, matching_ports.len());
                egui::CollapsingHeader::new(title)
                    .default_open(!filter.is_empty() || view.entities.len() == 1)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.small(format!("entity {:?}", entity.entity));
                            if let Some(api_id) = entity.api_id {
                                ui.small(format!("api_id {api_id}"));
                            }
                        });
                        egui::Grid::new(("port_rows", entity.entity))
                            .striped(true)
                            .num_columns(5)
                            .show(ui, |ui| {
                                ui.strong("Port");
                                ui.strong("Value");
                                ui.strong("Type / unit");
                                ui.strong("Source / authority");
                                ui.strong("Control");
                                ui.end_row();

                                for row in &matching_ports {
                                    self.render_row(ui, ctx, row);
                                    ui.end_row();
                                }
                            });
                    });
            }
        });
    }
}

impl PortPanel {
    fn render_row(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx, row: &PortRow) {
        let info = &row.info;
        ui.vertical(|ui| {
            ui.label(&info.name);
            ui.small(direction_label(info.direction));
            if row.wired {
                ui.small(egui::RichText::new("wired").weak());
            }
        });

        if let Some(held) = row.held {
            ui.label(format!("{:.6}  (held)", held));
        } else {
            ui.label(format_value(info.value));
        }

        ui.vertical(|ui| {
            ui.label(info.metadata.value_type);
            ui.small(
                info.metadata
                    .unit
                    .as_deref()
                    .unwrap_or("unitless / unspecified"),
            );
            if let Some(range) = range_label(&info.metadata) {
                ui.small(range);
            }
        });

        ui.vertical(|ui| {
            ui.label(&info.metadata.source);
            ui.small(&info.metadata.authority);
        });

        if !info.metadata.writable {
            ui.small(egui::RichText::new("read-only").weak());
            return;
        }

        let key = (row.entity, info.name.clone());
        let draft = self
            .drafts
            .entry(key.clone())
            .or_insert_with(|| format!("{:.9}", row.held.unwrap_or(info.value)));
        let validation = draft
            .parse::<f64>()
            .map_err(|_| "enter a number".to_owned())
            .and_then(|value| info.metadata.validate(value).map(|()| value));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(draft).desired_width(82.0));
            if ui
                .add_enabled(validation.is_ok(), egui::Button::new("Apply"))
                .clicked()
            {
                if let Ok(value) = validation.as_ref() {
                    ctx.trigger(lunco_cosim::SetPorts {
                        target: row.entity,
                        writes: vec![(info.name.clone(), *value)],
                        seq: 0,
                        tick: 0,
                    });
                }
            }
            if row.held.is_some()
                && ui
                    .button("Release")
                    .on_hover_text("Return this input to its authored wiring")
                    .clicked()
            {
                ctx.trigger(lunco_cosim::ReleasePort {
                    target: row.entity,
                    name: info.name.clone(),
                });
            }
        });
        if let Err(error) = validation {
            ui.small(egui::RichText::new(error).color(egui::Color32::RED));
        }
    }
}

fn port_matches(row: &PortRow, entity_label: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    [
        entity_label,
        &row.info.name,
        &row.info.metadata.source,
        &row.info.metadata.authority,
        row.info.metadata.unit.as_deref().unwrap_or_default(),
    ]
    .iter()
    .any(|value| value.to_lowercase().contains(filter))
}

fn direction_label(direction: PortDirection) -> &'static str {
    match direction {
        PortDirection::In => "in",
        PortDirection::Out => "out",
        PortDirection::InOut => "in / out",
    }
}

fn format_value(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.6}")
    } else {
        "invalid".into()
    }
}

fn range_label(metadata: &PortMetadata) -> Option<String> {
    match (metadata.min, metadata.max) {
        (Some(min), Some(max)) => Some(format!("range {min}..{max}")),
        (Some(min), None) => Some(format!("min {min}")),
        (None, Some(max)) => Some(format!("max {max}")),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_matches_metadata_as_well_as_port_name() {
        let row = PortRow {
            entity: Entity::PLACEHOLDER,
            info: PortInfo {
                name: "throttle".into(),
                direction: PortDirection::In,
                value: 0.0,
                metadata: PortMetadata::scalar(
                    PortDirection::In,
                    None,
                    Some(-1.0),
                    Some(1.0),
                    "rover controller",
                    "operator",
                    true,
                ),
            },
            wired: false,
            held: None,
        };
        assert!(port_matches(&row, "Rover", "controller"));
        assert!(port_matches(&row, "Rover", "throttle"));
        assert!(!port_matches(&row, "Rover", "lander"));
    }

    #[test]
    fn metadata_validation_rejects_non_finite_and_out_of_range_values() {
        let metadata = PortMetadata::scalar(
            PortDirection::In,
            None,
            Some(-1.0),
            Some(1.0),
            "control",
            "operator",
            true,
        );
        assert!(metadata.validate(0.5).is_ok());
        assert!(metadata.validate(2.0).is_err());
        assert!(metadata.validate(f64::NAN).is_err());
    }
}
