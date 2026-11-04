//! Universal runtime port inspection and control.
//!
//! The registry is the only source of port identity and value semantics. This
//! module projects it into a small, change-gated view-model so the panel can
//! browse every port-bearing entity without scanning the ECS during egui paint.
//! Entity discovery is delegated to each registered backend through the shared
//! registry; the panel never probes the whole world to find port owners.

use std::collections::{BTreeMap, HashMap, HashSet};

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
    /// Short presentation label; the exact port name remains in `info`.
    display_name: String,
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
    /// Namespace for hierarchy disclosure, isolated between composed stages.
    hierarchy_scope: egui::Id,
    /// Whether the owner comes from a currently mounted scene source.
    scene_source: bool,
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

/// Owners of visible port rows or the selected port in the last egui pass.
///
/// An empty set is meaningful: collapsed, unselected ports have no consumer and
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
    all_sources: bool,
    port_count: usize,
    roots: BTreeMap<u64, PortTreeNode>,
    matching_port_indices: Vec<Vec<usize>>,
}

/// Hierarchy prepared only when topology or the search changes.
struct PortTreeNode {
    id: egui::Id,
    label: String,
    path: String,
    port_count: usize,
    owners: Vec<usize>,
    children: BTreeMap<String, PortTreeNode>,
}

impl Default for PortTreeNode {
    fn default() -> Self {
        Self {
            id: egui::Id::NULL,
            label: String::new(),
            path: String::new(),
            port_count: 0,
            owners: Vec::new(),
            children: BTreeMap::new(),
        }
    }
}

enum PortBrowserRow {
    Branch {
        id: egui::Id,
        title: String,
        path: String,
        depth: usize,
    },
    Port {
        entity_index: usize,
        port_index: usize,
        depth: usize,
    },
}

#[derive(Default)]
struct PortRowIndexCache {
    built: bool,
    rows: Vec<PortBrowserRow>,
}

struct PortDraft {
    value: String,
    validation: Result<f64, String>,
}

fn insert_port_owner(
    root: &mut PortTreeNode,
    scope: egui::Id,
    path: &str,
    owner: usize,
    count: usize,
) {
    let mut node = root;
    let mut prefix = String::new();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        prefix.push('/');
        prefix.push_str(segment);
        node = node
            .children
            .entry(segment.to_owned())
            .or_insert_with(|| PortTreeNode {
                id: scope.with(&prefix),
                label: lunco_core::entity_display_name(
                    Some(&Name::new(segment.to_owned())),
                    None,
                    None,
                ),
                path: prefix.clone(),
                ..default()
            });
        node.port_count += count;
    }
    node.owners.push(owner);
}

impl PortRowIndexCache {
    fn rebuild(&mut self, context: &egui::Context, cache: &PortMatchCache) {
        self.rows.clear();
        for root in cache.roots.values() {
            for child in root.children.values() {
                self.collect(context, child, cache, 0);
            }
        }
        self.built = true;
    }

    fn collect(
        &mut self,
        context: &egui::Context,
        node: &PortTreeNode,
        cache: &PortMatchCache,
        depth: usize,
    ) {
        self.rows.push(PortBrowserRow::Branch {
            id: node.id,
            title: format!("{} ({})", node.label, node.port_count),
            path: node.path.clone(),
            depth,
        });
        let open = egui::collapsing_header::CollapsingState::load_with_default_open(
            context, node.id, false,
        )
        .is_open();
        if !open && cache.filter.is_empty() {
            return;
        }
        for &entity_index in &node.owners {
            for &port_index in &cache.matching_port_indices[entity_index] {
                self.rows.push(PortBrowserRow::Port {
                    entity_index,
                    port_index,
                    depth: depth + 1,
                });
            }
        }
        for child in node.children.values() {
            self.collect(context, child, cache, depth + 1);
        }
    }
}

