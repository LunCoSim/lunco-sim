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
use lunco_port_core::ports::{
    PortDirection, PortHandle, PortInfo, PortMetadata, PortRegistry, PortTopologyRevision,
};
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelSlot, WorkbenchSnapshot};

/// Stable id of the universal port inspection panel.
pub const PORT_PANEL_ID: PanelId = PanelId("port_inspector");

/// A port row shown by [`PortPanel`].
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
    /// Formatted live/held value, refreshed with the sampled port data.
    value_text: String,
    /// Formatted authored range, stable while port topology is unchanged.
    range_text: Option<String>,
}

/// One port-bearing entity in the universal browser.
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
#[derive(Resource, Default)]
pub struct PortView {
    /// Port-bearing entities sorted by label.
    pub entities: Vec<PortEntity>,
    /// Total number of projected ports, retained for the panel summary.
    pub total_ports: usize,
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

#[derive(Default)]
struct PortRowIndexCache {
    built: bool,
    topology_revision: Option<u64>,
    filter: String,
    expanded_entities: HashSet<Entity>,
    expanded_indices: Vec<usize>,
    collapsed_indices: Vec<usize>,
    row_titles: Vec<Option<String>>,
}

struct PortDraft {
    value: String,
    validation: Result<f64, String>,
}

impl PortRowIndexCache {
    fn matches(
        &self,
        topology_revision: Option<u64>,
        filter: &str,
        expanded_entities: &HashSet<Entity>,
    ) -> bool {
        self.built
            && self.topology_revision == topology_revision
            && self.filter == filter
            && self.expanded_entities == *expanded_entities
    }

