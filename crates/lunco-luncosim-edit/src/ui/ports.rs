//! Universal runtime port inspection and control.
//!
//! The registry is the only source of port identity and value semantics. This
//! module projects it into a small, change-gated view-model so the panel can
//! browse every port-bearing entity without scanning the ECS during egui paint.
//! Entity discovery is delegated to each registered backend through the shared
//! registry; the panel never probes the whole world to find port owners.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_core::ports::{PortDirection, PortHandle, PortInfo, PortMetadata, PortRegistry};
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelSlot, WorkbenchSnapshot};

/// Stable id of the universal port inspection panel.
pub const PORT_PANEL_ID: PanelId = PanelId("port_inspector");

/// A port row shown by [`PortPanel`].
#[derive(Clone)]
pub struct PortRow {
    /// The entity that owns the port surface.
    pub entity: Entity,
    /// The backend that produced this inspection row.
    owner: PortHandle,
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
    /// Backend-owned identity keys for the current candidate set.
    candidate_topology_keys: HashMap<Entity, u64>,
    /// Labels are cached with the port rows so a name change also rebuilds the
    /// metadata projection without sampling every backend each tick.
    candidate_labels: HashMap<Entity, (String, Option<u64>)>,
}

/// Rebuild the table at operator-readable cadence. A bounded 10 Hz sample is the
/// honest shared gate for this diagnostic/control surface; the registry supplies
/// backend-owned entity candidates so sampling does not probe every ECS entity.
pub fn port_view_due(
    mut first: Local<bool>,
    time: Res<Time>,
    view: Res<PortView>,
    layout: Option<Res<WorkbenchSnapshot>>,
) -> bool {
    if !layout.is_some_and(|layout| layout.is_panel_visible(PORT_PANEL_ID)) {
        return false;
    }
    let now = time.elapsed_secs_f64();
    let due = !*first || now - view.sampled_at >= 0.1;
    *first = true;
    due
}

fn build_port_rows(
    registry: &PortRegistry,
    world: &World,
    entity: Entity,
    wired: &HashMap<Entity, HashSet<String>>,
    holds: &HashMap<Entity, HashMap<String, f64>>,
) -> Vec<PortRow> {
    let mut ports: Vec<_> = registry
        .entity_port_infos_with_handles(world, entity)
        .into_iter()
        .map(|(owner, info)| PortRow {
            entity,
            owner,
            wired: wired
                .get(&entity)
                .is_some_and(|ports| ports.contains(&info.name)),
            held: holds
                .get(&entity)
                .and_then(|ports| ports.get(&info.name).copied()),
            info,
        })
        .collect();
    ports.sort_by(|a, b| a.info.name.cmp(&b.info.name));
    ports
}

fn refresh_live_values(registry: &PortRegistry, world: &World, entities: &mut [PortEntity]) {
    for entity in entities {
        for row in &mut entity.ports {
            if let Some(value) = registry.read_port_for_handle(
                world,
                row.owner,
                row.entity,
                &row.info.name,
                row.info.direction,
            ) {
                row.info.value = value;
            }
        }
    }
}

fn refresh_decorations(
    wired: &HashMap<Entity, HashSet<String>>,
    holds: &HashMap<Entity, HashMap<String, f64>>,
    entities: &mut [PortEntity],
) {
    for entity in entities {
        for row in &mut entity.ports {
            row.wired = wired
                .get(&row.entity)
                .is_some_and(|ports| ports.contains(&row.info.name));
            row.held = holds
                .get(&row.entity)
                .and_then(|ports| ports.get(&row.info.name).copied());
        }
    }
}