fn build_port_match_cache(view: &PortView, filter: &str, all_sources: bool) -> PortMatchCache {
    let mut roots = BTreeMap::new();
    let mut matching_port_indices = Vec::with_capacity(view.entities.len());

    for (entity_index, entity) in view.entities.iter().enumerate() {
        let search_label = format!(
            "{} {}",
            entity.label,
            lunco_core::humanize_identifier(&entity.label)
        );
        let port_indices: Vec<usize> = if !all_sources && !entity.scene_source {
            Vec::new()
        } else if filter.is_empty() {
            (0..entity.ports.len()).collect()
        } else {
            entity
                .ports
                .iter()
                .enumerate()
                .filter_map(|(port_index, row)| {
                    port_matches(row, &search_label, &filter).then_some(port_index)
                })
                .collect()
        };
        let matching_count = port_indices.len();
        if matching_count > 0 {
            insert_port_owner(
                roots.entry(entity.hierarchy_scope.value()).or_default(),
                entity.hierarchy_scope,
                &entity.label,
                entity_index,
                matching_count,
            );
        }
        matching_port_indices.push(port_indices);
    }

    let port_count = matching_port_indices.iter().map(Vec::len).sum();
    PortMatchCache {
        port_count,
        topology_revision: view.candidate_topology_revision,
        filter: filter.to_owned(),
        all_sources,
        roots,
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
                display_name: lunco_core::humanize_identifier(&info.name),
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
            row.info.value = registry.read_port_for_handle(
                world,
                row.owner,
                row.entity,
                &row.info.name,
                row.info.direction,
            );
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

fn port_hierarchy_scope(world: &World, entity: Entity, label: &str) -> egui::Id {
    if let Some(prim) = world.get::<lunco_usd_bevy_scene::UsdPrimPath>(entity) {
        egui::Id::new(("port_stage", prim.stage_handle.id()))
    } else if label.starts_with('/') {
        egui::Id::new("port_named_hierarchy")
    } else {
        egui::Id::new(("port_entity", entity))
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
            let label = world
                .get::<lunco_usd_bevy_scene::UsdPrimPath>(entity)
                .map(|prim| prim.path.clone())
                .or_else(|| name.map(|name| name.as_str().to_owned()))
                .or_else(|| global_id.map(|id| format!("Entity {}", id.get())))
                .unwrap_or_else(|| format!("{entity:?}"));
            let scene_source = world
                .get::<lunco_usd_bevy_scene::UsdPrimPath>(entity)
                .is_some_and(|prim| {
                    lunco_usd_bevy_scene::is_mounted_scene_stage(world, prim.stage_handle.id())
                })
                && !lunco_usd_bevy_scene::is_preview_only_entity(world, entity);
            (
                entity,
                label,
                global_id.map(lunco_core::GlobalEntityId::get),
                key,
                scene_source,
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
    for (entity, label, api_id, key, scene_source) in candidates {
        let topology_unchanged = old_topology_keys.get(&entity).copied() == Some(key);
        candidate_topology_keys.insert(entity, key);

        let Some(mut port_entity) = existing_entities.remove(&entity) else {
            let ports = build_port_rows(&registry, world, entity, &wired, &holds);
            if !ports.is_empty() {
                rows.push(PortEntity {
                    entity,
                    hierarchy_scope: port_hierarchy_scope(world, entity, &label),
                    scene_source,
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
        port_entity.hierarchy_scope = port_hierarchy_scope(world, entity, &label);
        port_entity.scene_source = scene_source;
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

#[derive(Resource, Default)]
pub(crate) struct PortPanelState {
    filter: String,
    normalized_filter: String,
    all_sources: bool,
    drafts: HashMap<Entity, HashMap<String, PortDraft>>,
    selected: Option<(Entity, String)>,
    selected_indices: Option<(usize, usize)>,
    matching_cache: Option<PortMatchCache>,
    row_index: PortRowIndexCache,
}

impl lunco_core::SceneResource for PortView {}
impl lunco_core::SceneResource for PortInspectionRequest {}
impl lunco_core::SceneResource for PortPanelState {}

#[derive(Default)]
pub struct PortPanel;

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
        lunco_workbench_core::PanelMenuGroup::Analysis
    }

    fn preferred_perspective(&self) -> Option<lunco_workbench_core::PerspectiveId> {
        Some(lunco_workbench_core::PerspectiveId::new("analysis"))
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
                    let _ = ctx.resource_scope::<PortPanelState, _>(|ctx, state| {
                        state.render_view(ui, ctx, view);
                    });
                });
            })
            .is_none()
        {
            ui.label("Port view is not active.");
        }
    }
}

impl PortPanelState {
    fn render_view(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx, view: &PortView) {
        ui.horizontal(|ui| {
            let width =
                (ui.available_width() - ui.spacing().interact_size.y - ui.spacing().item_spacing.x)
                    .max(0.0);
            let response = ui.add(
                lunco_workbench_widgets::text_editor::singleline(&mut self.filter)
                    .hint_text("Search ports, components, sources…")
                    .desired_width(width),
            );
            if response.changed() {
                self.normalized_filter = self.filter.trim().to_lowercase();
            }
            if ui
                .add_enabled(!self.filter.is_empty(), egui::Button::new("×").small())
                .on_hover_text("Clear search")
                .clicked()
            {
                self.filter.clear();
                self.normalized_filter.clear();
            }
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.all_sources, false, "Scene");
            ui.selectable_value(&mut self.all_sources, true, "All components")
                .on_hover_text("Include editor previews and application components");
        });

        let cache_stale = self.matching_cache.as_ref().is_none_or(|cache| {
            cache.topology_revision != view.candidate_topology_revision
                || cache.filter != self.normalized_filter
                || cache.all_sources != self.all_sources
        });
        if cache_stale {
            self.matching_cache = Some(build_port_match_cache(
                view,
                &self.normalized_filter,
                self.all_sources,
            ));
            self.row_index.built = false;
        }
        let cache = self
            .matching_cache
            .take()
            .expect("port match cache initialized above");
        ui.weak(format!("{} ports", cache.port_count));
        if !self.row_index.built {
            self.row_index.rebuild(ui.ctx(), &cache);
        }
        if cache_stale {
            self.selected_indices = self.selected.as_ref().and_then(|(owner, name)| {
                view.entities
                    .iter()
                    .enumerate()
                    .find(|(_, entity)| entity.entity == *owner)
                    .and_then(|(entity_index, entity)| {
                        entity
                            .ports
                            .iter()
                            .position(|row| row.info.name == *name)
                            .map(|port_index| (entity_index, port_index))
                    })
            });
            if self.selected_indices.is_none() {
                self.selected = None;
            }
        }
        let selected = self.selected_indices.map(|(entity_index, port_index)| {
            let entity = &view.entities[entity_index];
            (entity, &entity.ports[port_index])
        });
        if let Some((entity, row)) = selected {
            let row_height = ui.spacing().interact_size.y;
            egui::Panel::top(ui.id().with("port_details"))
                .resizable(true)
                .default_size(row_height * 16.0)
                .min_size(row_height * 3.0)
                .max_size((ui.available_height() - row_height * 3.0).max(row_height * 3.0))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("port_detail_scroll")
                        .show(ui, |ui| {
                            self.render_details(ui, ctx, entity, row);
                        });
                });
        }
        let rows = std::mem::take(&mut self.row_index.rows);
        let mut requested = HashSet::new();
        let mut changed = false;
        let searching = !cache.filter.is_empty();
        let row_height = ui.spacing().interact_size.y;
        if rows.is_empty() {
            ui.weak(if searching {
                "No matching ports."
            } else if self.all_sources {
                "No ports available."
            } else {
                "No scene ports. Choose All components to browse other sources."
            });
        } else {
            egui::ScrollArea::vertical()
                .id_salt("port_browser")
                .auto_shrink([false; 2])
                .show_rows(ui, row_height, rows.len(), |ui, range| {
                    for index in range {
                        match &rows[index] {
                            PortBrowserRow::Branch {
                                id,
                                title,
                                path,
                                depth,
                            } => {
                                let indent = (ui.spacing().indent * *depth as f32).min(
                                    (ui.available_width() - ui.spacing().interact_size.x).max(0.0),
                                );
                                ui.horizontal(|ui| {
                                    ui.add_space(indent);
                                    ui.vertical(|ui| {
                                        let branch = lunco_workbench_widgets::tree::branch_header(
                                            ui,
                                            if searching { id.with("search") } else { *id },
                                            false,
                                            searching.then_some(true),
                                            |ui| {
                                                lunco_workbench_widgets::tree::label(
                                                    ui,
                                                    egui::RichText::new(title).strong(),
                                                    ui.available_width(),
                                                    egui::Sense::click(),
                                                )
                                                .on_hover_text(path)
                                                .clicked()
                                            },
                                        );
                                        changed |= branch.changed;
                                    });
                                });
                            }
                            PortBrowserRow::Port {
                                entity_index,
                                port_index,
                                depth,
                            } => {
                                let entity = &view.entities[*entity_index];
                                let row = &entity.ports[*port_index];
                                requested.insert(entity.entity);
                                if self.render_port_label(ui, entity, row, *depth) {
                                    self.selected = Some((entity.entity, row.info.name.clone()));
                                    self.selected_indices = Some((*entity_index, *port_index));
                                }
                            }
                        }
                    }
                });
        }
        if let Some((entity, _)) = &self.selected {
            requested.insert(*entity);
        }
        let _ = ctx.resource_scope::<PortInspectionRequest, _>(|_, request| {
            if request.expanded_entities != requested {
                request.expanded_entities = requested;
            }
        });
        self.row_index.rows = rows;
        self.row_index.built &= !changed;
        self.matching_cache = Some(cache);
    }

    fn render_port_label(
        &self,
        ui: &mut egui::Ui,
        entity: &PortEntity,
        row: &PortRow,
        depth: usize,
    ) -> bool {
        let selected = self
            .selected
            .as_ref()
            .is_some_and(|(owner, name)| *owner == row.entity && *name == row.info.name);
        let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
        let (_, rect) = ui.allocate_space(size);
        let mut clicked = false;
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                let value =
                    egui::WidgetText::from(egui::RichText::new(&row.value_text).monospace())
                        .into_galley(
                            ui,
                            Some(egui::TextWrapMode::Extend),
                            f32::INFINITY,
                            egui::TextStyle::Body,
                        );
                if let Some(unit) = row
                    .info
                    .metadata
                    .unit
                    .as_ref()
                    .filter(|unit| unit.id() != "1")
                {
                    let width =
                        (ui.available_width() - value.size().x - ui.spacing().item_spacing.x)
                            .max(0.0);
                    let galley = egui::WidgetText::from(egui::RichText::new(unit.id()).weak())
                        .into_galley(
                            ui,
                            Some(egui::TextWrapMode::Truncate),
                            width,
                            egui::TextStyle::Body,
                        );
                    ui.add(egui::Label::new(galley)).on_hover_text(unit.id());
                }
                ui.add(egui::Label::new(value).extend()).on_hover_text(
                    row.held
                        .or(row.info.value)
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "Unavailable".into()),
                );
                let indent = (ui.spacing().indent * depth as f32)
                    .min((ui.available_width() - ui.spacing().button_padding.x * 2.0).max(0.0));
                let width = (ui.available_width() - indent).max(0.0);
                let response = lunco_workbench_widgets::tree::selectable_label(
                    ui,
                    selected,
                    &row.display_name,
                    width,
                )
                .on_hover_text(format!(
                    "{}\n{} · {}",
                    entity.label,
                    row.info.name,
                    direction_label(row.info.direction)
                ));
                clicked = response.clicked();
            },
        );
        clicked
            || ui
                .interact(
                    rect,
                    ui.id().with((row.entity, &row.info.name)),
                    egui::Sense::click(),
                )
                .clicked()
    }

    fn render_details(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &mut PanelCtx,
        entity: &PortEntity,
        row: &PortRow,
    ) {
        let info = &row.info;
        let theme = lunco_theme::active(ui.ctx());
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        ui.horizontal(|ui| {
            let width =
                (ui.available_width() - ui.spacing().interact_size.x - ui.spacing().item_spacing.x)
                    .max(0.0);
            lunco_workbench_widgets::tree::label(
                ui,
                egui::RichText::new(&row.display_name).strong(),
                width,
                egui::Sense::hover(),
            )
            .on_hover_text(&info.name);
            if ui
                .small_button("×")
                .on_hover_text("Close port details")
                .clicked()
            {
                self.selected = None;
                self.selected_indices = None;
            }
        });
        let owner_name =
            lunco_core::entity_display_name(Some(&Name::new(entity.label.clone())), None, None);
        ui.add(egui::Label::new(egui::RichText::new(owner_name).weak()).truncate())
            .on_hover_text(&entity.label);
        ui.add_space(theme.spacing.item_spacing);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(&row.value_text)
                    .text_style(lunco_theme::TypographyRole::Display.text_style()),
            )
            .on_hover_text(
                row.held
                    .or(info.value)
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "Unavailable".into()),
            );
            if let Some(unit) = &info.metadata.unit {
                ui.weak(if unit.id() == "1" {
                    "Dimensionless"
                } else {
                    unit.id()
                });
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.weak(direction_label(info.direction));
            if !info.metadata.writable {
                ui.weak("Read only");
            }
            if row.wired {
                ui.weak("Connected");
            }
            if row.held.is_some() {
                ui.colored_label(theme.tokens.accent, "Manual override");
            }
        });
        if info.metadata.writable {
            ui.add_space(theme.spacing.item_spacing);
            self.render_controls(ui, ctx, row);
        }
        if let Some(range) = &row.range_text {
            ui.add(egui::Label::new(egui::RichText::new(range).weak()).wrap())
                .on_hover_text(format!(
                    "Minimum: {}\nMaximum: {}",
                    info.metadata
                        .min
                        .map(|value| format!("{value:?}"))
                        .unwrap_or_else(|| "Unbounded".into()),
                    info.metadata
                        .max
                        .map(|value| format!("{value:?}"))
                        .unwrap_or_else(|| "Unbounded".into())
                ));
        }
        ui.add_space(theme.spacing.item_spacing);
        egui::CollapsingHeader::new("Details")
            .id_salt((row.entity, &info.name))
            .show(ui, |ui| {
                for (label, value) in [
                    ("Source", info.metadata.source.as_ref()),
                    ("Authority", info.metadata.authority.as_ref()),
                    ("Type", info.metadata.value_type.as_str()),
                ] {
                    ui.weak(label);
                    ui.add(egui::Label::new(value).wrap());
                }
                if let Some(frame) = &info.metadata.frame {
                    ui.weak("Reference frame");
                    ui.add(egui::Label::new(frame.as_str()).wrap());
                }
                ui.weak("Port address");
                ui.add(egui::Label::new(format!("{} · {}", entity.label, info.name)).wrap());
                if let Some(id) = entity.api_id {
                    ui.weak(format!("Entity {id}"));
                }
            });
    }

    fn render_controls(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx, row: &PortRow) {
        let info = &row.info;
        let theme = lunco_theme::active(ui.ctx());
        let local_origin = ctx
            .resource::<lunco_core_session::LocalSession>()
            .map(|session| lunco_core::CommandOrigin::LocalUser {
                session_id: session.0,
            });
        ui.weak("Set value");
        let entity_drafts = self.drafts.entry(row.entity).or_default();
        if !entity_drafts.contains_key(info.name.as_str()) {
            let value = row
                .held
                .or(info.value)
                .map(|value| value.to_string())
                .unwrap_or_default();
            let validation = validate_port_value(info, &value);
            entity_drafts.insert(info.name.clone(), PortDraft { value, validation });
        }
        let draft = entity_drafts
            .get_mut(info.name.as_str())
            .expect("port draft was inserted above");
        let response = ui.add(
            lunco_workbench_widgets::text_editor::singleline(&mut draft.value)
                .desired_width(ui.available_width()),
        );
        if response.changed() {
            draft.validation = validate_port_value(info, &draft.value);
        }
        ui.horizontal_wrapped(|ui| {
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
                            seq: 0,
                            tick: 0,
                        },
                        origin,
                    );
                }
            }
        });
        if local_origin.is_none() {
            ui.label(
                egui::RichText::new("local control session unavailable").color(theme.tokens.error),
            );
        }
        if let Err(error) = &draft.validation {
            ui.label(egui::RichText::new(error).color(theme.tokens.error));
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
        &row.display_name,
        &row.info.metadata.source,
        &row.info.metadata.authority,
        row.info
            .metadata
            .unit
            .as_ref()
            .map(|unit| unit.id())
            .unwrap_or_default(),
    ]
    .iter()
    .any(|value| value.to_lowercase().contains(filter))
}

