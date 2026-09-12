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
use lunco_core::ports::{
    PortDirection, PortHandle, PortInfo, PortMetadata, PortRegistry, PortTopologyRevision,
};
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
    /// Last owner-published port-surface generation projected into `entities`.
    candidate_topology_revision: Option<u64>,
}

/// Entities whose port bodies were actually visible in the last egui pass.
///
/// An empty set is meaningful: collapsed bodies have no live-value consumer and
/// must not make the Update schedule read tens of thousands of ports. The panel
/// publishes the next set after painting, so a disclosure change takes effect on
/// the following sample.
#[derive(Resource, Default)]
pub struct PortInspectionRequest {
    expanded_entities: HashSet<Entity>,
}

/// Cached result of the panel's text filter. Matching is a presentation
/// concern, so it is keyed by the projected port topology and normalized
/// filter text rather than recomputed during every egui paint.
#[derive(Default)]
struct PortMatchCache {
    topology_revision: Option<u64>,
    filter: String,
    matching_counts: Vec<usize>,
    matching_indices: Vec<usize>,
    matching_port_indices: Vec<Vec<usize>>,
}

fn build_port_match_cache(view: &PortView, filter: String) -> PortMatchCache {
    let mut matching_counts = Vec::with_capacity(view.entities.len());
    let mut matching_indices = Vec::new();
    let mut matching_port_indices = Vec::with_capacity(view.entities.len());

    for (entity_index, entity) in view.entities.iter().enumerate() {
        let port_indices: Vec<usize> = if filter.is_empty() {
            (0..entity.ports.len()).collect()
        } else {
            entity
                .ports
                .iter()
                .enumerate()
                .filter_map(|(port_index, row)| {
                    port_matches(row, &entity.label, &filter).then_some(port_index)
                })
                .collect()
        };
        let matching_count = port_indices.len();
        if matching_count > 0 {
            matching_indices.push(entity_index);
        }
        matching_counts.push(matching_count);
        matching_port_indices.push(port_indices);
    }

    PortMatchCache {
        topology_revision: view.candidate_topology_revision,
        filter,
        matching_counts,
        matching_indices,
        matching_port_indices,
    }
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

fn refresh_live_values(
    registry: &PortRegistry,
    world: &World,
    entities: &mut [PortEntity],
    requested_entities: &HashSet<Entity>,
) {
    for entity in entities {
        if !requested_entities.contains(&entity.entity) {
            continue;
        }
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
    requested_entities: &HashSet<Entity>,
) {
    for entity in entities {
        if !requested_entities.contains(&entity.entity) {
            continue;
        }
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
        view.candidate_topology_revision = None;
        return;
    };
    let requested_entities = world
        .get_resource::<PortInspectionRequest>()
        .map(|request| request.expanded_entities.clone())
        .unwrap_or_default();
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

    let sampled_at = world.resource::<Time>().elapsed_secs_f64();
    let topology_revision = world.resource::<PortTopologyRevision>().0;
    let discover_candidates = {
        let view = world.resource::<PortView>();
        view.candidate_topology_revision != Some(topology_revision)
    };

    if !discover_candidates {
        // Port identity and metadata are unchanged. Refresh only values and the
        // small wire/hold decorations; no backend list or metadata callback runs
        // in the normal 10 Hz sample path.
        let mut entities = {
            let mut view = world.resource_mut::<PortView>();
            std::mem::take(&mut view.entities)
        };
        refresh_live_values(&registry, world, &mut entities, &requested_entities);
        refresh_decorations(&wired, &holds, &mut entities, &requested_entities);
        let mut view = world.resource_mut::<PortView>();
        view.entities = entities;
        view.sampled_at = sampled_at;
        return;
    }

    let candidates = registry
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
        .collect::<Vec<_>>();

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
    for (entity, label, api_id, key) in candidates {
        let topology_unchanged = old_topology_keys.get(&entity).copied() == Some(key);
        candidate_topology_keys.insert(entity, key);

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
    refresh_live_values(&registry, world, &mut rows, &requested_entities);
    refresh_decorations(&wired, &holds, &mut rows, &requested_entities);
    let mut view = world.resource_mut::<PortView>();
    view.entities = rows;
    view.candidate_topology_keys = candidate_topology_keys;
    view.candidate_topology_revision = Some(topology_revision);
    view.sampled_at = sampled_at;
}

#[derive(Default)]
pub struct PortPanel {
    filter: String,
    drafts: HashMap<(Entity, String), String>,
    expanded_entities: HashSet<Entity>,
    matching_cache: Option<PortMatchCache>,
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

    fn scroll_policy(&self) -> lunco_workbench_core::PanelScrollPolicy {
        lunco_workbench_core::PanelScrollPolicy::SelfManaged
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

        let normalized_filter = self.filter.trim().to_lowercase();
        let topology_revision = view.candidate_topology_revision;
        let cache_stale = self.matching_cache.as_ref().is_none_or(|cache| {
            cache.topology_revision != topology_revision || cache.filter != normalized_filter
        });
        if cache_stale {
            self.matching_cache = Some(build_port_match_cache(view, normalized_filter));
        }
        // Temporarily own the cache while painting so the expanded row controls
        // can still mutate the panel's draft state without cloning the cached
        // index vectors on every frame.
        let cache = self
            .matching_cache
            .take()
            .expect("port match cache is initialized above");
        let matching_counts = &cache.matching_counts;
        let matching_indices = &cache.matching_indices;

        // Expanded bodies are painted outside the virtualized header list. The
        // list therefore remains fixed-height even when a body contains a full
        // port grid; opening a header moves its body into this small explicit set
        // on the next paint.
        let mut next_expanded = HashSet::new();
        let auto_expand_single = self.expanded_entities.is_empty() && matching_indices.len() == 1;
        let expanded_indices: Vec<_> = matching_indices
            .iter()
            .copied()
            .filter(|index| {
                self.expanded_entities
                    .contains(&view.entities[*index].entity)
                    || auto_expand_single
            })
            .collect();
        for index in expanded_indices {
            let entity = &view.entities[index];
            if self.render_entity(
                ui,
                ctx,
                entity,
                matching_counts[index],
                &cache.matching_port_indices[index],
                self.expanded_entities.contains(&entity.entity) || auto_expand_single,
            ) {
                next_expanded.insert(entity.entity);
            }
        }

        let collapsed_indices: Vec<_> = matching_indices
            .iter()
            .copied()
            .filter(|index| !next_expanded.contains(&view.entities[*index].entity))
            .collect();
        let row_height = ui.text_style_height(&egui::TextStyle::Body);
        egui::ScrollArea::vertical()
            .id_salt("port_entity_browser")
            .auto_shrink([false; 2])
            .show_rows(ui, row_height, collapsed_indices.len(), |ui, range| {
                for row_index in range {
                    let index = collapsed_indices[row_index];
                    let entity = &view.entities[index];
                    let title = format!("{}  ({})", entity.label, matching_counts[index]);
                    let response = egui::CollapsingHeader::new(title)
                        .default_open(false)
                        .show(ui, |_| {});
                    if response.body_returned.is_some() {
                        next_expanded.insert(entity.entity);
                    }
                }
            });
        self.expanded_entities = next_expanded;
        let _ = ctx.resource_scope::<PortInspectionRequest, _>(|_, request| {
            request.expanded_entities = self.expanded_entities.clone();
        });
        self.matching_cache = Some(cache);
    }

    fn render_entity(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &mut PanelCtx,
        entity: &PortEntity,
        matching_count: usize,
        matching_port_indices: &[usize],
        default_open: bool,
    ) -> bool {
        let title = format!("{}  ({matching_count})", entity.label);
        let response = egui::CollapsingHeader::new(title)
            .default_open(default_open)
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

                        for &port_index in matching_port_indices {
                            let row = &entity.ports[port_index];
                            self.render_row(ui, ctx, row);
                            ui.end_row();
                        }
                    });
            });
        response.body_returned.is_some()
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
    fn port_match_cache_indexes_rows_for_reuse_across_paints() {
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
        let view = PortView {
            entities: vec![PortEntity {
                entity: Entity::PLACEHOLDER,
                label: "Rover".into(),
                api_id: None,
                ports: vec![row],
            }],
            candidate_topology_revision: Some(7),
            ..default()
        };

        let cache = build_port_match_cache(&view, "controller".into());
        assert_eq!(cache.topology_revision, Some(7));
        assert_eq!(cache.matching_indices, vec![0]);
        assert_eq!(cache.matching_counts, vec![1]);
        assert_eq!(cache.matching_port_indices, vec![vec![0]]);

        let cache = build_port_match_cache(&view, "lander".into());
        assert!(cache.matching_indices.is_empty());
        assert_eq!(cache.matching_counts, vec![0]);
        assert_eq!(cache.matching_port_indices, vec![Vec::<usize>::new()]);
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
        world.init_resource::<PortInspectionRequest>();
        world.insert_resource(PortRegistry::default());
        world.insert_resource(Time::<()>::default());
        world.init_resource::<PortTopologyRevision>();
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
            0.0
        );

        world
            .resource_mut::<PortInspectionRequest>()
            .expanded_entities
            .insert(owned);
        populate_port_view(&mut world);
        assert_eq!(
            world.resource::<PortView>().entities[0].ports[0].info.value,
            0.75
        );

        let second = world
            .spawn((Name::new("second"), lunco_core::InputPorts::new(&["arm"])))
            .id();
        populate_port_view(&mut world);
        assert_eq!(
            world.resource::<PortView>().entities.len(),
            1,
            "candidate discovery must not fall back to entity-count polling"
        );

        world.resource_mut::<PortTopologyRevision>().bump();
        populate_port_view(&mut world);
        let view = world.resource::<PortView>();
        assert_eq!(view.entities.len(), 2);
        assert!(view.entities.iter().any(|entity| entity.entity == second));
    }
}