/// Project all registered port backends into the panel's render model.
pub fn populate_port_view(world: &mut World) {
    let Some(registry) = world.get_resource::<PortRegistry>().cloned() else {
        let mut view = world.resource_mut::<PortView>();
        view.entities.clear();
        view.candidate_topology_keys.clear();
        view.candidate_labels.clear();
        return;
    };
    let holds: HashMap<Entity, HashMap<String, f64>> = world
        .get_resource::<lunco_cosim::PortHolds>()
        .map(lunco_cosim::PortHolds::snapshot)
        .unwrap_or_default()
        .into_iter()
        .fold(HashMap::new(), |mut by_entity, ((entity, name), value)| {
            by_entity.entry(entity).or_default().insert(name, value);
            by_entity
        });
    let wired: HashMap<Entity, HashSet<String>> = world
        .query::<&lunco_cosim::SimConnection>()
        .iter(world)
        .flat_map(|connection| {
            [
                (connection.start_element, connection.start_connector.clone()),
                (connection.end_element, connection.end_connector.clone()),
            ]
        })
        .fold(HashMap::new(), |mut by_entity, (entity, name)| {
            by_entity.entry(entity).or_default().insert(name);
            by_entity
        });

    let candidates = {
        registry
            .port_entities_with_topology_keys(world)
            .into_iter()
            .map(|(entity, key)| {
                let name = world.get::<Name>(entity);
                let global_id = world.get::<lunco_core::GlobalEntityId>(entity);
                let label = name
                    .map(|name| name.as_str().to_owned())
                    .or_else(|| global_id.map(|id| format!("Entity {}", id.get())))
                    .unwrap_or_else(|| format!("{entity:?}"));
                (
                    entity,
                    label,
                    global_id.map(lunco_core::GlobalEntityId::get),
                    key,
                )
            })
            .collect::<Vec<_>>()
    };

    let sampled_at = world.resource::<Time>().elapsed_secs_f64();
    let topology_changed = {
        let view = world.resource::<PortView>();
        view.candidate_topology_keys.len() != candidates.len()
            || candidates.iter().any(|(entity, label, api_id, key)| {
                view.candidate_topology_keys.get(entity).copied() != Some(*key)
                    || view
                        .candidate_labels
                        .get(entity)
                        .is_none_or(|old| old.0 != *label || old.1 != *api_id)
            })
    };

    if !topology_changed {
        // Port identity and metadata are unchanged. Refresh only values and the
        // small wire/hold decorations; no backend list or metadata callback runs
        // in the normal 10 Hz sample path.
        let mut entities = {
            let mut view = world.resource_mut::<PortView>();
            std::mem::take(&mut view.entities)
        };
        refresh_live_values(&registry, world, &mut entities);
        refresh_decorations(&wired, &holds, &mut entities);
        let mut view = world.resource_mut::<PortView>();
        view.entities = entities;
        view.sampled_at = sampled_at;
        return;
    }

    let (old_topology_keys, old_entities) = {
        let mut view = world.resource_mut::<PortView>();
        (
            std::mem::take(&mut view.candidate_topology_keys),
            std::mem::take(&mut view.entities),
        )
    };
    let mut existing_entities: HashMap<_, _> = old_entities
        .into_iter()
        .map(|entity| (entity.entity, entity))
        .collect();
    let mut rows = Vec::with_capacity(candidates.len());
    let mut candidate_topology_keys = HashMap::with_capacity(candidates.len());
    let mut candidate_labels = HashMap::with_capacity(candidates.len());
    for (entity, label, api_id, key) in candidates {
        let topology_unchanged = old_topology_keys.get(&entity).copied() == Some(key);
        candidate_topology_keys.insert(entity, key);
        candidate_labels.insert(entity, (label.clone(), api_id));

        let Some(mut port_entity) = existing_entities.remove(&entity) else {
            let ports = build_port_rows(&registry, world, entity, &wired, &holds);
            if !ports.is_empty() {
                rows.push(PortEntity {
                    entity,
                    label,
                    api_id,
                    ports,
                });
            }
            continue;
        };

        if !topology_unchanged {
            port_entity.ports = build_port_rows(&registry, world, entity, &wired, &holds);
        }
        port_entity.label = label;
        port_entity.api_id = api_id;
        if !port_entity.ports.is_empty() {
            rows.push(port_entity);
        }
    }
    rows.sort_by(|a, b| a.label.cmp(&b.label));
    refresh_live_values(&registry, world, &mut rows);
    refresh_decorations(&wired, &holds, &mut rows);
    let mut view = world.resource_mut::<PortView>();
    view.entities = rows;
    view.candidate_topology_keys = candidate_topology_keys;
    view.candidate_labels = candidate_labels;
    view.sampled_at = sampled_at;
}

#[derive(Default)]
pub struct PortPanel {
    filter: String,
    drafts: HashMap<(Entity, String), String>,
}

impl Panel for PortPanel {
    fn id(&self) -> PanelId {
        PORT_PANEL_ID
    }

    fn title(&self) -> String {
        "Ports".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }

    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Scene
    }

    fn transparent_background(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        if ctx
            .resource_scope::<PortView, _>(|ctx, view| {
                ctx.panel_content_frame().show(ui, |ui| {
                    self.render_view(ui, ctx, view);
                });
            })
            .is_none()
        {
            ui.label("Port view is not active.");
        }
    }
}

impl PortPanel {
    fn render_view(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx, view: &PortView) {
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
                lunco_workbench::text_editor::singleline(&mut self.filter)
                    .hint_text("entity, port, source, or authority")
                    .desired_width(ui.available_width()),
            );
        });
        ui.label(
            egui::RichText::new(format!(
                "{} entities · {} ports · sampled at {:.1} Hz",
                view.entities.len(),
                view.entities
                    .iter()
                    .map(|entity| entity.ports.len())
                    .sum::<usize>(),
                10.0,
            ))
            .small()
            .weak(),
        );

        let filter = self.filter.trim().to_lowercase();
        for entity in &view.entities {
            let matching_count = entity
                .ports
                .iter()
                .filter(|row| port_matches(row, &entity.label, &filter))
                .count();
            if matching_count == 0 {
                continue;
            }
            let title = format!("{}  ({matching_count})", entity.label);
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

                            for row in entity
                                .ports
                                .iter()
                                .filter(|row| port_matches(row, &entity.label, &filter))
                            {
                                self.render_row(ui, ctx, row);
                                ui.end_row();
                            }
                        });
                });
        }
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
            ui.add(lunco_workbench::text_editor::singleline(draft).desired_width(82.0));
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
            owner: PortHandle::default(),
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

    #[test]
    fn port_view_uses_backend_owned_candidates() {
        let mut world = World::new();
        world.init_resource::<PortView>();
        world.insert_resource(PortRegistry::default());
        world.insert_resource(Time::<()>::default());
        let owned = world
            .spawn((
                Name::new("owned"),
                lunco_core::InputPorts::new(&["throttle"]),
            ))
            .id();
        world.spawn(Name::new("not a port owner"));

        populate_port_view(&mut world);

        let view = world.resource::<PortView>();
        assert_eq!(view.entities.len(), 1);
        assert_eq!(view.entities[0].entity, owned);
        assert_eq!(view.entities[0].ports[0].info.name, "throttle");

        let registry = world.resource::<PortRegistry>().clone();
        assert!(registry.write_port(&mut world, owned, "throttle", 0.75));
        populate_port_view(&mut world);
        assert_eq!(
            world.resource::<PortView>().entities[0].ports[0].info.value,
            0.75
        );
    }
}