fn direction_label(direction: PortDirection) -> &'static str {
    match direction {
        PortDirection::In => "Input",
        PortDirection::Out => "Output",
        PortDirection::InOut => "Bidirectional",
    }
}

fn format_value(value: Option<f64>) -> String {
    match value {
        Some(value) if value.is_finite() => {
            if value != 0.0 && !(0.0001..1_000_000.0).contains(&value.abs()) {
                format!("{value:.4e}")
            } else {
                format!("{value:.4}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_owned()
            }
        }
        Some(_) => "invalid".into(),
        None => "unavailable".into(),
    }
}

fn display_value(held: Option<f64>, value: Option<f64>) -> String {
    format_value(held.or(value))
}

fn validate_port_value(info: &PortInfo, value: &str) -> Result<f64, String> {
    value
        .parse::<f64>()
        .map_err(|_| "enter a number".to_owned())
        .and_then(|value| info.metadata.validate(value).map(|()| value))
}

fn range_label(metadata: &PortMetadata) -> Option<String> {
    match (metadata.min, metadata.max) {
        (Some(min), Some(max)) => Some(format!(
            "Range {} … {}",
            format_value(Some(min)),
            format_value(Some(max))
        )),
        (Some(min), None) => Some(format!("Minimum {}", format_value(Some(min)))),
        (None, Some(max)) => Some(format!("Maximum {}", format_value(Some(max)))),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_numbers_stay_compact_without_changing_the_value() {
        let bound = f32::MAX as f64;
        assert_eq!(format_value(Some(bound)), "3.4028e38");
        assert_eq!(format_value(Some(-bound)), "-3.4028e38");
        assert_eq!(format_value(Some(0.0)), "0");
        assert_eq!(format_value(Some(0.125)), "0.125");
        assert_eq!(format_value(Some(0.00001)), "1.0000e-5");
        assert_eq!(format_value(Some(f64::INFINITY)), "invalid");
        assert_eq!(format_value(None), "unavailable");
        let metadata = PortMetadata::scalar(
            PortDirection::In,
            None,
            Some(-bound),
            Some(bound),
            "property",
            "author",
            true,
            None,
        );
        assert_eq!(
            range_label(&metadata).as_deref(),
            Some("Range -3.4028e38 … 3.4028e38")
        );
        assert_eq!(metadata.max, Some(bound));
    }

    #[test]
    fn port_hierarchy_keeps_parent_owners_and_distinct_leaf_identities() {
        let mut root = PortTreeNode::default();
        let scope = egui::Id::new("stage");
        for (index, path) in ["/Plant", "/Plant/Pump", "/Plant/Valve", "/Plant/Pump"]
            .into_iter()
            .enumerate()
        {
            insert_port_owner(&mut root, scope, path, index, 2);
        }
        let plant = root.children.get("Plant").unwrap();
        assert_eq!(plant.label, "Plant");
        assert_eq!(plant.port_count, 8);
        assert_eq!(plant.owners, vec![0]);
        assert_eq!(plant.children["Pump"].owners, vec![1, 3]);
        assert_eq!(plant.children["Valve"].port_count, 2);
        assert_ne!(plant.id, plant.children["Pump"].id);

        let mut other = PortTreeNode::default();
        insert_port_owner(&mut other, egui::Id::new("other stage"), "/Plant", 0, 1);
        assert_ne!(plant.id, other.children["Plant"].id);
        assert!(PortTreeNode::default().children.is_empty());

        let plant_id = plant.id;
        let context = egui::Context::default();
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            &context, plant_id, true,
        );
        state.set_open(false);
        state.store(&context);
        let mut cache = PortMatchCache {
            roots: BTreeMap::from([(scope.value(), root)]),
            matching_port_indices: vec![vec![0, 1]; 4],
            ..default()
        };
        let mut index = PortRowIndexCache::default();
        index.rebuild(&context, &cache);
        assert_eq!(
            index.rows.len(),
            1,
            "a folded ancestor hides its complete subtree"
        );
        cache.filter = "pump".into();
        index.rebuild(&context, &cache);
        assert_eq!(
            index
                .rows
                .iter()
                .filter(|row| matches!(row, PortBrowserRow::Port { .. }))
                .count(),
            8
        );
        cache.filter.clear();
        index.rebuild(&context, &cache);
        assert_eq!(
            index.rows.len(),
            1,
            "search preserves ordinary disclosure state"
        );
    }

    #[test]
    fn filter_matches_metadata_as_well_as_port_name() {
        let row = PortRow {
            entity: Entity::PLACEHOLDER,
            owner: PortHandle::default(),
            info: PortInfo {
                name: "throttle".into(),
                direction: PortDirection::In,
                value: Some(0.0),
                metadata: PortMetadata::scalar(
                    PortDirection::In,
                    None,
                    Some(-1.0),
                    Some(1.0),
                    "rover controller",
                    "operator",
                    true,
                    None,
                ),
            },
            wired: false,
            held: None,
            display_name: "Throttle".into(),
            value_text: "0".into(),
            range_text: Some("Range -1 … 1".into()),
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
                value: Some(0.0),
                metadata: PortMetadata::scalar(
                    PortDirection::In,
                    None,
                    Some(-1.0),
                    Some(1.0),
                    "rover controller",
                    "operator",
                    true,
                    None,
                ),
            },
            wired: false,
            held: None,
            display_name: "Throttle".into(),
            value_text: "0".into(),
            range_text: Some("Range -1 … 1".into()),
        };
        let view = PortView {
            entities: vec![PortEntity {
                entity: Entity::PLACEHOLDER,
                label: "Rover".into(),
                api_id: None,
                hierarchy_scope: egui::Id::new("test"),
                scene_source: false,
                ports: vec![row],
            }],
            candidate_topology_revision: Some(7),
            ..default()
        };

        let cache = build_port_match_cache(&view, "controller", true);
        assert_eq!(cache.topology_revision, Some(7));
        assert_eq!(cache.roots.len(), 1);
        assert_eq!(cache.matching_port_indices, vec![vec![0]]);

        assert!(
            build_port_match_cache(&view, "", false).roots.is_empty(),
            "scene browsing excludes application/preview-only sources"
        );
        let cache = build_port_match_cache(&view, "lander", true);
        assert!(cache.roots.is_empty());
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
            None,
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
        assert!(
            registry
                .write_port(&mut world, owned, "throttle", 0.75)
                .is_ok()
        );
        populate_port_view(&mut world);
        assert_eq!(
            world.resource::<PortView>().entities[0].ports[0].info.value,
            Some(0.0)
        );

        world
            .resource_mut::<PortInspectionRequest>()
            .expanded_entities
            .insert(owned);
        populate_port_view(&mut world);
        assert_eq!(
            world.resource::<PortView>().entities[0].ports[0].info.value,
            Some(0.75)
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