    fn rebuild(
        &mut self,
        view: &PortView,
        topology_revision: Option<u64>,
        filter: &str,
        expanded_entities: &HashSet<Entity>,
        matching_indices: &[usize],
        matching_counts: &[usize],
    ) {
        let auto_expand_single = expanded_entities.is_empty() && matching_indices.len() == 1;
        self.expanded_indices.clear();
        self.collapsed_indices.clear();
        self.row_titles.clear();
        self.row_titles.resize_with(view.entities.len(), || None);
        for &index in matching_indices {
            let entity = view.entities[index].entity;
            self.row_titles[index] = Some(format!(
                "{}  ({})",
                view.entities[index].label, matching_counts[index]
            ));
            if expanded_entities.contains(&entity) || auto_expand_single {
                self.expanded_indices.push(index);
            } else {
                self.collapsed_indices.push(index);
            }
        }
        self.topology_revision = topology_revision;
        self.filter.clear();
        self.filter.push_str(filter);
        self.expanded_entities.clone_from(expanded_entities);
        self.built = true;
    }
}

fn build_port_match_cache(view: &PortView, filter: &str) -> PortMatchCache {
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
        filter: filter.to_owned(),
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
        .map(|(owner, info)| {
            let held = holds
                .get(&entity)
                .and_then(|ports| ports.get(&info.name).copied());
            let value_text = display_value(held, info.value);
            let range_text = range_label(&info.metadata);
            PortRow {
                entity,
                owner,
                wired: wired
                    .get(&entity)
                    .is_some_and(|ports| ports.contains(&info.name)),
                held,
                info,
                value_text,
                range_text,
            }
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

fn refresh_display_values(entities: &mut [PortEntity], requested_entities: &HashSet<Entity>) {
    for entity in entities {
        if !requested_entities.contains(&entity.entity) {
            continue;
        }
        for row in &mut entity.ports {
            row.value_text = display_value(row.held, row.info.value);
        }
    }
}

/// Project all registered port backends into the panel's render model.
pub fn populate_port_view(world: &mut World) {
    let Some(registry) = world.get_resource::<PortRegistry>().cloned() else {
        let mut view = world.resource_mut::<PortView>();
        view.entities.clear();
        view.total_ports = 0;
        view.candidate_topology_keys.clear();
        view.candidate_topology_revision = None;
        return;
    };
    let requested_entities = world
        .get_resource::<PortInspectionRequest>()
        .map(|request| request.expanded_entities.clone())
        .unwrap_or_default();
    let holds: HashMap<Entity, HashMap<String, f64>> = world
        .get_resource::<lunco_cosim_core::PortHolds>()
        .map(lunco_cosim_core::PortHolds::snapshot)
        .unwrap_or_default()
        .into_iter()
        .fold(HashMap::new(), |mut by_entity, ((entity, name), value)| {
            by_entity.entry(entity).or_default().insert(name, value);
            by_entity
        });
    let wired: HashMap<Entity, HashSet<String>> = world
        .query::<&lunco_cosim_core::SimConnection>()
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
        refresh_display_values(&mut entities, &requested_entities);
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
    refresh_display_values(&mut rows, &requested_entities);
    let total_ports = rows.iter().map(|entity| entity.ports.len()).sum();
    let mut view = world.resource_mut::<PortView>();
    view.entities = rows;
    view.total_ports = total_ports;
    view.candidate_topology_keys = candidate_topology_keys;
    view.candidate_topology_revision = Some(topology_revision);
    view.sampled_at = sampled_at;
}

#[derive(Default)]
pub struct PortPanel {
    filter: String,
    normalized_filter: String,
    drafts: HashMap<Entity, HashMap<String, PortDraft>>,
    expanded_entities: HashSet<Entity>,
    next_expanded_entities: HashSet<Entity>,
    matching_cache: Option<PortMatchCache>,
    row_index: PortRowIndexCache,
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
        lunco_workbench_core::PanelMenuGroup::Builder
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
            let response = ui.add(
                lunco_workbench_widgets::text_editor::singleline(&mut self.filter)
                    .hint_text("entity, port, source, or authority")
                    .desired_width(ui.available_width()),
            );
            if response.changed() {
                self.normalized_filter = self.filter.trim().to_lowercase();
            }
        });
        ui.label(
            egui::RichText::new(format!(
                "{} entities · {} ports · sampled at {:.1} Hz",
                view.entities.len(),
                view.total_ports,
                10.0,
            ))
            .small()
            .weak(),
        );

        let topology_revision = view.candidate_topology_revision;
        let cache_stale = self.matching_cache.as_ref().is_none_or(|cache| {
            cache.topology_revision != topology_revision || cache.filter != self.normalized_filter
        });
        if cache_stale {
            self.matching_cache = Some(build_port_match_cache(view, &self.normalized_filter));
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

        let auto_expand_single = self.expanded_entities.is_empty() && matching_indices.len() == 1;
        if !self.row_index.matches(
            topology_revision,
            &self.normalized_filter,
            &self.expanded_entities,
        ) {
            self.row_index.rebuild(
                view,
                topology_revision,
                &self.normalized_filter,
                &self.expanded_entities,
                matching_indices,
                matching_counts,
            );
        }

        // Expanded bodies are painted outside the virtualized header list. The
        // stable row index is rebuilt only for topology, filter, or expansion
        // changes; widgets are still painted for the current frame.
        let expanded_indices = std::mem::take(&mut self.row_index.expanded_indices);
        let collapsed_indices = std::mem::take(&mut self.row_index.collapsed_indices);
        let row_titles = std::mem::take(&mut self.row_index.row_titles);
        let mut next_expanded = std::mem::take(&mut self.next_expanded_entities);
        next_expanded.clear();
        for index in expanded_indices.iter().copied() {
            let entity = &view.entities[index];
            if self.render_entity(
                ui,
                ctx,
                entity,
                row_titles[index]
                    .as_deref()
                    .expect("matching port entity has a cached row title"),
                &cache.matching_port_indices[index],
                self.expanded_entities.contains(&entity.entity) || auto_expand_single,
            ) {
                next_expanded.insert(entity.entity);
            }
        }

        let row_height = ui.text_style_height(&egui::TextStyle::Body);
        egui::ScrollArea::vertical()
            .id_salt("port_entity_browser")
            .auto_shrink([false; 2])
            .show_rows(ui, row_height, collapsed_indices.len(), |ui, range| {
                for row_index in range {
                    let index = collapsed_indices[row_index];
                    let entity = &view.entities[index];
                    let title = row_titles[index]
                        .as_deref()
                        .expect("matching port entity has a cached row title");
                    if lunco_workbench_widgets::tree::branch(
                        ui,
                        ui.make_persistent_id(("port_entity", entity.entity)),
                        false,
                        None,
                        |ui| {
                            let width = ui.available_width();
                            lunco_workbench_widgets::tree::label(
                                ui,
                                title,
                                width,
                                egui::Sense::click(),
                            )
                            .clicked()
                        },
                        |_| {},
                    )
                    .is_open
                    {
                        next_expanded.insert(entity.entity);
                    }
                }
            });
        if next_expanded != self.expanded_entities {
            std::mem::swap(&mut next_expanded, &mut self.expanded_entities);
        }
        next_expanded.clear();
        let _ = ctx.resource_scope::<PortInspectionRequest, _>(|_, request| {
            if request.expanded_entities != self.expanded_entities {
                request
                    .expanded_entities
                    .clone_from(&self.expanded_entities);
            }
        });
        self.next_expanded_entities = next_expanded;
        self.row_index.expanded_indices = expanded_indices;
        self.row_index.collapsed_indices = collapsed_indices;
        self.row_index.row_titles = row_titles;
        self.matching_cache = Some(cache);
    }

    fn render_entity(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &mut PanelCtx,
        entity: &PortEntity,
        title: &str,
        matching_port_indices: &[usize],
        default_open: bool,
    ) -> bool {
        lunco_workbench_widgets::tree::branch(
            ui,
            ui.make_persistent_id(("port_entity", entity.entity)),
            default_open,
            None,
            |ui| {
                let width = ui.available_width();
                lunco_workbench_widgets::tree::label(ui, title, width, egui::Sense::click())
                    .clicked()
            },
            |ui| {
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
            },
        )
        .is_open
    }
}

impl PortPanel {
    fn render_row(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx, row: &PortRow) {
        let info = &row.info;
        let local_origin = ctx
            .resource::<lunco_core_session::LocalSession>()
            .map(|session| lunco_core::CommandOrigin::LocalUser {
                session_id: session.0,
            });
        ui.vertical(|ui| {
            ui.label(&info.name);
            ui.small(direction_label(info.direction));
            if row.wired {
                ui.small(egui::RichText::new("wired").weak());
            }
        });

        ui.label(&row.value_text);

        ui.vertical(|ui| {
            ui.label(info.metadata.value_type);
            ui.small(
                info.metadata
                    .unit
                    .as_deref()
                    .unwrap_or("unitless / unspecified"),
            );
            if let Some(range) = row.range_text.as_deref() {
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

        let entity_drafts = self.drafts.entry(row.entity).or_default();
        if !entity_drafts.contains_key(info.name.as_str()) {
            let value = format!("{:.9}", row.held.unwrap_or(info.value));
            let validation = validate_port_value(info, &value);
            entity_drafts.insert(info.name.clone(), PortDraft { value, validation });
        }
        let draft = entity_drafts
            .get_mut(info.name.as_str())
            .expect("port draft was inserted above");
        ui.horizontal(|ui| {
            let response = ui.add(
                lunco_workbench_widgets::text_editor::singleline(&mut draft.value)
                    .desired_width(82.0),
            );
            if response.changed() {
                draft.validation = validate_port_value(info, &draft.value);
            }
            if ui
                .add_enabled(
                    draft.validation.is_ok() && local_origin.is_some(),
                    egui::Button::new("Apply"),
                )
                .clicked()
            {
                if let (Ok(value), Some(origin)) = (&draft.validation, local_origin) {
                    ctx.trigger_command(
                        lunco_cosim_core::commands::SetPorts {
                            target: row.entity,
                            writes: vec![(info.name.clone(), *value)],
                            seq: 0,
                            tick: 0,
                            producer_id: None,
                        },
                        origin,
                    );
                }
            }
            if row.held.is_some()
                && ui
                    .add_enabled(local_origin.is_some(), egui::Button::new("Release"))
                    .on_hover_text("Return this input to its authored wiring")
                    .clicked()
            {
                if let Some(origin) = local_origin {
                    ctx.trigger_command(
                        lunco_cosim_core::commands::ReleasePort {
                            target: row.entity,
                            name: info.name.clone(),
                            producer_id: None,
                        },
                        origin,
                    );
                }
            }
        });
        if local_origin.is_none() {
            ui.small(
                egui::RichText::new("local control session unavailable").color(egui::Color32::RED),
            );
        }
        if let Err(error) = &draft.validation {
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

fn display_value(held: Option<f64>, value: f64) -> String {
    match held {
        Some(held) => format!("{held:.6}  (held)"),
        None => format_value(value),
    }
}

fn validate_port_value(info: &PortInfo, value: &str) -> Result<f64, String> {
    value
        .parse::<f64>()
        .map_err(|_| "enter a number".to_owned())
        .and_then(|value| info.metadata.validate(value).map(|()| value))
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
            value_text: "0.000000".into(),
            range_text: Some("range -1..1".into()),
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
            value_text: "0.000000".into(),
            range_text: Some("range -1..1".into()),
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
                lunco_port_core::InputPorts::new(&["throttle"]),
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
            .spawn((
                Name::new("second"),
                lunco_port_core::InputPorts::new(&["arm"]),
            ))
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
