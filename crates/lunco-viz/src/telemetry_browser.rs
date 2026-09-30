//! Telemetry channel browser with a persistent incremental presentation index.
//!
//! Lists every scalar channel in the [`SignalRegistry`], grouped by the
//! subsystem it serves, with unit (from [`crate::signal::SignalMeta`]) and
//! live latest value. A filter box narrows the list; clicking a row
//! shows a detail strip with the latest value and a small inline
//! preview plot (reusing the min-max decimation path in
//! [`crate::plot_fmt`]).
//!
//! ## Change-driven list
//!
//! The grouped/sorted catalog is **not** rebuilt every frame. The
//! registry publishes coalesced descriptor changes. Bounded worker batches
//! prepare those descriptors, then patch the affected tree paths and alias groups.
//! Selection filters the persistent index; samples do not enqueue patches.
//! Latest-value cells are
//! O(1) `samples.back()` reads per visible row.
//!
//! ## Scoping to the selection
//!
//! A channel is owned by the prim it measures — a motor, a battery, a wheel —
//! while the user selects a VESSEL. "Selected only" therefore filters by an
//! ANCESTOR test against [`lunco_signal::TelemetryFocus`]
//! ([`entity_in_focus`]), not by entity equality, which would show nothing for
//! every rover ever selected. The focus resource is written by whichever app owns
//! selection (`lunco-scene-selection` mirrors `SelectedEntities` into it); a host
//! without one leaves it empty and the toggle disabled.
//!
//! ## Getting a channel onto a canvas
//!
//! Two doors, both landing on the existing dirty-checked plot node
//! substrate ([`crate::kinds::canvas_plot_node`]):
//!
//! * **Drag** — every row is an egui `dnd_drag_source` carrying a
//!   [`ChannelDragPayload`]. A canvas host accepts it via
//!   `response.dnd_release_payload::<ChannelDragPayload>()` and
//!   inserts the node [`plot_node_at`] builds (kind =
//!   [`crate::kinds::canvas_plot_node::PLOT_NODE_KIND`], payload =
//!   `PlotNodeData` with `PlotBinding::Pinned`).
//! * **Double-click** — queues a [`PlotDropRequest`] with no position
//!   into the egui context ([`queue_plot_drop`]); the canvas host
//!   drains it once per frame ([`drain_plot_drops`]) and places the
//!   node at a default position. Same pattern as
//!   `canvas_plot_node::drain_input_writes`.
//!
//! The browser additionally offers "Open as plot tab" on the selected
//! channel — that path is entirely in-crate (insert a
//! `VisualizationConfig`, fire `OpenTab { VIZ_PANEL_KIND }`) and works
//! without any canvas host wiring.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use egui;
use egui_plot::{Line, Plot, PlotPoints};
use lunco_core::{Command, on_command, register_commands};
use lunco_settings::SettingsSection;
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_workbench_core::commands::OpenTab;
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelMenuGroup, PanelSlot};

use crate::kinds::canvas_plot_node::{PLOT_NODE_KIND, PlotBinding, PlotNodeData};
use crate::registry::VisualizationRegistry;
use crate::signal::{
    ScalarHistory, SignalExposure, SignalPresentation, SignalRef, SignalRegistry, TelemetryFocus,
    display_channel_label, humanize_identifier, operator_identifier_label,
};
use crate::view::ViewTarget;
use crate::viz::{SignalBinding, VisualizationConfig};
use crate::{LINE_PLOT_KIND, VIZ_PANEL_KIND};
use lunco_viz_core::VizId;

/// Panel id — new id, not the deleted stub's `"telemetry"`, so stale
/// saved layouts referencing the tombstone don't resurrect over us.
pub const TELEMETRY_BROWSER_PANEL_ID: PanelId = PanelId("telemetry_browser");

/// Runtime-authored view intent for the telemetry browser.
///
/// The browser remains the owner of its egui-local state; commands publish a
/// typed request here and the panel consumes it on its next render. This keeps
/// HTTP/Rhai automation independent of a concrete dock layout or panel object.
#[derive(Resource, Clone, Debug, Default)]
pub struct TelemetryBrowserView {
    pub filter: String,
    pub signal: String,
}

/// Presentation-only preferences for the telemetry browser.
///
/// These settings never change samples, deadbands, units, or plot data. They
/// control only the telemetry browser presentation, so the operator can
/// suppress numerical noise or historical rows without losing the underlying
/// state in the shared registry or API.
#[derive(Resource, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TelemetryDisplaySettings {
    /// Number of significant digits used in a compact value cell.
    pub significant_digits: u8,
    /// Values below this magnitude are rendered as `0` in the compact cell.
    /// The stored sample and detail/plot history remain unchanged.
    pub zero_threshold: f64,
    /// Use the exact producer-generated path in operator-facing labels. This
    /// is a diagnostic presentation choice; signal identity and data remain
    /// unchanged.
    #[serde(default)]
    pub show_generated_names: bool,
    /// Include retained channels whose publisher no longer exists.  Current
    /// scene telemetry stays live-only by default; history remains available
    /// through the explicit telemetry/API history surfaces.
    #[serde(default)]
    pub show_archived: bool,
}

impl Default for TelemetryDisplaySettings {
    fn default() -> Self {
        Self {
            significant_digits: 4,
            zero_threshold: 1.0e-4,
            show_generated_names: false,
            show_archived: false,
        }
    }
}

impl SettingsSection for TelemetryDisplaySettings {
    const KEY: &'static str = "telemetry_display";
}

/// Select the telemetry browser's signal filter and focused signal.
#[Command(default)]
pub struct SetTelemetryBrowserView {
    pub filter: String,
    pub signal: String,
}

#[on_command(SetTelemetryBrowserView)]
fn on_set_telemetry_browser_view(
    trigger: On<SetTelemetryBrowserView>,
    mut view: ResMut<TelemetryBrowserView>,
) {
    let request = trigger.event();
    view.filter = request.filter.clone();
    view.signal = request.signal.clone();
}

register_commands!(on_set_telemetry_browser_view);

/// Insert a newly configured visualization after the browser has finished
/// painting. The panel emits the domain operation; the observer owns the
/// registry mutation.
#[derive(Event)]
pub(crate) struct OpenVisualizationRequested {
    pub(crate) config: VisualizationConfig,
}

pub(crate) fn on_open_visualization_requested(
    trigger: On<OpenVisualizationRequested>,
    mut registry: bevy::prelude::ResMut<VisualizationRegistry>,
) {
    registry.insert(trigger.config.clone());
}

// ── Drag payload + canvas-host doors ─────────────────────────────────

/// What a browser row drags: enough to mint a `PlotNodeData` with a
/// `PlotBinding::Pinned` binding on the drop side. Kept plain-data
/// (entity bits, not `Entity`) to mirror `PlotBinding`'s own encoding.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChannelDragPayload {
    /// `Entity::to_bits()` of the channel's owning entity.
    pub entity_bits: u64,
    /// Shared immutable signal path, e.g. `"P.y"`.
    pub path: Arc<str>,
}

impl ChannelDragPayload {
    pub fn from_signal(sig: &SignalRef) -> Self {
        Self {
            entity_bits: sig.entity.to_bits(),
            path: Arc::from(sig.path.as_str()),
        }
    }
}

/// A queued "make a plot node for this channel" request. `world_pos =
/// None` means "host picks a default position" (the double-click
/// path); `Some` carries a canvas world position (a host may also
/// queue drops itself, e.g. from a context menu).
#[derive(Clone, Debug, PartialEq)]
pub struct PlotDropRequest {
    pub payload: ChannelDragPayload,
    pub world_pos: Option<[f32; 2]>,
}

type DropQueue = Arc<std::sync::Mutex<Vec<PlotDropRequest>>>;

fn drop_queue_id() -> egui::Id {
    egui::Id::new("lunco_viz_telemetry_plot_drops")
}

fn drop_queue(ctx: &egui::Context) -> DropQueue {
    ctx.data_mut(|d| {
        if let Some(existing) = d.get_temp(drop_queue_id()) {
            existing
        } else {
            let fresh: DropQueue = Default::default();
            d.insert_temp(drop_queue_id(), fresh.clone());
            fresh
        }
    })
}

/// Queue a plot-node creation request (browser double-click, or any
/// other UI that wants a channel plotted on the active canvas).
pub fn queue_plot_drop(ctx: &egui::Context, req: PlotDropRequest) {
    if let Ok(mut q) = drop_queue(ctx).lock() {
        q.push(req);
    }
}

/// Drain pending plot-drop requests. Called by the canvas host once
/// per frame; it inserts a node per request into its scene (see
/// [`plot_node_at`]). Draining semantics: requests queued while no
/// canvas is open stay queued until one drains them.
pub fn drain_plot_drops(ctx: &egui::Context) -> Vec<PlotDropRequest> {
    drop_queue(ctx)
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

/// Build the canvas `Node` for a dropped channel — the same shape the
/// Modelica canvas's own "add plot" menu door builds, so the node goes
/// through the existing dirty-checked plot substrate (`PLOT_NODE_KIND`
/// visual, `SignalInterest` registration, snapshot producer).
///
/// The caller allocates the id (`scene.alloc_node_id()`) and inserts
/// the returned node (`scene.insert_node(..)`).
pub fn plot_node_at(
    id: lunco_canvas::scene::NodeId,
    at: lunco_canvas::Pos,
    payload: &ChannelDragPayload,
) -> lunco_canvas::scene::Node {
    let data: lunco_canvas::NodeData = Arc::new(PlotNodeData {
        binding: PlotBinding::Pinned {
            entity: payload.entity_bits,
        },
        signal_path: payload.path.to_string(),
        title: String::new(),
    });
    lunco_canvas::scene::Node {
        id,
        // Same default extent as the canvas context-menu door.
        rect: lunco_canvas::Rect::from_min_max(
            at,
            lunco_canvas::Pos::new(at.x + 60.0, at.y + 40.0),
        ),
        kind: PLOT_NODE_KIND.into(),
        data,
        ports: Vec::new(),
        label: String::new(),
        origin: None,
        resizable: true,
        visual_rect: None,
    }
}

/// Bind a telemetry drag payload to an existing visualization.
pub fn bind_dropped_channel(
    registry: &mut VisualizationRegistry,
    viz_id: VizId,
    payload: &ChannelDragPayload,
) -> bool {
    let source = SignalRef::new(
        Entity::from_bits(payload.entity_bits),
        payload.path.to_string(),
    );
    let Some(config) = registry.get_mut(viz_id) else {
        return false;
    };
    if config.inputs.iter().any(|binding| binding.source == source) {
        return false;
    }
    config.inputs.push(SignalBinding::live(source, "y"));
    true
}

// ── Cached catalog ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Row {
    sig: SignalRef,
    /// Reused by visible rows so each repaint only clones a shared path handle.
    drag_payload: ChannelDragPayload,
    unit: Option<String>,
    description: Option<String>,
    provenance: Option<String>,
    group_path: Option<String>,
    model_class: Option<String>,
    model_variable: Option<String>,
    source_asset: Option<String>,
    canonical_name: Option<String>,
    presentation: SignalPresentation,
    exposure: SignalExposure,
    active: bool,
    /// Lowercased values keep runtime filter matching allocation-free during
    /// repaint; a descriptor patch refreshes them only for its changed row.
    search_fields: [String; 5],
}

#[derive(Debug)]
struct TreeNode {
    label: Arc<str>,
    filter_label: String,
    id: String,
    children: std::collections::BTreeMap<String, TreeNode>,
    rows: Vec<Arc<Row>>,
}

impl TreeNode {
    fn new(id: String, label: String) -> Self {
        let filter_label = label.to_lowercase();
        Self {
            label: Arc::from(label),
            filter_label,
            id,
            children: Default::default(),
            rows: Vec::new(),
        }
    }
}

/// Authored identity of one Modelica state as it should appear in the operator
/// tree.  Generated aliases and unit-qualified solver variables can have
/// different signal paths while still describing the same `(component, class,
/// variable)` value.  The signal registry keeps every path; the browser chooses
/// one presentation row from this identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ModelStateIdentity {
    entity: Entity,
    group_path: String,
    model_class: String,
    model_variable: String,
}

fn model_state_identity(row: &Row) -> Option<ModelStateIdentity> {
    Some(ModelStateIdentity {
        entity: row.sig.entity,
        group_path: row.group_path.clone()?,
        model_class: row.model_class.clone()?,
        model_variable: row.model_variable.clone()?,
    })
}

/// Prefer a live channel, then the public channel, then the exact canonical
/// authored channel. A
/// generated wrapper may expose one authored value through several public or
/// internal paths; only the path named by `canonical_name` is the
/// operator-facing address. This is presentation selection only: all other
/// signal identities remain in the registry and remain valid plot/API sources.
fn model_state_priority(row: &Row) -> (u8, u8, u8) {
    (
        (row.active) as u8,
        (row.exposure == SignalExposure::Public) as u8,
        (row.canonical_name.as_deref() == Some(row.sig.path.as_str())) as u8,
    )
}

#[cfg(test)]
fn snapshot_rows(reg: &SignalRegistry) -> Vec<Row> {
    reg.iter_scalar()
        .map(|(sig, _history)| {
            let meta = reg.meta(sig);
            Row {
                sig: sig.clone(),
                drag_payload: ChannelDragPayload::from_signal(sig),
                unit: meta.and_then(|m| m.unit.clone()),
                description: meta.and_then(|m| m.description.clone()),
                provenance: meta.and_then(|m| m.provenance.clone()),
                group_path: meta.and_then(|m| m.group_path.clone()),
                model_class: meta.and_then(|m| m.model_class.clone()),
                model_variable: meta.and_then(|m| m.model_variable.clone()),
                source_asset: meta.and_then(|m| m.source_asset.clone()),
                canonical_name: meta.and_then(|m| m.canonical_name.clone()),
                presentation: meta.map(|m| m.presentation.clone()).unwrap_or_default(),
                exposure: meta.map_or(SignalExposure::Public, |m| m.exposure),

                active: reg.is_active(sig),
                search_fields: Default::default(),
            }
        })
        .collect()
}

#[cfg(test)]
fn deduplicated_rows(reg: &SignalRegistry) -> Vec<Row> {
    let mut catalog = Catalog::default();
    for row in snapshot_rows(reg) {
        let prepared = prepare_telemetry_row(row, |_| None, |_| None, |_| None, |_| false);
        catalog.apply(prepared.row.sig.clone(), Some(prepared));
    }
    let mut rows: Vec<_> = catalog
        .displayed
        .keys()
        .map(|signal| catalog.entries[signal].row.as_ref().clone())
        .collect();
    rows.sort_by(|a, b| {
        a.sig
            .entity
            .to_bits()
            .cmp(&b.sig.entity.to_bits())
            .then(a.sig.path.cmp(&b.sig.path))
    });
    rows
}

struct Catalog {
    key: u64,
    root: TreeNode,
    entries: HashMap<SignalRef, PreparedTelemetryRow>,
    aliases: HashMap<ModelStateIdentity, HashSet<SignalRef>>,
    winners: HashMap<ModelStateIdentity, SignalRef>,
    displayed: HashMap<SignalRef, Vec<String>>,
    ancestor_signals: HashMap<Entity, HashSet<SignalRef>>,
    owner_signals: HashMap<Entity, HashSet<SignalRef>>,
    facts: HashMap<Entity, EntityCatalogFacts>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            key: 0,
            root: TreeNode::new("root".to_string(), "Telemetry".to_string()),
            entries: Default::default(),
            aliases: Default::default(),
            winners: Default::default(),
            displayed: Default::default(),
            ancestor_signals: Default::default(),
            owner_signals: Default::default(),
            facts: Default::default(),
        }
    }
}

fn insert_prepared_row(root: &mut TreeNode, prepared: &PreparedTelemetryRow) {
    let mut node = root;
    for (id, label) in &prepared.lineage {
        node = node
            .children
            .entry(id.clone())
            .or_insert_with(|| TreeNode::new(id.clone(), label.clone()));
        if node.label.as_ref() != label {
            node.label = Arc::from(label.as_str());
            node.filter_label = label.to_lowercase();
        }
    }
    match node.rows.binary_search_by(|row| {
        row.sig.path.cmp(&prepared.row.sig.path).then(
            row.sig
                .entity
                .to_bits()
                .cmp(&prepared.row.sig.entity.to_bits()),
        )
    }) {
        Ok(index) => node.rows[index] = Arc::clone(&prepared.row),
        Err(index) => node.rows.insert(index, Arc::clone(&prepared.row)),
    }
}

fn remove_tree_row(node: &mut TreeNode, path: &[String], signal: &SignalRef) {
    if let Some((first, rest)) = path.split_first() {
        if let Some(child) = node.children.get_mut(first) {
            remove_tree_row(child, rest, signal);
            if child.children.is_empty() && child.rows.is_empty() {
                node.children.remove(first);
            }
        }
    } else {
        if let Ok(index) = node.rows.binary_search_by(|row| {
            row.sig
                .path
                .cmp(&signal.path)
                .then(row.sig.entity.to_bits().cmp(&signal.entity.to_bits()))
        }) {
            node.rows.remove(index);
        }
    }
}

impl Catalog {
    fn focused_owners(
        &self,
        roots: &[Entity],
        root_path: impl Fn(Entity) -> Option<String>,
    ) -> HashSet<Entity> {
        let paths: Vec<_> = roots.iter().filter_map(|root| root_path(*root)).collect();
        self.owner_signals
            .keys()
            .copied()
            .filter(|owner| {
                if *owner == Entity::PLACEHOLDER {
                    return false;
                }
                if let Some(path) = self
                    .facts
                    .get(owner)
                    .and_then(|fact| fact.usd_path.as_deref())
                {
                    paths.iter().any(|root| {
                        path == root
                            || path
                                .strip_prefix(root)
                                .is_some_and(|suffix| suffix.starts_with('/'))
                    })
                } else {
                    entity_in_focus(*owner, roots, |entity| {
                        self.facts.get(&entity).and_then(|fact| fact.parent)
                    })
                }
            })
            .collect()
    }

    fn remove_displayed(&mut self, signal: &SignalRef) {
        if let Some(path) = self.displayed.remove(signal) {
            remove_tree_row(&mut self.root, &path, signal);
        }
    }

    fn display(&mut self, signal: &SignalRef) {
        let prepared = self
            .entries
            .get(signal)
            .expect("displayed channel has a prepared descriptor");
        let path: Vec<_> = prepared.lineage.iter().map(|(id, _)| id.clone()).collect();
        if self.displayed.get(signal).is_some_and(|old| old != &path) {
            self.remove_displayed(signal);
        }
        let prepared = &self.entries[signal];
        insert_prepared_row(&mut self.root, prepared);
        self.displayed.insert(signal.clone(), path);
    }

    /// Only changed channels and their old/new alias groups are reconciled.
    fn apply(&mut self, signal: SignalRef, prepared: Option<PreparedTelemetryRow>) {
        let mut affected = HashSet::new();
        let mut previous_ancestors = Vec::new();
        if let Some(previous) = self.entries.remove(&signal) {
            if let Some(identity) = model_state_identity(&previous.row) {
                if let Some(members) = self.aliases.get_mut(&identity) {
                    members.remove(&signal);
                }
                affected.insert(identity);
            }
            previous_ancestors = previous.ancestors;
            for ancestor in &previous_ancestors {
                if let Some(channels) = self.ancestor_signals.get_mut(ancestor) {
                    channels.remove(&signal);
                    if channels.is_empty() {
                        self.ancestor_signals.remove(ancestor);
                    }
                }
            }
            if let Some(channels) = self.owner_signals.get_mut(&signal.entity) {
                channels.remove(&signal);
                if channels.is_empty() {
                    self.owner_signals.remove(&signal.entity);
                }
            }
        }
        let mut standalone = false;
        if let Some(prepared) = prepared {
            for ancestor in &prepared.ancestors {
                self.ancestor_signals
                    .entry(*ancestor)
                    .or_default()
                    .insert(signal.clone());
            }
            self.owner_signals
                .entry(signal.entity)
                .or_default()
                .insert(signal.clone());
            if let Some(identity) = model_state_identity(&prepared.row) {
                self.aliases
                    .entry(identity.clone())
                    .or_default()
                    .insert(signal.clone());
                affected.insert(identity);
            } else {
                standalone = true;
            }
            self.entries.insert(signal.clone(), prepared);
        }
        // Deterministic tie-breaking uses exact signal path after producer priority.
        let mut affected: Vec<_> = affected.into_iter().collect();
        affected.sort_by(|a, b| {
            a.entity
                .to_bits()
                .cmp(&b.entity.to_bits())
                .then(a.group_path.cmp(&b.group_path))
                .then(a.model_class.cmp(&b.model_class))
                .then(a.model_variable.cmp(&b.model_variable))
        });
        let mut selected = standalone;
        for identity in affected {
            let chosen = self
                .aliases
                .get(&identity)
                .into_iter()
                .flatten()
                .filter_map(|sig| self.entries.get(sig))
                .max_by(|a, b| {
                    model_state_priority(&a.row)
                        .cmp(&model_state_priority(&b.row))
                        .then_with(|| b.row.sig.path.cmp(&a.row.sig.path))
                })
                .map(|prepared| prepared.row.sig.clone());
            let previous = self.winners.remove(&identity);
            if let Some(previous) = previous.filter(|previous| Some(previous) != chosen.as_ref()) {
                self.remove_displayed(&previous);
            }
            if let Some(chosen) = chosen {
                selected |= chosen == signal;
                self.display(&chosen);
                self.winners.insert(identity, chosen);
            } else {
                self.aliases.remove(&identity);
            }
        }
        if standalone {
            self.display(&signal);
        } else if !selected {
            self.remove_displayed(&signal);
        }
        for ancestor in previous_ancestors {
            if !self.ancestor_signals.contains_key(&ancestor) {
                self.facts.remove(&ancestor);
            }
        }
        self.key = self.key.wrapping_add(1);
    }
}

/// Registry revision for resolving an explicit channel-selection request.
/// Samples leave this lookup cache valid; presentation caches use `Catalog::key`.
fn catalog_key(reg: &SignalRegistry) -> u64 {
    reg.catalog_revision()
}

/// Humanize a path segment through the shared entity-label policy. Its complete
/// path remains the tree key and tooltip identity; the browser uses this label
/// only to keep the live hierarchy readable in a narrow panel.
fn display_path_segment(segment: &str) -> String {
    let name = Name::new(segment.to_owned());
    lunco_core::entity_display_name(Some(&name), None, None)
}

fn authored_path_lineage(path: &str, leaf_label: Option<&str>) -> Vec<(String, String)> {
    let mut key = String::new();
    let segments: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let segment_count = segments.len();
    segments
        .into_iter()
        .enumerate()
        .map(|(index, segment)| {
            key.push('/');
            key.push_str(segment);
            let label = (index + 1 == segment_count)
                .then_some(leaf_label)
                .flatten()
                .filter(|label| !label.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| display_path_segment(segment));
            (key.clone(), label)
        })
        .collect()
}

/// Prepare one descriptor from the live ownership hierarchy. This deliberately
/// has no `wheel`, `motor`, `beam`, or other name-based classifier: the USD
/// parent graph supplies the assembly, subsystem, and component grouping for
/// every scene, including ones the editor has never seen before.
struct PreparedTelemetryRow {
    row: Arc<Row>,
    lineage: Vec<(String, String)>,
    ancestors: Vec<Entity>,
}

fn prepare_telemetry_row(
    mut row: Row,
    label_of: impl Fn(Entity) -> Option<String>,
    parent_of: impl Fn(Entity) -> Option<Entity>,
    usd_path_of: impl Fn(Entity) -> Option<String>,
    is_navigation_root: impl Fn(Entity) -> bool,
) -> PreparedTelemetryRow {
    let sig = row.sig.clone();
    row.search_fields = normalized_search_fields(
        &row.sig.path,
        row.description.as_deref(),
        row.model_class.as_deref(),
        row.model_variable.as_deref(),
        row.source_asset.as_deref(),
    );

    let group_path = row.group_path.as_deref().filter(|path| !path.is_empty());
    let mut lineage: Vec<(String, String)> =
        if let Some(path) = group_path.filter(|path| path.trim_start().starts_with('/')) {
            // Authored ownership is the canonical hierarchy for all
            // producers. This merges physical readback and Modelica channels
            // without coupling either producer to the other.
            authored_path_lineage(path, None)
        } else if let Some(path) = usd_path_of(sig.entity) {
            let label = label_of(sig.entity);
            authored_path_lineage(&path, label.as_deref())
        } else {
            let mut entities = Vec::new();
            let mut cursor = Some(sig.entity);
            for _ in 0..MAX_ANCESTOR_DEPTH {
                let Some(entity) = cursor else { break };
                if entities.contains(&entity) {
                    break;
                }
                entities.push(entity);
                if is_navigation_root(entity) {
                    break;
                }
                cursor = (entity != Entity::PLACEHOLDER)
                    .then(|| parent_of(entity))
                    .flatten();
            }
            entities.reverse();
            entities
                .into_iter()
                .map(|entity| {
                    let label = if entity == Entity::PLACEHOLDER {
                        "Global".to_string()
                    } else {
                        label_of(entity).unwrap_or_else(|| "Unnamed entity".to_string())
                    };
                    (format!("entity:{}", entity.to_bits()), label)
                })
                .collect()
        };
    // A producer-owned semantic presentation group is a value namespace,
    // not another entity in the ownership hierarchy. It therefore gets one
    // explicit child below the owner and its rows retain their exact signal
    // identities. Scalar channels keep the authored Modelica namespace or
    // ordinary producer path structure.
    let mut structure = if let Some(group) = presentation_group(&row.presentation) {
        vec![(format!("signal-group:{group}"), humanize_identifier(group))]
    } else {
        let structure_path = group_path
            .and(row.model_variable.as_deref())
            .unwrap_or(&sig.path);
        signal_structure(structure_path)
    };
    // Canonical authored paths may repeat the USD ancestry already
    // represented by the entity lineage. Remove the complete shared
    // prefix, then make the remaining nodes relative to their owner.
    let shared_prefix_len = structure
        .iter()
        .zip(&lineage)
        .take_while(|((structure_id, _), (lineage_id, _))| structure_id == lineage_id)
        .count();
    if shared_prefix_len > 0 {
        structure.drain(..shared_prefix_len);
        for (id, _) in &mut structure {
            if id.starts_with('/') {
                if let Some(segment) = id.rsplit('/').find(|segment| !segment.is_empty()) {
                    *id = format!("signal-structure:{segment}");
                }
            }
        }
    }
    if group_path.is_some_and(|path| path.trim_start().starts_with('/')) {
        // `signal_structure` returns relative IDs for Modelica variables;
        // keep that distinction explicit even if a future producer emits
        // an absolute variable spelling.
        for (id, _) in &mut structure {
            if id.starts_with('/') {
                if let Some(segment) = id.rsplit('/').find(|s| !s.is_empty()) {
                    *id = format!("signal-structure:{segment}");
                }
            }
        }
    }
    lineage.extend(structure);
    let mut ancestors = Vec::new();
    let mut cursor = Some(sig.entity);
    for _ in 0..MAX_ANCESTOR_DEPTH {
        let Some(entity) = cursor else { break };
        if ancestors.contains(&entity) {
            break;
        }
        ancestors.push(entity);
        cursor = parent_of(entity);
    }
    PreparedTelemetryRow {
        row: Arc::new(row),
        lineage,
        ancestors,
    }
}

#[cfg(test)]
fn build_tree_rows(
    rows: Vec<Row>,
    label_of: impl Fn(Entity) -> Option<String>,
    parent_of: impl Fn(Entity) -> Option<Entity>,
    usd_path_of: impl Fn(Entity) -> Option<String>,
    is_navigation_root: impl Fn(Entity) -> bool,
) -> TreeNode {
    let mut catalog = Catalog::default();
    for row in rows {
        let prepared = prepare_telemetry_row(
            row,
            &label_of,
            &parent_of,
            &usd_path_of,
            &is_navigation_root,
        );
        catalog.apply(prepared.row.sig.clone(), Some(prepared));
    }
    catalog.root
}

#[cfg(test)]
fn build_tree(
    reg: &SignalRegistry,
    label_of: impl Fn(Entity) -> Option<String>,
    parent_of: impl Fn(Entity) -> Option<Entity>,
    usd_path_of: impl Fn(Entity) -> Option<String>,
    is_navigation_root: impl Fn(Entity) -> bool,
) -> TreeNode {
    build_tree_rows(
        snapshot_rows(reg),
        label_of,
        parent_of,
        usd_path_of,
        is_navigation_root,
    )
}

const MAX_ANCESTOR_DEPTH: usize = 32;
const TELEMETRY_CATALOG_ROWS_PER_UPDATE: usize = 64;

#[derive(Default, Clone, PartialEq, Eq)]
struct EntityCatalogFacts {
    label: Option<String>,
    parent: Option<Entity>,
    usd_path: Option<String>,
}

struct PreparedTelemetryBatch {
    rows: Vec<(SignalRef, u64, Option<PreparedTelemetryRow>)>,
    facts: HashMap<Entity, EntityCatalogFacts>,
    worker_ms: f64,
}

#[derive(Default)]
struct CatalogMetrics {
    initial_scans: u64,
    descriptor_notifications: u64,
    hierarchy_invalidations: u64,
    prepared_channels: u64,
    committed_channels: u64,
    superseded_channels: u64,
    last_batch_channels: usize,
    last_capture_ms: f64,
    max_capture_ms: f64,
    last_worker_ms: f64,
    max_worker_ms: f64,
    last_commit_ms: f64,
    max_commit_ms: f64,
}

#[derive(Resource, Default)]
pub(crate) struct TelemetryCatalogBuildState {
    catalog: Catalog,
    initialized: bool,
    pending: VecDeque<SignalRef>,
    queued: HashSet<SignalRef>,
    versions: HashMap<SignalRef, u64>,
    in_flight_ancestors: HashMap<Entity, HashSet<SignalRef>>,
    in_flight_facts: HashMap<Entity, EntityCatalogFacts>,
    sequence: u64,
    task: Option<Task<PreparedTelemetryBatch>>,
    metrics: CatalogMetrics,
}

impl TelemetryCatalogBuildState {
    fn enqueue(&mut self, signal: SignalRef) {
        self.sequence = self.sequence.wrapping_add(1);
        self.versions.insert(signal.clone(), self.sequence);
        if self.queued.insert(signal.clone()) {
            self.pending.push_back(signal);
        }
    }
}

pub(crate) fn clear_telemetry_catalog(mut build: ResMut<TelemetryCatalogBuildState>) {
    let next_key = build.catalog.key.wrapping_add(1);
    *build = TelemetryCatalogBuildState::default();
    build.catalog.key = next_key;
}

fn read_entity_catalog_facts(
    entity: Entity,
    entity_info: &Query<(
        Option<&Name>,
        Option<&lunco_core::markers::Callsign>,
        Option<&lunco_core::CatalogEntryId>,
        Option<&ChildOf>,
        Option<&UsdPrimPath>,
    )>,
) -> Option<EntityCatalogFacts> {
    let (name, callsign, catalog_id, parent, usd_path) = entity_info.get(entity).ok()?;
    let label = lunco_core::entity_display_name(name, callsign, catalog_id);
    Some(EntityCatalogFacts {
        label: (!label.is_empty()).then_some(label),
        parent: parent.map(ChildOf::parent),
        usd_path: usd_path.map(|path| path.path.clone()),
    })
}

fn capture_entity_catalog_facts(
    owner: Entity,
    entity_info: &Query<(
        Option<&Name>,
        Option<&lunco_core::markers::Callsign>,
        Option<&lunco_core::CatalogEntryId>,
        Option<&ChildOf>,
        Option<&UsdPrimPath>,
    )>,
    previous: &HashMap<Entity, EntityCatalogFacts>,
    facts: &mut HashMap<Entity, EntityCatalogFacts>,
) {
    let mut cursor = Some(owner);
    for _ in 0..MAX_ANCESTOR_DEPTH {
        let Some(entity) = cursor else { break };
        if facts.contains_key(&entity) {
            break;
        }
        let Some(fact) = read_entity_catalog_facts(entity, entity_info)
            .or_else(|| previous.get(&entity).cloned())
        else {
            break;
        };
        cursor = fact.parent;
        facts.insert(entity, fact);
    }
}

/// Notifications are consumed even for hidden tabs. Only changed descriptors
/// and channels depending on changed hierarchy facts are queued for preparation.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn prepare_telemetry_catalog(
    mut build: ResMut<TelemetryCatalogBuildState>,
    registry: Option<Res<SignalRegistry>>,
    workbench: Option<Res<lunco_workbench_core::WorkbenchSnapshot>>,
    mut changes: MessageReader<lunco_signal::SignalDescriptorsChanged>,
    entity_info: Query<(
        Option<&Name>,
        Option<&lunco_core::markers::Callsign>,
        Option<&lunco_core::CatalogEntryId>,
        Option<&ChildOf>,
        Option<&UsdPrimPath>,
    )>,
    changed_owners: Query<
        Entity,
        Or<(
            Changed<Name>,
            Changed<lunco_core::markers::Callsign>,
            Changed<lunco_core::CatalogEntryId>,
            Changed<ChildOf>,
            Changed<UsdPrimPath>,
        )>,
    >,
    mut removed_names: RemovedComponents<Name>,
    mut removed_callsigns: RemovedComponents<lunco_core::markers::Callsign>,
    mut removed_catalog_ids: RemovedComponents<lunco_core::CatalogEntryId>,
    mut removed_parents: RemovedComponents<ChildOf>,
    mut removed_paths: RemovedComponents<UsdPrimPath>,
) {
    let Some(registry) = registry else {
        return;
    };
    for change in changes.read() {
        build.metrics.descriptor_notifications += change.signals.len() as u64;
        for signal in &change.signals {
            build.enqueue(signal.clone());
        }
    }
    let mut hierarchy_channels = HashSet::new();
    for entity in changed_owners
        .iter()
        .chain(removed_names.read())
        .chain(removed_callsigns.read())
        .chain(removed_catalog_ids.read())
        .chain(removed_parents.read())
        .chain(removed_paths.read())
    {
        let current = read_entity_catalog_facts(entity, &entity_info);
        let captured = build
            .in_flight_facts
            .get(&entity)
            .or_else(|| build.catalog.facts.get(&entity));
        if current.as_ref() == captured {
            continue;
        }
        for channels in [
            build.catalog.ancestor_signals.get(&entity),
            build.in_flight_ancestors.get(&entity),
        ]
        .into_iter()
        .flatten()
        {
            hierarchy_channels.extend(channels.iter().cloned());
        }
    }
    let mut hierarchy_channels: Vec<_> = hierarchy_channels.into_iter().collect();
    hierarchy_channels.sort_by(|a, b| {
        a.entity
            .to_bits()
            .cmp(&b.entity.to_bits())
            .then(a.path.cmp(&b.path))
    });
    build.metrics.hierarchy_invalidations += hierarchy_channels.len() as u64;
    for signal in hierarchy_channels {
        build.enqueue(signal);
    }
    if workbench
        .as_deref()
        .is_some_and(|snapshot| !snapshot.is_panel_visible(TELEMETRY_BROWSER_PANEL_ID))
    {
        return;
    }
    if !build.initialized {
        let mut signals: Vec<_> = registry
            .iter_scalar()
            .map(|(signal, _)| signal.clone())
            .collect();
        signals.sort_by(|a, b| {
            a.entity
                .to_bits()
                .cmp(&b.entity.to_bits())
                .then(a.path.cmp(&b.path))
        });
        for signal in signals {
            build.enqueue(signal);
        }
        build.initialized = true;
        build.metrics.initial_scans += 1;
    }
    if build.task.is_some() || build.pending.is_empty() {
        return;
    }
    let started = std::time::Instant::now();
    let _span = bevy::log::info_span!(
        "telemetry_catalog_patch_capture",
        pending = build.pending.len()
    )
    .entered();
    let mut rows = Vec::new();
    let mut facts = HashMap::new();
    for _ in 0..TELEMETRY_CATALOG_ROWS_PER_UPDATE {
        let Some(signal) = build.pending.pop_front() else {
            break;
        };
        build.queued.remove(&signal);
        let version = build.versions[&signal];
        let row = registry.scalar_history(&signal).map(|_| {
            capture_entity_catalog_facts(
                signal.entity,
                &entity_info,
                &build.catalog.facts,
                &mut facts,
            );
            let meta = registry.meta(&signal);
            Row {
                active: registry.is_active(&signal),
                drag_payload: ChannelDragPayload::from_signal(&signal),
                unit: meta.and_then(|meta| meta.unit.clone()),
                description: meta.and_then(|meta| meta.description.clone()),
                provenance: meta.and_then(|meta| meta.provenance.clone()),
                group_path: meta.and_then(|meta| meta.group_path.clone()),
                model_class: meta.and_then(|meta| meta.model_class.clone()),
                model_variable: meta.and_then(|meta| meta.model_variable.clone()),
                source_asset: meta.and_then(|meta| meta.source_asset.clone()),
                canonical_name: meta.and_then(|meta| meta.canonical_name.clone()),
                presentation: meta
                    .map(|meta| meta.presentation.clone())
                    .unwrap_or_default(),
                exposure: meta.map_or(SignalExposure::Public, |meta| meta.exposure),
                search_fields: Default::default(),
                sig: signal.clone(),
            }
        });
        if row.is_some() {
            let mut cursor = Some(signal.entity);
            let mut visited = HashSet::new();
            for _ in 0..MAX_ANCESTOR_DEPTH {
                let Some(entity) = cursor else { break };
                if !visited.insert(entity) {
                    break;
                }
                build
                    .in_flight_ancestors
                    .entry(entity)
                    .or_default()
                    .insert(signal.clone());
                cursor = facts.get(&entity).and_then(|fact| fact.parent);
            }
        }
        rows.push((signal, version, row));
    }
    build.in_flight_facts.clone_from(&facts);
    build.metrics.last_batch_channels = rows.len();
    build.metrics.prepared_channels += rows.len() as u64;
    build.metrics.last_capture_ms = started.elapsed().as_secs_f64() * 1000.0;
    build.metrics.max_capture_ms = build
        .metrics
        .max_capture_ms
        .max(build.metrics.last_capture_ms);
    build.task = Some(AsyncComputeTaskPool::get().spawn(async move {
        let started = std::time::Instant::now();
        let _span = bevy::log::info_span!("telemetry_catalog_patch_worker", channels = rows.len())
            .entered();
        let rows = rows
            .into_iter()
            .map(|(sig, version, row)| {
                let row = row.map(|row| {
                    prepare_telemetry_row(
                        row,
                        |entity| facts.get(&entity).and_then(|fact| fact.label.clone()),
                        |entity| facts.get(&entity).and_then(|fact| fact.parent),
                        |entity| facts.get(&entity).and_then(|fact| fact.usd_path.clone()),
                        |_| false,
                    )
                });
                (sig, version, row)
            })
            .collect();
        PreparedTelemetryBatch {
            rows,
            facts,
            worker_ms: started.elapsed().as_secs_f64() * 1000.0,
        }
    }));
}

/// Commit a bounded patch without replacing the tree. Only an individually
/// superseded descriptor is skipped; unrelated rows from the batch still land.
pub(crate) fn poll_telemetry_catalog(mut build: ResMut<TelemetryCatalogBuildState>) {
    let completed = build
        .task
        .as_mut()
        .and_then(|task| future::block_on(future::poll_once(task)));
    let Some(batch) = completed else {
        return;
    };
    let started = std::time::Instant::now();
    let _span = bevy::log::info_span!(
        "telemetry_catalog_patch_commit",
        channels = batch.rows.len()
    )
    .entered();
    build.task = None;
    build.in_flight_ancestors.clear();
    build.in_flight_facts.clear();
    build.metrics.last_worker_ms = batch.worker_ms;
    build.metrics.max_worker_ms = build.metrics.max_worker_ms.max(batch.worker_ms);
    for (signal, version, prepared) in batch.rows {
        if build.versions.get(&signal) == Some(&version) {
            build.versions.remove(&signal);
            if let Some(prepared) = &prepared {
                for ancestor in &prepared.ancestors {
                    if let Some(fact) = batch.facts.get(ancestor) {
                        build.catalog.facts.insert(*ancestor, fact.clone());
                    }
                }
            }
            build.catalog.apply(signal, prepared);
            build.metrics.committed_channels += 1;
        } else {
            build.metrics.superseded_channels += 1;
        }
    }
    build.metrics.last_commit_ms = started.elapsed().as_secs_f64() * 1000.0;
    build.metrics.max_commit_ms = build
        .metrics
        .max_commit_ms
        .max(build.metrics.last_commit_ms);
}

/// Read-only diagnostics for the production browser's incremental index.
/// Counters distinguish steady sampling/selection from descriptor preparation.
pub(crate) struct InspectTelemetryCatalogProvider;

impl lunco_api::queries::ApiQueryProvider for InspectTelemetryCatalogProvider {
    fn name(&self) -> &'static str {
        "InspectTelemetryCatalog"
    }

    fn schema(&self) -> lunco_api_core::ApiQuerySchema {
        lunco_api_core::ApiQuerySchema {
            name: "InspectTelemetryCatalog".to_owned(),
            description: Some("Inspect the telemetry browser's incremental presentation index and preparation costs.".to_owned()),
            parameters: Some(vec![lunco_api_core::ApiQueryParameterSchema {
                name: "signal".to_owned(), type_name: "String".to_owned(), required: false,
                description: "Exact signal path to inspect across all owning entities.".to_owned(), allowed_values: None,
            }]),
            exactly_one_of: Vec::new(),
            response: Some("Catalog counters, queue state, capture/worker/commit milliseconds, and matching descriptors with explicit entity identities.".to_owned()),
        }
    }

    fn execute(
        &self,
        world: &World,
        params: &lunco_api_core::ApiValue,
    ) -> lunco_api::ApiQueryResult {
        use lunco_api_core::api_value;
        let build = world.resource::<TelemetryCatalogBuildState>();
        let metrics = &build.metrics;
        let signal = params
            .get("signal")
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    lunco_api::queries::ApiQueryError::new(
                        lunco_api_core::ApiErrorCode::DeserializationError,
                        "InspectTelemetryCatalog: `signal` must be a string",
                    )
                })
            })
            .transpose()?;
        let mut matching: Vec<_> = build
            .catalog
            .entries
            .values()
            .filter(|entry| {
                signal
                    .as_ref()
                    .is_some_and(|name| &entry.row.sig.path == name)
            })
            .collect();
        matching.sort_by_key(|entry| entry.row.sig.entity.to_bits());
        let descriptors: Vec<_> = matching.into_iter().map(|entry| api_value!({
            "signal": entry.row.sig.path.clone(), "entity_bits": entry.row.sig.entity.to_bits(),
            "unit": entry.row.unit.clone(), "group_path": entry.row.group_path.clone(),
            "active": entry.row.active, "displayed": build.catalog.displayed.contains_key(&entry.row.sig),
            "lineage": entry.lineage.iter().map(|(id,_)| api_value!(id.clone())).collect::<Vec<_>>(),
        })).collect();
        let focus = world
            .get_resource::<TelemetryFocus>()
            .cloned()
            .unwrap_or_default();
        let focused_owners = build.catalog.focused_owners(&focus.roots, |root| {
            world.get::<UsdPrimPath>(root).map(|path| path.path.clone())
        });
        Ok(Some(api_value!({
            "indexed_channels": build.catalog.entries.len(), "displayed_channels": build.catalog.displayed.len(),
            "catalog_revision": build.catalog.key, "initial_scans": metrics.initial_scans,
            "descriptor_notifications": metrics.descriptor_notifications, "hierarchy_invalidations": metrics.hierarchy_invalidations,
            "pending_channels": build.pending.len(), "worker_active": build.task.is_some(),
            "preparing": build.catalog.root.children.is_empty() && (!build.pending.is_empty() || build.task.is_some()),
            "prepared_channels": metrics.prepared_channels, "committed_channels": metrics.committed_channels,
            "superseded_channels": metrics.superseded_channels, "last_batch_channels": metrics.last_batch_channels,
            "last_capture_ms": metrics.last_capture_ms, "max_capture_ms": metrics.max_capture_ms,
            "last_worker_ms": metrics.last_worker_ms, "max_worker_ms": metrics.max_worker_ms,
            "last_commit_ms": metrics.last_commit_ms, "max_commit_ms": metrics.max_commit_ms,
            "focused_owners": focused_owners.len(), "descriptors": descriptors,
        })))
    }
}

fn entity_in_focus(
    entity: Entity,
    roots: &[Entity],
    parent_of: impl Fn(Entity) -> Option<Entity>,
) -> bool {
    let mut cursor = Some(entity);
    for _ in 0..MAX_ANCESTOR_DEPTH {
        let Some(e) = cursor else { return false };
        if roots.contains(&e) {
            return true;
        }
        cursor = parent_of(e);
    }
    false
}

/// Normalize immutable row fields while the catalog worker constructs its tree.
fn normalized_search_fields(
    path: &str,
    description: Option<&str>,
    model_class: Option<&str>,
    model_variable: Option<&str>,
    source_asset: Option<&str>,
) -> [String; 5] {
    [
        path.to_lowercase(),
        description.unwrap_or_default().to_lowercase(),
        model_class.unwrap_or_default().to_lowercase(),
        model_variable.unwrap_or_default().to_lowercase(),
        source_asset.unwrap_or_default().to_lowercase(),
    ]
}

/// Match against catalog-normalized text and a once-per-state normalized query.
fn filter_match_prepared(filter: &str, label: &str, search_fields: &[String]) -> bool {
    filter.is_empty()
        || label.contains(filter)
        || search_fields.iter().any(|field| field.contains(filter))
}

/// Convert a signal identity into structural display nodes. Generated USD
/// namespaces may encode separators as `_x2f_`; decoding is presentation-only,
/// so registry identities and persistence remain untouched. The final dotted
/// token is the value name and is intentionally not made into another node.
fn signal_structure(path: &str) -> Vec<(String, String)> {
    let decoded = path.replace("_x2f_", "/");
    let absolute = decoded.starts_with('/');
    let mut segments: Vec<String> = decoded
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect();
    // A scalar channel with a flat name belongs directly to its owning entity.
    // Only a dotted/slashed namespace contributes a structural grouping node;
    // making every bare channel a one-child tree creates presentation noise and
    // hides the entity's actual ownership boundary.
    if segments.len() == 1 && !segments[0].contains('.') {
        return Vec::new();
    }
    if let Some(last) = segments.last_mut() {
        if let Some((component, _value)) = last.rsplit_once('.') {
            *last = component.to_owned();
        }
    }
    // A plain scalar belongs directly to its owning entity. Only a qualified
    // name or an authored path contributes an intermediate presentation node.
    if segments.len() == 1 && !decoded.contains('.') && !absolute {
        return Vec::new();
    }
    let mut authored_path = String::new();
    segments
        .into_iter()
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let label = humanize_identifier(&segment);
            let id = if absolute {
                authored_path.push('/');
                authored_path.push_str(&segment);
                authored_path.clone()
            } else {
                format!("signal-structure:{segment}")
            };
            (id, label)
        })
        .collect()
}

fn row_visible(
    row: &Row,
    scoped: bool,
    show_model_variables: bool,
    show_archived: bool,
    filter: &str,
    label: &str,
    focused_owners: &HashSet<Entity>,
) -> bool {
    (show_archived || row.active)
        && (show_model_variables || row.exposure == SignalExposure::Public)
        && (!scoped || focused_owners.contains(&row.sig.entity))
        && filter_match_prepared(filter, label, &row.search_fields)
}

/// Display the authored/operator channel name. Public and internal rows share
/// the same concise operator projection; [`SignalExposure`] supplies the
/// visual distinction while the row detail tooltip retains the exact
/// Modelica class, variable, and source asset.
fn telemetry_row_label(row: &Row, show_generated_names: bool) -> String {
    if show_generated_names {
        return row.sig.path.clone();
    }
    match &row.presentation {
        SignalPresentation::Component { component, .. } => return humanize_identifier(component),
        SignalPresentation::Summary { label, .. } => return humanize_identifier(label),
        SignalPresentation::Scalar => {}
    }
    if row.exposure == SignalExposure::Internal {
        // The tree already identifies the authored component.  Show the
        // Modelica variable here and keep the exact solver address in the
        // detail strip, so inspecting internal state does not require reading
        // a generated namespace.
        return row
            .model_variable
            .as_deref()
            .or_else(|| row.sig.path.rsplit('.').next())
            .map(|variable| operator_identifier_label(variable, row.unit.as_deref()))
            .unwrap_or_else(|| "state".to_string());
    }
    display_channel_label(
        &row.sig.path,
        row.group_path.as_deref(),
        row.unit.as_deref(),
        show_generated_names,
    )
}

fn presentation_group(presentation: &SignalPresentation) -> Option<&str> {
    match presentation {
        SignalPresentation::Scalar => None,
        SignalPresentation::Component { group, .. } | SignalPresentation::Summary { group, .. } => {
            Some(group)
        }
    }
}

#[derive(Clone, Copy)]
struct TelemetryTheme {
    text: egui::Color32,
    text_subdued: egui::Color32,
    warning: egui::Color32,
}

impl From<&lunco_theme::Theme> for TelemetryTheme {
    fn from(theme: &lunco_theme::Theme) -> Self {
        Self {
            text: theme.tokens.text,
            text_subdued: theme.tokens.text_subdued,
            warning: theme.tokens.warning,
        }
    }
}

fn telemetry_row_label_color(row: &Row, theme: &TelemetryTheme) -> egui::Color32 {
    if !row.active {
        theme.text_subdued
    } else if row.exposure == SignalExposure::Internal {
        theme.warning
    } else {
        theme.text
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct NodeVisibility {
    public: usize,
    complete: usize,
    focused: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FocusInputs(Vec<(Entity, Option<String>)>);

impl FocusInputs {
    fn capture(roots: &[Entity], root_path: impl Fn(Entity) -> Option<String>) -> Self {
        Self(roots.iter().map(|root| (*root, root_path(*root))).collect())
    }
}

#[derive(Debug)]
struct VisibilityCache {
    catalog_key: u64,
    focus_key: FocusInputs,
    filter: String,
    normalized_filter: String,
    scoped: bool,
    show_archived: bool,
    // TreeNode IDs may repeat under different owners. Node addresses key counts
    // only until the next catalog patch invalidates this visibility cache.
    counts: HashMap<usize, NodeVisibility>,
    root: NodeVisibility,
    focused_owners: HashSet<Entity>,
}

impl VisibilityCache {
    fn matches(
        &self,
        catalog_key: u64,
        focus_key: &FocusInputs,
        filter: &str,
        scoped: bool,
        show_archived: bool,
    ) -> bool {
        self.catalog_key == catalog_key
            && &self.focus_key == focus_key
            && self.filter == filter
            && self.scoped == scoped
            && self.show_archived == show_archived
    }
}

/// Build one bottom-up visibility summary per catalog/filter state so rendered
/// branches can reuse descendant counts.
fn collect_visibility(
    node: &TreeNode,
    scoped: bool,
    show_archived: bool,
    filter: &str,
    counts: &mut HashMap<usize, NodeVisibility>,
    focused_owners: &HashSet<Entity>,
) -> NodeVisibility {
    let mut visibility = NodeVisibility::default();
    for row in &node.rows {
        if focused_owners.contains(&row.sig.entity) && (row.active || show_archived) {
            visibility.focused += 1;
        }
        if (!show_archived && !row.active)
            || (scoped && !focused_owners.contains(&row.sig.entity))
            || !filter_match_prepared(filter, &node.filter_label, &row.search_fields)
        {
            continue;
        }
        visibility.complete += 1;
        if row.exposure == SignalExposure::Public {
            visibility.public += 1;
        }
    }
    for child in node.children.values() {
        let child_visibility =
            collect_visibility(child, scoped, show_archived, filter, counts, focused_owners);
        visibility.public += child_visibility.public;
        visibility.complete += child_visibility.complete;
        visibility.focused += child_visibility.focused;
    }
    counts.insert(node as *const TreeNode as usize, visibility);
    visibility
}

#[cfg(test)]
fn entity_key(entity: Entity) -> String {
    format!("entity:{}", entity.to_bits())
}

/// The virtual catalog root is never shown.
fn display_roots(root: &TreeNode) -> impl Iterator<Item = &TreeNode> {
    root.children.values()
}

enum VisibleTelemetryRow {
    Group {
        display_label: Arc<str>,
        branch_id: egui::Id,
        depth: usize,
    },
    Channel {
        row: Arc<Row>,
        depth: usize,
        stripe: usize,
    },
}

#[derive(Debug, PartialEq, Eq)]
struct VisibleTelemetryRowsKey {
    catalog_key: u64,
    focus_key: FocusInputs,
    filter: String,
    scoped: bool,
    show_model_variables: bool,
    show_archived: bool,
}

impl VisibleTelemetryRowsKey {
    fn matches(
        &self,
        catalog_key: u64,
        focus_key: &FocusInputs,
        filter: &str,
        scoped: bool,
        show_model_variables: bool,
        show_archived: bool,
    ) -> bool {
        self.catalog_key == catalog_key
            && &self.focus_key == focus_key
            && self.filter == filter
            && self.scoped == scoped
            && self.show_model_variables == show_model_variables
            && self.show_archived == show_archived
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_visible_telemetry_rows(
    ctx: &egui::Context,
    node: &TreeNode,
    parent_scope: egui::Id,
    depth: usize,
    scoped: bool,
    show_model_variables: bool,
    show_archived: bool,
    filter: &str,
    counts: &HashMap<usize, NodeVisibility>,
    rows: &mut Vec<VisibleTelemetryRow>,
    focused_owners: &HashSet<Entity>,
) {
    let visible_count = counts
        .get(&(node as *const TreeNode as usize))
        .map_or(0, |count| {
            if show_model_variables {
                count.complete
            } else {
                count.public
            }
        });
    if visible_count == 0 {
        return;
    }

    let branch_id = parent_scope.with(("tb_entity", &node.id));
    let display_label: Arc<str> = format!("{} ({visible_count})", node.label).into();
    rows.push(VisibleTelemetryRow::Group {
        display_label,
        branch_id,
        depth,
    });

    let state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ctx,
        branch_id,
        lunco_workbench_widgets::tree::default_open_at_depth(depth),
    );
    if !state.is_open() {
        return;
    }

    let child_scope = parent_scope.with(branch_id);
    for child in node.children.values() {
        collect_visible_telemetry_rows(
            ctx,
            child,
            child_scope,
            depth + 1,
            scoped,
            show_model_variables,
            show_archived,
            filter,
            counts,
            rows,
            focused_owners,
        );
    }
    for (stripe, row) in node
        .rows
        .iter()
        .filter(|row| {
            row_visible(
                row,
                scoped,
                show_model_variables,
                show_archived,
                filter,
                &node.filter_label,
                focused_owners,
            )
        })
        .enumerate()
    {
        rows.push(VisibleTelemetryRow::Channel {
            row: Arc::clone(row),
            depth: depth + 1,
            stripe,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn render_visible_telemetry_row(
    ui: &mut egui::Ui,
    entry: &VisibleTelemetryRow,
    registry: &SignalRegistry,
    theme: &TelemetryTheme,
    display_settings: &TelemetryDisplaySettings,
    row_text_cache: &mut TelemetryRowTextCache,
    selected: Option<&SignalRef>,
    clicked: &mut Option<SignalRef>,
    tree_changed: &mut bool,
) {
    match entry {
        VisibleTelemetryRow::Group {
            display_label,
            branch_id,
            depth,
        } => {
            ui.push_id(("tb_entity_row", branch_id), |ui| {
                let indent = ui.spacing().indent * *depth as f32;
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    ui.vertical(|ui| {
                        let branch_state = lunco_workbench_widgets::tree::branch(
                            ui,
                            *branch_id,
                            lunco_workbench_widgets::tree::default_open_at_depth(*depth),
                            None,
                            |ui| {
                                let width = ui.available_width();
                                lunco_workbench_widgets::tree::label(
                                    ui,
                                    egui::RichText::new(display_label.as_ref()).strong(),
                                    width,
                                    egui::Sense::click(),
                                )
                                .clicked()
                            },
                            |_| {},
                        );
                        *tree_changed |= branch_state.changed;
                    });
                });
            });
        }
        VisibleTelemetryRow::Channel { row, depth, stripe } => {
            let latest = registry
                .scalar_history(&row.sig)
                .and_then(ScalarHistory::back)
                .map(|sample| sample.value);
            let label_text =
                row_text_cache.label_text(row, theme, display_settings.show_generated_names);
            let value_text = row_text_cache.value_text(row, latest, display_settings, theme);
            let unit_text = row_text_cache.unit_text(row, theme);
            ui.push_id(("tb_channel_row", &row.sig), |ui| {
                let row_height = ui.spacing().interact_size.y;
                let row_rect = ui.available_rect_before_wrap();
                if stripe % 2 == 1 {
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(
                            row_rect.min,
                            egui::vec2(row_rect.width(), row_height),
                        ),
                        0.0,
                        ui.visuals().faint_bg_color,
                    );
                }
                let indent = ui.spacing().indent * *depth as f32;
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    let width = ui.available_width();
                    let label_width = (width * 0.55).max(72.0).min(width);
                    let inner = ui.dnd_drag_source(
                        ui.id().with(("tb_row", &row.sig)),
                        row.drag_payload.clone(),
                        |ui| {
                            lunco_workbench_widgets::tree::selectable_label(
                                ui,
                                selected == Some(&row.sig),
                                egui::WidgetText::RichText(label_text),
                                label_width,
                            )
                        },
                    );
                    let response = inner.inner;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let unit_response = ui.label(egui::WidgetText::RichText(unit_text));
                        let tip = unit_tooltip(row.unit.as_deref());
                        if !tip.is_empty() && unit_response.hovered() {
                            unit_response.on_hover_text(tip);
                        }
                        ui.label(egui::WidgetText::RichText(value_text));
                    });
                    if response.double_clicked() {
                        queue_plot_drop(
                            ui.ctx(),
                            PlotDropRequest {
                                payload: row.drag_payload.clone(),
                                world_pos: None,
                            },
                        );
                        *clicked = Some(row.sig.clone());
                    } else if response.clicked() {
                        *clicked = Some(row.sig.clone());
                    }
                    attach_row_tooltip(response, row);
                });
            });
        }
    }
}

/// Attach the source-authored explanation to a single telemetry-row cell.
/// Called once per row on the drag label only; see the call site for why the
/// value/unit cells do not each get their own `on_hover_ui` closure.
fn attach_row_tooltip(response: egui::Response, row: &Row) {
    response.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(&row.sig.path).strong().monospace());
        match &row.presentation {
            SignalPresentation::Component { group, component } => {
                ui.label(format!(
                    "Component: {} ({})",
                    humanize_identifier(component),
                    humanize_identifier(group)
                ));
            }
            SignalPresentation::Summary {
                group,
                label,
                formula,
            } => {
                ui.label(format!(
                    "Summary: {} ({})",
                    humanize_identifier(label),
                    humanize_identifier(group)
                ));
                ui.label(format!("Definition: {formula}"));
            }
            SignalPresentation::Scalar => {}
        }
        if let Some(description) = &row.description {
            ui.label(description);
        } else {
            ui.label(egui::RichText::new("No description is authored for this value.").weak());
        }
        if let Some(unit) = &row.unit {
            ui.label(egui::RichText::new(format!("Unit: {unit}")).small().weak());
        }
        if let Some(provenance) = &row.provenance {
            ui.label(
                egui::RichText::new(format!("Declared by: {provenance}"))
                    .small()
                    .weak(),
            );
        }
        if let Some(model_class) = &row.model_class {
            ui.label(format!("Modelica class: {model_class}"));
        }
        if let Some(model_variable) = &row.model_variable {
            ui.label(format!("Modelica variable: {model_variable}"));
        }
        if let Some(canonical_name) = &row.canonical_name {
            if canonical_name != &row.sig.path {
                ui.label(format!("Canonical USD channel: {canonical_name}"));
            }
        }
        if let Some(source_asset) = &row.source_asset {
            ui.label(
                egui::RichText::new(format!("Source: {source_asset}"))
                    .small()
                    .weak(),
            );
        }
        if !row.active {
            ui.label(
                egui::RichText::new("Publisher despawned; samples are retained for review.").weak(),
            );
        }
        ui.label(egui::RichText::new("Drag to a canvas; double-click to plot.").weak());
    });
}

// ── Preview cache ────────────────────────────────────────────────────

/// Same cheap ring-buffer change detector the snapshot producer uses:
/// length + first/last sample times. Any push/evict/clear moves it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct HistFingerprint {
    len: usize,
    first_t: u64,
    last_t: u64,
}

fn hist_fingerprint(h: &ScalarHistory) -> HistFingerprint {
    HistFingerprint {
        len: h.len(),
        first_t: h.front().map_or(0, |s| s.time.to_bits()),
        last_t: h.back().map_or(0, |s| s.time.to_bits()),
    }
}

struct PreviewCache {
    sig: SignalRef,
    fp: HistFingerprint,
    /// Decimated to the preview strip's pixel budget.
    points: Vec<egui_plot::PlotPoint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FormattedValueKey {
    value_bits: u64,
    significant_digits: u8,
    zero_threshold_bits: u64,
    color: egui::Color32,
}

struct CachedFormattedValue {
    key: FormattedValueKey,
    text: Arc<egui::RichText>,
}

struct CachedRowLabels {
    descriptor: std::sync::Weak<Row>,
    label_color: egui::Color32,
    unit_color: egui::Color32,
    compact: Arc<egui::RichText>,
    generated: Arc<egui::RichText>,
    unit: Arc<egui::RichText>,
}

/// Reuse row text until its channel descriptor, latest numeric value, or
/// display colors change. Builder repaints these rows more often than many
/// telemetry channels produce a new sample.
#[derive(Default)]
struct TelemetryRowTextCache {
    catalog_key: Option<u64>,
    labels: HashMap<SignalRef, CachedRowLabels>,
    values: HashMap<SignalRef, CachedFormattedValue>,
    no_sample: Option<(egui::Color32, Arc<egui::RichText>)>,
}

impl TelemetryRowTextCache {
    fn use_catalog(&mut self, catalog: &Catalog) {
        if self.catalog_key == Some(catalog.key) {
            return;
        }
        self.catalog_key = Some(catalog.key);
        self.labels
            .retain(|signal, _| catalog.entries.contains_key(signal));
        self.values
            .retain(|signal, _| catalog.entries.contains_key(signal));
    }

    fn refresh_labels(&mut self, row: &Arc<Row>, theme: &TelemetryTheme) {
        let label_color = telemetry_row_label_color(row, theme);
        let unit_color = theme.text_subdued;
        let rebuild = self.labels.get(&row.sig).is_none_or(|cached| {
            !std::ptr::eq(cached.descriptor.as_ptr(), Arc::as_ptr(row))
                || cached.label_color != label_color
                || cached.unit_color != unit_color
        });
        if rebuild {
            let make_label = |show_generated_names| {
                let mut label = telemetry_row_label(row, show_generated_names);
                if !row.active {
                    label.push_str(" (archived)");
                }
                Arc::new(egui::RichText::new(label).color(label_color))
            };
            let labels = CachedRowLabels {
                descriptor: Arc::downgrade(row),
                label_color,
                unit_color,
                compact: make_label(false),
                generated: make_label(true),
                unit: Arc::new(
                    egui::RichText::new(pretty_unit(row.unit.as_deref()))
                        .small()
                        .color(unit_color),
                ),
            };
            self.labels.insert(row.sig.clone(), labels);
        }
    }

    fn label_text(
        &mut self,
        row: &Arc<Row>,
        theme: &TelemetryTheme,
        show_generated_names: bool,
    ) -> Arc<egui::RichText> {
        self.refresh_labels(row, theme);
        let cached = self
            .labels
            .get(&row.sig)
            .expect("the telemetry row label cache was populated");
        Arc::clone(if show_generated_names {
            &cached.generated
        } else {
            &cached.compact
        })
    }

    fn unit_text(&mut self, row: &Arc<Row>, theme: &TelemetryTheme) -> Arc<egui::RichText> {
        self.refresh_labels(row, theme);
        Arc::clone(
            &self
                .labels
                .get(&row.sig)
                .expect("the telemetry row label cache was populated")
                .unit,
        )
    }

    fn value_text(
        &mut self,
        row: &Row,
        value: Option<f64>,
        settings: &TelemetryDisplaySettings,
        theme: &TelemetryTheme,
    ) -> Arc<egui::RichText> {
        let Some(value) = value else {
            let color = theme.text_subdued;
            if self
                .no_sample
                .as_ref()
                .is_none_or(|(cached_color, _)| *cached_color != color)
            {
                self.no_sample = Some((
                    color,
                    Arc::new(egui::RichText::new("—").monospace().color(color)),
                ));
            }
            return Arc::clone(
                &self
                    .no_sample
                    .as_ref()
                    .expect("the empty telemetry value was cached")
                    .1,
            );
        };

        let key = FormattedValueKey {
            value_bits: value.to_bits(),
            significant_digits: settings.significant_digits.clamp(1, 8),
            zero_threshold_bits: settings.zero_threshold.to_bits(),
            color: theme.text,
        };
        let rebuild = self
            .values
            .get(&row.sig)
            .is_none_or(|cached| cached.key != key);
        if rebuild {
            self.values.insert(
                row.sig.clone(),
                CachedFormattedValue {
                    key,
                    text: Arc::new(
                        egui::RichText::new(fmt_value(value, settings))
                            .monospace()
                            .color(theme.text),
                    ),
                },
            );
        }
        Arc::clone(
            &self
                .values
                .get(&row.sig)
                .expect("the telemetry value cache was populated")
                .text,
        )
    }
}

// ── Value formatting ─────────────────────────────────────────────────

/// Latest-value formatter: configurable significant digits, with an explicit
/// display-only near-zero threshold. The number is never rescaled.
///
/// It used to apply SI prefixes (`0.9` → `900.000m`), which is wrong the
/// moment a value has a unit the prefix cannot legally attach to. A state
/// of charge is `0.9` dimensionless, not 900 milli-anything; a `°C`
/// reading is never `mdegC`; and a prefix silently produced `km` from an
/// already-prefixed authored unit. The unit belongs to the CHANNEL — this
/// function's job is to make the digits readable, not to reinterpret the
/// quantity.
///
/// Wide magnitudes fall back to scientific notation, so a diverged sim stays
/// readable rather than becoming a wall of digits.
fn fmt_value(v: f64, settings: &TelemetryDisplaySettings) -> String {
    if !v.is_finite() {
        return "—".to_string();
    }
    if v == 0.0 {
        return "0".to_string();
    }
    let av = v.abs();
    if settings.zero_threshold > 0.0 && av < settings.zero_threshold {
        return "0".to_string();
    }
    let significant_digits = settings.significant_digits.clamp(1, 8) as i32;
    let exponent = av.log10().floor() as i32;
    let decimals = (significant_digits - 1 - exponent).max(0) as usize;
    let mut text = if !(-4..7).contains(&exponent) {
        format!(
            "{v:.precision$e}",
            precision = (significant_digits - 1) as usize
        )
    } else {
        // Round the decimal value explicitly before formatting.  Relying only
        // on binary-float formatting makes halfway decimal values such as
        // 1.2345 render as 1.234 on some toolchains.
        let factor = 10_f64.powi(decimals as i32);
        let rounded = (v * factor).round() / factor;
        format!("{rounded:.decimals$}")
    };
    if let Some((mantissa, exponent)) = text.split_once('e') {
        let trimmed = mantissa.trim_end_matches('0').trim_end_matches('.');
        text = format!("{trimmed}e{exponent}");
    } else if let Some((whole, fraction)) = text.split_once('.') {
        let fraction = fraction.trim_end_matches('0');
        text = if fraction.is_empty() {
            whole.to_string()
        } else {
            format!("{whole}.{fraction}")
        };
    }
    text
}

/// How a channel's authored unit is DISPLAYED.
///
/// Two jobs, both about not lying:
/// * `"1"` (and the empty string) mean *dimensionless* — SI's own spelling
///   for a ratio. Printing a literal `1` beside every state of charge reads
///   as the number one, so the cell stays blank and the tooltip says it.
/// * `*` is how a unit product is spelled in a `token` attribute (USD is
///   fine with `·`, but every authored unit in the library uses `*`).
///   Display uses the typographic middle dot.
///
/// Nothing here CONVERTS: a channel publishes the unit it publishes, and a
/// browser that quietly scaled values would be the bug this replaced.
fn display_unit(unit: Option<&str>) -> &str {
    match unit.map(str::trim) {
        None | Some("") | Some("1") => "",
        Some(u) => u,
    }
}

/// The unit as typeset for a cell — `N*m` → `N·m`.
fn pretty_unit(unit: Option<&str>) -> String {
    display_unit(unit).replace('*', "·")
}

/// Tooltip text for a channel's unit, including the dimensionless case the
/// cell renders as blank.
fn unit_tooltip(unit: Option<&str>) -> &'static str {
    if display_unit(unit).is_empty() {
        "dimensionless (a ratio — no unit)"
    } else {
        ""
    }
}

// ── The panel ────────────────────────────────────────────────────────

/// Telemetry channel browser panel. Registered by
/// [`crate::LuncoVizPlugin`]; no host wiring needed for the panel
/// itself. See the module docs for the two plot-creation doors.
pub struct TelemetryBrowserPanel {
    filter: String,
    visibility_cache: Option<VisibilityCache>,
    visible_rows: Vec<VisibleTelemetryRow>,
    visible_rows_key: Option<VisibleTelemetryRowsKey>,
    visible_rows_dirty: bool,
    row_text_cache: TelemetryRowTextCache,
    selected: Option<SignalRef>,
    requested_selection: Option<RequestedSignalSelection>,
    preview: Option<PreviewCache>,
    /// Narrow the list to [`TelemetryFocus`] — the selected vessel and everything
    /// under it. On by default: in an editor with a selection, "the thing I clicked"
    /// is what a telemetry panel is being opened to look at. Ignored (with the
    /// checkbox disabled) while nothing is selected, so the panel never goes blank
    /// just because the user hasn't clicked anything yet.
    focus_only: bool,
    /// Show the complete generated solver state in addition to canonical
    /// USD-facing channels. The registry/API always retain both; this is only
    /// the normal operator tree's presentation mode.
    show_model_variables: bool,
}

/// Resolve a persistent view request only when either the request or the
/// telemetry catalog changes. The panel still reapplies the resolved request
/// if the user selects another row while an external view request is active.
struct RequestedSignalSelection {
    signal: String,
    catalog_key: u64,
    resolved: Option<SignalRef>,
}

impl Default for TelemetryBrowserPanel {
    fn default() -> Self {
        Self {
            filter: String::new(),
            visibility_cache: None,
            visible_rows: Vec::new(),
            visible_rows_key: None,
            visible_rows_dirty: true,
            row_text_cache: TelemetryRowTextCache::default(),
            selected: None,
            requested_selection: None,
            preview: None,
            focus_only: true,
            // The operator view starts with the complete generated solver
            // state. `deduplicated_rows` collapses the public/member alias
            // pairs, while the checkbox remains available for a concise
            // operator-only view. A telemetry browser must not hide authored
            // model state by default.
            show_model_variables: true,
        }
    }
}

/// Pixel budget for the inline preview decimation. Fixed (not
/// measured) so the cache key doesn't churn with panel resizes.
const PREVIEW_PX_WIDTH: f32 = 240.0;

impl Panel for TelemetryBrowserPanel {
    fn id(&self) -> PanelId {
        TELEMETRY_BROWSER_PANEL_ID
    }

    fn title(&self) -> String {
        "Telemetry".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }

    fn menu_group(&self) -> PanelMenuGroup {
        PanelMenuGroup::Lunica
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        if let Some(view) = ctx.resource::<TelemetryBrowserView>() {
            if self.filter != view.filter {
                self.filter.clone_from(&view.filter);
            }
            if !view.signal.is_empty()
                && self
                    .selected
                    .as_ref()
                    .is_none_or(|selected| selected.path != view.signal)
            {
                self.selected = None;
            }
        }
        let Some(theme) = ctx
            .resource::<lunco_theme::Theme>()
            .map(TelemetryTheme::from)
        else {
            ui.label("Theme not installed.");
            return;
        };
        let subdued = theme.text_subdued;
        let telemetry_enabled = ctx
            .resource::<lunco_telemetry::TelemetrySettings>()
            .map(|settings| settings.enabled);
        if telemetry_enabled == Some(false) {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(
                        "Telemetry is off. Turn it on to collect and show channels.",
                    )
                    .color(theme.warning),
                );
                if ui.button("Turn telemetry on").clicked() {
                    ctx.trigger(lunco_telemetry::ControlTelemetry {
                        channel: None,
                        entity: None,
                        port: None,
                        reflect: None,
                        unit: None,
                        enabled: Some(true),
                        rate_hz: None,
                        retention: None,
                        atol: None,
                        rtol: None,
                        deadband: None,
                    });
                }
            });
            ui.separator();
        }
        let mut display_settings = ctx.resource_expect::<TelemetryDisplaySettings>().clone();

        // ── Filter box ───────────────────────────────────────────
        ui.add(
            lunco_workbench_widgets::text_editor::singleline(&mut self.filter)
                .hint_text("Filter channels…")
                .desired_width(f32::INFINITY),
        );

        if self.show_model_variables {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("Internal state")
                        .text_style(lunco_theme::TypographyRole::Label.text_style())
                        .color(theme.warning),
                );
                ui.label(
                    egui::RichText::new(
                        "Amber labels are implementation values; hover a row for its exact path.",
                    )
                    .color(subdued),
                );
            });
        }

        // ── Selection scope ──────────────────────────────────────
        // The focus resource is written by whichever app owns selection
        // (`lunco-scene-selection` mirrors `SelectedEntities` into it); absent ⇒ a host
        // with no selection concept at all, and the toggle simply has nothing to do.
        let has_focus = ctx
            .resource::<TelemetryFocus>()
            .is_some_and(|focus| !focus.roots.is_empty());
        let focus_key = FocusInputs::capture(
            ctx.resource::<TelemetryFocus>()
                .map(|focus| focus.roots.as_slice())
                .unwrap_or_default(),
            |root| ctx.get::<UsdPrimPath>(root).map(|path| path.path.clone()),
        );
        ui.horizontal(|ui| {
            ui.add_enabled(
                has_focus,
                egui::Checkbox::new(&mut self.focus_only, "Selected only"),
            )
            .on_hover_text(
                "Show only the selected vessel's channels — including every part \
                 underneath it (motors, battery, wheels).",
            )
            .on_disabled_hover_text("Select something in the scene to scope the list.");
            if !has_focus {
                ui.label(egui::RichText::new("nothing selected").color(subdued));
            }
            ui.checkbox(&mut self.show_model_variables, "Internal variables")
                .on_hover_text(
                    "Include generated Modelica inputs, connector values, and component state. \
                     Canonical USD-facing channels remain visible; implementation rows use the \
                     shared internal-state styling and keep their exact identity in details.",
                );
            ui.checkbox(&mut display_settings.show_archived, "Archived histories")
                .on_hover_text(
                    "Include retained channels whose publisher no longer exists. Current \
                     scene telemetry stays live-only by default; history remains available \
                     through the telemetry/API history surfaces.",
                );
            ui.menu_button("Display", |ui| {
                ui.label("Latest-value formatting");
                ui.add(
                    egui::Slider::new(&mut display_settings.significant_digits, 1..=8)
                        .text("significant digits"),
                );
                ui.horizontal(|ui| {
                    ui.label("near-zero as 0");
                    ui.add(
                        egui::DragValue::new(&mut display_settings.zero_threshold)
                            .speed(1.0e-5)
                            .range(0.0..=1.0e6),
                    );
                });
                ui.checkbox(
                    &mut display_settings.show_generated_names,
                    "Show generated names",
                )
                .on_hover_text(
                    "Use exact producer paths in the telemetry browser and graphs for diagnostics.",
                );
                ui.label(
                    egui::RichText::new("Display only; stored samples and plot data stay exact.")
                        .color(subdued),
                );
            });
        });
        if ctx.resource_expect::<TelemetryDisplaySettings>() != &display_settings {
            display_settings.significant_digits = display_settings.significant_digits.clamp(1, 8);
            display_settings.zero_threshold = if display_settings.zero_threshold.is_finite() {
                display_settings.zero_threshold.max(0.0)
            } else {
                TelemetryDisplaySettings::default().zero_threshold
            };
            ctx.set_resource(display_settings.clone());
        }
        ui.separator();

        let Some(registry) = ctx.resource::<SignalRegistry>() else {
            ui.label(egui::RichText::new("SignalRegistry not installed.").color(subdued));
            return;
        };

        // ── Change-driven catalog read ───────────────────────────
        // Descriptor patches are prepared off-thread; samples and selection
        // do not invalidate the persistent channel index.
        let key = catalog_key(registry);
        if let Some(view) = ctx.resource::<TelemetryBrowserView>() {
            if view.signal.is_empty() {
                self.requested_selection = None;
            } else {
                let refresh = self
                    .requested_selection
                    .as_ref()
                    .is_none_or(|cached| cached.signal != view.signal || cached.catalog_key != key);
                if refresh {
                    let resolved = registry
                        .iter_scalar()
                        .map(|(signal, _)| signal)
                        .find(|signal| signal.path == view.signal)
                        .cloned();
                    self.requested_selection = Some(RequestedSignalSelection {
                        signal: view.signal.clone(),
                        catalog_key: key,
                        resolved,
                    });
                }
                let requested = self
                    .requested_selection
                    .as_ref()
                    .expect("the active telemetry selection request is cached");
                if self.selected.as_ref() != requested.resolved.as_ref() {
                    self.selected.clone_from(&requested.resolved);
                }
            }
        } else {
            self.requested_selection = None;
        }
        let Some(build) = ctx.resource::<TelemetryCatalogBuildState>() else {
            ui.label("Telemetry catalog is unavailable.");
            return;
        };
        let scoped = self.focus_only && has_focus;
        let catalog = &build.catalog;

        // Cache rows by the published tree, not the pending registry revision.
        let key = catalog.key;
        self.row_text_cache.use_catalog(catalog);

        if catalog.root.children.is_empty() {
            if !build.pending.is_empty() || build.task.is_some() {
                ui.label("Preparing telemetry channels…");
            } else if telemetry_enabled != Some(false) {
                ui.label(
                    egui::RichText::new(
                        "No telemetry channels yet — run a simulation to populate the registry.",
                    )
                    .color(subdued),
                );
            }
            return;
        }

        // Scoping is applied at RENDER time, not at build time: flipping the
        // checkbox must not invalidate the catalog, and the "selection has no
        // channels" case below needs to know the difference between "no channels"
        // and "none in scope".
        if self.visibility_cache.as_ref().is_none_or(|cache| {
            !cache.matches(
                key,
                &focus_key,
                &self.filter,
                scoped,
                display_settings.show_archived,
            )
        }) {
            let mut counts = HashMap::new();
            let normalized_filter = self.filter.to_lowercase();
            let focus_roots = ctx
                .resource::<TelemetryFocus>()
                .map(|focus| focus.roots.as_slice())
                .unwrap_or_default();
            let focused_owners = catalog.focused_owners(focus_roots, |root| {
                focus_key
                    .0
                    .iter()
                    .find(|(entity, _)| *entity == root)
                    .and_then(|(_, path)| path.clone())
            });
            let root = collect_visibility(
                &catalog.root,
                scoped,
                display_settings.show_archived,
                &normalized_filter,
                &mut counts,
                &focused_owners,
            );
            self.visibility_cache = Some(VisibilityCache {
                catalog_key: key,
                focus_key: focus_key.clone(),
                filter: self.filter.clone(),
                normalized_filter,
                scoped,
                show_archived: display_settings.show_archived,
                counts,
                root,
                focused_owners,
            });
        }
        let visibility = self
            .visibility_cache
            .as_ref()
            .expect("visibility cache was populated for the current catalog");
        if scoped && visibility.root.focused == 0 {
            ui.label(
                egui::RichText::new(
                    "The selection publishes no telemetry yet — start its simulation or untick \
                     “Selected only” to inspect the rest of the live scene.",
                )
                .color(subdued),
            );
            return;
        }
        let visible = if self.show_model_variables {
            visibility.root.complete
        } else {
            visibility.root.public
        };
        if visible == 0 {
            ui.label(
                egui::RichText::new(
                    "No channels match the current display filters. Change the filter or \
                     untick Selected only.",
                )
                .color(subdued),
            );
            return;
        }

        if !self.show_model_variables {
            let hidden_count = visibility
                .root
                .complete
                .saturating_sub(visibility.root.public);
            if hidden_count > 0 {
                ui.label(
                    egui::RichText::new(format!(
                        "{hidden_count} internal variables hidden — enable Internal variables to inspect the complete model state."
                    ))
                    .color(subdued),
                );
            }
        }

        // Deferred row actions — can't mutate `self.selected` while
        // iterating the catalog snapshot.
        let mut clicked: Option<SignalRef> = None;

        // Build the open tree's lightweight row index, then let egui construct
        // widgets only for entries inside the scroll viewport.
        let visible_rows_stale = self.visible_rows_dirty
            || !self.visible_rows_key.as_ref().is_some_and(|cache| {
                cache.matches(
                    key,
                    &focus_key,
                    &visibility.normalized_filter,
                    scoped,
                    self.show_model_variables,
                    display_settings.show_archived,
                )
            });
        if visible_rows_stale {
            self.visible_rows.clear();
            let tree_scope = egui::Id::new("telemetry_browser_tree");
            for node in display_roots(&catalog.root) {
                collect_visible_telemetry_rows(
                    ui.ctx(),
                    node,
                    tree_scope,
                    0,
                    scoped,
                    self.show_model_variables,
                    display_settings.show_archived,
                    &visibility.normalized_filter,
                    &visibility.counts,
                    &mut self.visible_rows,
                    &visibility.focused_owners,
                );
            }
            self.visible_rows_key = Some(VisibleTelemetryRowsKey {
                catalog_key: key,
                focus_key,
                filter: visibility.normalized_filter.clone(),
                scoped,
                show_model_variables: self.show_model_variables,
                show_archived: display_settings.show_archived,
            });
            self.visible_rows_dirty = false;
        }

        // ── Channel list ─────────────────────────────────────────
        let detail_reserve = if self.selected.is_some() { 150.0 } else { 0.0 };
        let row_height = ui.spacing().interact_size.y;
        let mut tree_changed = false;
        egui::ScrollArea::vertical()
            .id_salt("telemetry_browser_list")
            .auto_shrink([false, false])
            .max_height((ui.available_height() - detail_reserve).max(60.0))
            .show_rows(ui, row_height, self.visible_rows.len(), |ui, range| {
                for row_index in range {
                    render_visible_telemetry_row(
                        ui,
                        &self.visible_rows[row_index],
                        registry,
                        &theme,
                        &display_settings,
                        &mut self.row_text_cache,
                        self.selected.as_ref(),
                        &mut clicked,
                        &mut tree_changed,
                    );
                }
            });
        if tree_changed {
            self.visible_rows_dirty = true;
        }

        if let Some(sig) = clicked {
            self.selected = Some(sig);
        }

        // ── Detail strip: latest value + inline preview ──────────
        let selected_is_inactive = self.selected.as_ref().is_some_and(|selected| {
            !display_settings.show_archived && !registry.is_active(selected)
        });
        if selected_is_inactive {
            self.selected = None;
            return;
        }
        let selected_is_hidden = self.selected.as_ref().is_some_and(|selected| {
            !self.show_model_variables
                && registry
                    .meta(selected)
                    .is_some_and(|meta| meta.exposure == SignalExposure::Internal)
        });
        if selected_is_hidden {
            self.selected = None;
            return;
        }
        let Some(sel) = self.selected.as_ref() else {
            return;
        };
        ui.separator();
        let metadata = registry.meta(sel);
        let unit = metadata.and_then(|m| m.unit.as_deref());
        let description = metadata.and_then(|m| m.description.as_deref());
        let hist = registry.scalar_history(sel);
        let latest = hist.and_then(ScalarHistory::back).copied();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(&sel.path)
                    .strong()
                    .monospace()
                    .color(theme.text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Time is always SECONDS here — it is the channel's own clock
                // reading (`lunco_time::domain_time`), not a wall-clock stamp, so
                // it is labelled with its unit like any other quantity.
                let text = match latest {
                    Some(s) => {
                        let u = pretty_unit(unit.as_deref());
                        let value = fmt_value(s.value, &display_settings);
                        let t = fmt_value(s.time, &display_settings);
                        if u.is_empty() {
                            format!("{value}   t = {t} s")
                        } else {
                            format!("{value} {u}   t = {t} s")
                        }
                    }
                    None => "no samples".to_string(),
                };
                ui.label(egui::RichText::new(text).monospace().color(subdued));
            });
        });
        if let Some(description) = description {
            ui.label(egui::RichText::new(description).color(subdued));
        }
        if let Some(metadata) = metadata {
            if let Some(model_class) = metadata.model_class.as_deref() {
                let variable = metadata.model_variable.as_deref().unwrap_or("unknown");
                ui.label(
                    egui::RichText::new(format!("Modelica: {model_class}.{variable}"))
                        .small()
                        .color(subdued),
                );
            }
        }

        // Preview points: re-copied + re-decimated only when the
        // history fingerprint moved (idle sim = fingerprint compare).
        if let Some(h) = hist {
            let fp = hist_fingerprint(h);
            let stale = !matches!(&self.preview, Some(p) if &p.sig == sel && p.fp == fp);
            if stale {
                let raw: Vec<[f64; 2]> = h.iter().map(|s| [s.time, s.value]).collect();
                let points = crate::plot_fmt::decimate_min_max(&raw, PREVIEW_PX_WIDTH)
                    .unwrap_or(raw)
                    .into_iter()
                    .map(egui_plot::PlotPoint::from)
                    .collect();
                self.preview = Some(PreviewCache {
                    sig: sel.clone(),
                    fp,
                    points,
                });
            }
        } else {
            self.preview = None;
        }
        if let Some(p) = &self.preview {
            if !p.points.is_empty() {
                let color = crate::signal::color_for_signal(
                    ctx.resource_expect::<lunco_theme::Theme>(),
                    &sel.path,
                );
                Plot::new(ui.id().with("tb_preview"))
                    .height(120.0)
                    .show_axes([true, true])
                    .show_grid(true)
                    .allow_drag(false)
                    .allow_zoom(false)
                    .allow_scroll(false)
                    .allow_boxed_zoom(false)
                    .sense(egui::Sense::hover())
                    .show(ui, |plot_ui| {
                        plot_ui.line(
                            Line::new(sel.path.as_str(), PlotPoints::from(p.points.as_slice()))
                                .color(color),
                        );
                    });
            }
        }

        // In-crate door that needs no canvas at all: a dedicated
        // plot tab through the existing VizPanel/LinePlot substrate.
        if ui.button("Open as plot tab").clicked() {
            let Some(viz_registry) = ctx.resource::<VisualizationRegistry>() else {
                return;
            };
            let cfg = VisualizationConfig {
                id: viz_registry.allocate_id(),
                title: display_channel_label(
                    &sel.path,
                    metadata.and_then(|meta| meta.group_path.as_deref()),
                    metadata.and_then(|meta| meta.unit.as_deref()),
                    display_settings.show_generated_names,
                ),
                kind: LINE_PLOT_KIND,
                view: ViewTarget::Panel2D,
                inputs: vec![SignalBinding::live(sel.clone(), "y")],
                style: serde_json::Value::Null,
            };
            let instance = cfg.id.raw();
            ctx.trigger(OpenVisualizationRequested { config: cfg });
            ctx.trigger(OpenTab {
                kind: VIZ_PANEL_KIND,
                instance,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::SignalRef;
    use crate::signal::compact_channel_label;

    fn ent(n: u32) -> Entity {
        Entity::from_raw_u32(n).unwrap()
    }

    fn indexed_row(registry: &SignalRegistry, signal: &SignalRef) -> PreparedTelemetryRow {
        let row = snapshot_rows(registry)
            .into_iter()
            .find(|row| &row.sig == signal)
            .unwrap();
        prepare_telemetry_row(
            row,
            |_| Some("Source".to_string()),
            |_| None,
            |_| None,
            |_| false,
        )
    }

    #[test]
    fn incremental_catalog_preserves_unrelated_rows_and_promotes_aliases() {
        let owner = ent(1);
        let canonical = SignalRef::new(owner, "power");
        let alias = SignalRef::new(owner, "wrapper.power");
        let unrelated = SignalRef::new(ent(2), "speed");
        let mut registry = SignalRegistry::default();
        for signal in [&canonical, &alias, &unrelated] {
            registry.push_scalar(signal.clone(), 0.0, 1.0);
        }
        for (signal, exposure) in [
            (&canonical, SignalExposure::Public),
            (&alias, SignalExposure::Internal),
        ] {
            registry.update_meta(
                signal.clone(),
                crate::signal::SignalMeta {
                    group_path: Some("/Vehicle/Power".into()),
                    model_class: Some("Plant".into()),
                    model_variable: Some("power".into()),
                    canonical_name: Some("power".into()),
                    unit: Some("W".into()),
                    exposure,
                    ..Default::default()
                },
            );
        }
        let mut catalog = Catalog::default();
        for signal in [&canonical, &alias, &unrelated] {
            catalog.apply(signal.clone(), Some(indexed_row(&registry, signal)));
        }
        catalog.facts.insert(owner, EntityCatalogFacts::default());
        let stable = Arc::clone(&catalog.entries[&unrelated].row);
        assert!(catalog.displayed.contains_key(&canonical));
        assert!(!catalog.displayed.contains_key(&alias));
        registry.deactivate_signal(&canonical);
        catalog.apply(canonical.clone(), Some(indexed_row(&registry, &canonical)));
        assert!(!catalog.displayed.contains_key(&canonical));
        assert!(catalog.displayed.contains_key(&alias));
        let mut meta = registry.meta(&alias).unwrap().clone();
        meta.group_path = Some("/Vehicle/Thermal".into());
        registry.update_meta(alias.clone(), meta);
        catalog.apply(alias.clone(), Some(indexed_row(&registry, &alias)));
        assert!(
            catalog.displayed[&alias]
                .iter()
                .any(|id| id == "/Vehicle/Thermal")
        );
        assert!(Arc::ptr_eq(&stable, &catalog.entries[&unrelated].row));
        catalog.apply(canonical.clone(), None);
        assert!(catalog.facts.contains_key(&owner));
        catalog.apply(alias.clone(), None);
        assert!(!catalog.facts.contains_key(&owner));
        assert!(!catalog.root.children.contains_key("/Vehicle"));
        assert!(catalog.displayed.contains_key(&unrelated));
        assert!(registry.scalar_history(&canonical).is_some());
    }

    #[test]
    fn incremental_catalog_focus_inputs_track_selected_usd_paths() {
        let owner = ent(1);
        let root = ent(2);
        let signal = SignalRef::new(owner, "power");
        let mut registry = SignalRegistry::default();
        registry.push_scalar(signal.clone(), 0.0, 1.0);
        let mut catalog = Catalog::default();
        catalog.apply(signal.clone(), Some(indexed_row(&registry, &signal)));
        catalog.facts.insert(
            owner,
            EntityCatalogFacts {
                usd_path: Some("/Vehicle/Power".into()),
                ..Default::default()
            },
        );
        let before = FocusInputs::capture(&[root], |_| Some("/Vehicle".into()));
        let after = FocusInputs::capture(&[root], |_| Some("/Elsewhere".into()));
        assert_ne!(before, after);
        assert_eq!(
            before,
            FocusInputs::capture(&[root], |_| Some("/Vehicle".into()))
        );
        let key = catalog.key;
        assert!(
            catalog
                .focused_owners(&[root], |_| before.0[0].1.clone())
                .contains(&owner)
        );
        assert!(
            catalog
                .focused_owners(&[root], |_| after.0[0].1.clone())
                .is_empty()
        );
        assert_eq!(catalog.key, key);
    }

    #[test]
    fn incremental_catalog_superseded_row_does_not_discard_unrelated_patch() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<TelemetryCatalogBuildState>()
            .add_systems(Update, poll_telemetry_catalog);
        let a = SignalRef::new(ent(1), "a");
        let b = SignalRef::new(ent(2), "b");
        let mut registry = SignalRegistry::default();
        for signal in [&a, &b] {
            registry.push_scalar(signal.clone(), 0.0, 1.0);
        }
        let stable;
        {
            let mut build = app.world_mut().resource_mut::<TelemetryCatalogBuildState>();
            for signal in [&a, &b] {
                build
                    .catalog
                    .apply(signal.clone(), Some(indexed_row(&registry, signal)));
                registry.update_meta(
                    signal.clone(),
                    crate::signal::SignalMeta {
                        unit: Some("V".into()),
                        ..Default::default()
                    },
                );
            }
            stable = Arc::clone(&build.catalog.entries[&a].row);
            build.versions.insert(a.clone(), 2);
            build.versions.insert(b.clone(), 1);
            build.pending.push_back(a.clone());
            build.queued.insert(a.clone());
            let batch = PreparedTelemetryBatch {
                rows: vec![
                    (a.clone(), 1, Some(indexed_row(&registry, &a))),
                    (b.clone(), 1, Some(indexed_row(&registry, &b))),
                ],
                facts: Default::default(),
                worker_ms: 0.0,
            };
            build.task = Some(AsyncComputeTaskPool::get().spawn(async move { batch }));
        }
        for _ in 0..3000 {
            app.update();
            if app
                .world()
                .resource::<TelemetryCatalogBuildState>()
                .task
                .is_none()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_micros(100));
        }
        let build = app.world().resource::<TelemetryCatalogBuildState>();
        assert!(build.task.is_none());
        assert!(Arc::ptr_eq(&stable, &build.catalog.entries[&a].row));
        assert_eq!(build.catalog.entries[&b].row.unit.as_deref(), Some("V"));
        assert_eq!(build.metrics.committed_channels, 1);
        assert_eq!(build.metrics.superseded_channels, 1);
        assert_eq!(build.pending.front(), Some(&a));
        assert_eq!(build.versions.get(&a), Some(&2));
    }

    fn settle_catalog(app: &mut App) {
        for _ in 0..3000 {
            app.update();
            let build = app.world().resource::<TelemetryCatalogBuildState>();
            if build.initialized && build.pending.is_empty() && build.task.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_micros(100));
        }
        panic!("bounded catalog patches did not settle");
    }

    #[test]
    fn incremental_catalog_async_batches_do_not_restart_for_selection_or_samples() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, lunco_signal::SignalRegistryPlugin))
            .init_resource::<TelemetryCatalogBuildState>()
            .init_resource::<TelemetryFocus>()
            .add_systems(
                Update,
                (prepare_telemetry_catalog, poll_telemetry_catalog)
                    .chain()
                    .after(lunco_signal::SignalDescriptorPublish),
            )
            .add_systems(lunco_core::SceneTeardown, clear_telemetry_catalog);
        let owner = app.world_mut().spawn(Name::new("Source")).id();
        let signal = SignalRef::new(owner, "signal_0");
        {
            let mut registry = app.world_mut().resource_mut::<SignalRegistry>();
            for index in 0..1024 {
                registry.push_scalar(SignalRef::new(owner, format!("signal_{index}")), 0.0, 1.0);
            }
        }
        settle_catalog(&mut app);
        let prepared = app
            .world()
            .resource::<TelemetryCatalogBuildState>()
            .metrics
            .prepared_channels;
        assert_eq!(prepared, 1024);
        for step in 0..10 {
            app.world_mut()
                .entity_mut(owner)
                .insert(Name::new("Source"));
            app.world_mut().resource_mut::<TelemetryFocus>().roots =
                if step % 2 == 0 { vec![owner] } else { vec![] };
            app.world_mut()
                .resource_mut::<SignalRegistry>()
                .push_scalar(signal.clone(), step as f64 + 1.0, 2.0);
            app.update();
        }
        assert_eq!(
            app.world()
                .resource::<TelemetryCatalogBuildState>()
                .metrics
                .prepared_channels,
            prepared
        );
        app.world_mut()
            .resource_mut::<SignalRegistry>()
            .update_meta(
                signal.clone(),
                crate::signal::SignalMeta {
                    unit: Some("V".into()),
                    ..Default::default()
                },
            );
        settle_catalog(&mut app);
        let build = app.world().resource::<TelemetryCatalogBuildState>();
        assert_eq!(build.metrics.prepared_channels, prepared + 1);
        assert_eq!(build.metrics.last_batch_channels, 1);
        assert_eq!(build.metrics.initial_scans, 1);
        assert_eq!(
            build.catalog.entries[&signal].row.unit.as_deref(),
            Some("V")
        );
        app.world_mut()
            .entity_mut(owner)
            .insert(Name::new("Renamed source"));
        settle_catalog(&mut app);
        let build = app.world().resource::<TelemetryCatalogBuildState>();
        assert_eq!(build.metrics.prepared_channels, prepared + 1 + 1024);
        assert_eq!(
            build.catalog.facts[&owner].label.as_deref(),
            Some("Renamed Source")
        );
        let previous_key = build.catalog.key;
        lunco_core::run_scene_teardown(app.world_mut());
        let build = app.world().resource::<TelemetryCatalogBuildState>();
        assert!(build.catalog.entries.is_empty());
        assert!(build.pending.is_empty());
        assert!(build.task.is_none());
        assert!(build.catalog.key > previous_key);
    }

    #[test]
    fn draggable_telemetry_rows_use_left_aligned_tree_labels() {
        let row = Arc::new(Row {
            sig: SignalRef::new(ent(1), "x"),
            drag_payload: ChannelDragPayload {
                entity_bits: ent(1).to_bits(),
                path: Arc::from("x"),
            },
            unit: None,
            description: None,
            provenance: None,
            group_path: None,
            model_class: None,
            model_variable: None,
            source_asset: None,
            canonical_name: None,
            presentation: SignalPresentation::Scalar,
            exposure: SignalExposure::Public,

            active: true,
            search_fields: Default::default(),
        });
        let label = telemetry_row_label(&row, false);
        let entry = VisibleTelemetryRow::Channel {
            row,
            depth: 0,
            stripe: 0,
        };
        let registry = SignalRegistry::default();
        let theme = TelemetryTheme {
            text: egui::Color32::WHITE,
            text_subdued: egui::Color32::GRAY,
            warning: egui::Color32::YELLOW,
        };
        let display_settings = TelemetryDisplaySettings::default();
        let mut row_text_cache = TelemetryRowTextCache::default();
        let mut clicked = None;
        let mut tree_changed = false;
        let mut row_left = 0.0;
        let mut label_width = 0.0;
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(600.0, 200.0),
        ));

        let egui_context = egui::Context::default();
        egui_context.all_styles_mut(|style| {
            lunco_theme::Theme::default()
                .typography
                .apply_to_style(style)
        });
        let output = egui_context.run_ui(input, |ui| {
            let available = ui.available_rect_before_wrap();
            row_left = available.left();
            label_width = (available.width() * 0.55).max(72.0).min(available.width());
            render_visible_telemetry_row(
                ui,
                &entry,
                &registry,
                &theme,
                &display_settings,
                &mut row_text_cache,
                None,
                &mut clicked,
                &mut tree_changed,
            );
        });

        let label_x = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos.x),
                _ => None,
            })
            .expect("the telemetry channel label should be painted");
        assert!(
            label_x < row_left + label_width * 0.25,
            "channel label started at {label_x}, away from the left edge of its column"
        );
    }

    #[test]
    fn telemetry_labels_keep_public_names_concise_and_internal_names_distinct() {
        let public = Row {
            sig: SignalRef::new(ent(1), "electrical_power"),
            drag_payload: ChannelDragPayload {
                entity_bits: ent(1).to_bits(),
                path: Arc::from("electrical_power"),
            },
            unit: None,
            description: None,
            provenance: None,
            group_path: None,
            model_class: Some("LunCo.Electrical.DCMotor".into()),
            model_variable: Some("electrical_power".into()),
            source_asset: None,
            canonical_name: None,
            presentation: SignalPresentation::Scalar,
            exposure: SignalExposure::Public,

            active: true,
            search_fields: Default::default(),
        };
        assert_eq!(telemetry_row_label(&public, false), "electrical power");

        let mut internal = public;
        internal.sig = SignalRef::new(
            ent(1),
            "__member_Traverse_x2f_Rover_x2f_Motor_L0.electrical_power",
        );
        internal.drag_payload = ChannelDragPayload::from_signal(&internal.sig);
        internal.exposure = SignalExposure::Internal;
        assert_eq!(telemetry_row_label(&internal, false), "electrical power");
        assert_eq!(
            telemetry_row_label(&internal, true),
            "__member_Traverse_x2f_Rover_x2f_Motor_L0.electrical_power"
        );
    }

    #[test]
    fn telemetry_browser_starts_with_complete_model_state_visible() {
        assert!(TelemetryBrowserPanel::default().show_model_variables);
        assert!(!TelemetryDisplaySettings::default().show_archived);
    }

    #[test]
    fn archived_rows_are_hidden_without_erasing_their_history() {
        let entity = ent(1);
        let signal = SignalRef::new(entity, "old_state");
        let mut reg = SignalRegistry::default();
        reg.push_scalar(signal.clone(), 0.0, 1.0);
        reg.deactivate_signal(&signal);

        let rows = deduplicated_rows(&reg);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].active);
        assert!(reg.scalar_history(&signal).is_some());
        assert!(!row_visible(
            &rows[0],
            false,
            true,
            false,
            "",
            "Rover",
            &HashSet::new()
        ));
        assert!(row_visible(
            &rows[0],
            false,
            true,
            true,
            "",
            "Rover",
            &HashSet::new()
        ));
    }

    #[test]
    fn live_model_state_wins_over_an_archived_alias() {
        let entity = ent(1);
        let archived = SignalRef::new(entity, "old_state");
        let live = SignalRef::new(entity, "new_state");
        let mut reg = SignalRegistry::default();
        for signal in [&archived, &live] {
            reg.push_scalar(signal.clone(), 0.0, 1.0);
            reg.update_meta(
                signal.clone(),
                crate::signal::SignalMeta {
                    group_path: Some("/Traverse/Rover/Motor".into()),
                    model_class: Some("LunCo.Electrical.Motor".into()),
                    model_variable: Some("power_w".into()),
                    exposure: SignalExposure::Internal,
                    ..Default::default()
                },
            );
        }
        reg.deactivate_signal(&archived);

        let rows = deduplicated_rows(&reg);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sig, live);
        assert!(rows[0].active);
    }

    #[test]
    fn duplicate_modelica_aliases_collapse_to_the_canonical_component_state() {
        let entity = ent(1);
        let mut reg = SignalRegistry::default();
        let public = SignalRef::new(entity, "power_draw");
        let generated = SignalRef::new(entity, "unit_1_Rover.Traverse_x2f_Rover_Camera.power_draw");
        for signal in [&public, &generated] {
            reg.push_scalar(signal.clone(), 0.0, 1.0);
            reg.update_meta(
                signal.clone(),
                crate::signal::SignalMeta {
                    group_path: Some("/Traverse/Rover/Camera".into()),
                    model_class: Some("LunCo.Electrical.CameraPayload".into()),
                    model_variable: Some("power_draw_w".into()),
                    exposure: if signal == &public {
                        SignalExposure::Public
                    } else {
                        SignalExposure::Internal
                    },
                    ..Default::default()
                },
            );
        }

        let rows = deduplicated_rows(&reg);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sig, public);
        assert_eq!(telemetry_row_label(&rows[0], false), "power draw");
    }

    #[test]
    fn duplicate_public_modelica_aliases_choose_the_canonical_path() {
        let entity = ent(1);
        let mut reg = SignalRegistry::default();
        let canonical = SignalRef::new(entity, "battery_capacity_ah");
        let generated = SignalRef::new(entity, "unit_1_Rover_Battery.battery_capacity_ah");
        for signal in [&canonical, &generated] {
            reg_push_with_meta(
                &mut reg,
                signal,
                SignalExposure::Public,
                Some("battery_capacity_ah"),
            );
        }

        let rows = deduplicated_rows(&reg);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sig, canonical);
        assert_eq!(telemetry_row_label(&rows[0], false), "capacity ah");
    }

    fn reg_push_with_meta(
        reg: &mut SignalRegistry,
        signal: &SignalRef,
        exposure: SignalExposure,
        canonical_name: Option<&str>,
    ) {
        reg.push_scalar(signal.clone(), 0.0, 1.0);
        reg.update_meta(
            signal.clone(),
            crate::signal::SignalMeta {
                group_path: Some("/Traverse/Rover/Battery".into()),
                model_class: Some("LunCo.Electrical.Battery".into()),
                model_variable: Some("capacity_ah".into()),
                canonical_name: canonical_name.map(str::to_owned),
                exposure,
                ..Default::default()
            },
        );
    }

    #[test]
    fn catalog_key_ignores_pushes_but_sees_catalog_changes() {
        let mut reg = SignalRegistry::default();
        reg.push_scalar(SignalRef::new(ent(1), "a"), 0.0, 1.0);
        let k1 = catalog_key(&reg);

        // More samples on an existing channel: same set, same key.
        reg.push_scalar(SignalRef::new(ent(1), "a"), 1.0, 2.0);
        assert_eq!(
            k1,
            catalog_key(&reg),
            "samples keep the channel lookup valid"
        );

        // New channel: key moves.
        reg.push_scalar(SignalRef::new(ent(2), "b"), 0.0, 3.0);
        let k2 = catalog_key(&reg);
        assert_ne!(k1, k2, "admission refreshes the channel lookup");

        // Channel removal changes the monotonic catalog revision too.
        reg.remove_signal(&SignalRef::new(ent(2), "b"));
        assert_ne!(
            k2,
            catalog_key(&reg),
            "removal refreshes the channel lookup"
        );
    }

    #[test]
    fn tree_follows_entity_ownership_and_carries_units() {
        let mut reg = SignalRegistry::default();
        reg.push_scalar(SignalRef::new(ent(2), "z.speed"), 0.0, 1.0);
        reg.push_scalar(SignalRef::new(ent(2), "a.torque"), 0.0, 2.0);
        reg.push_scalar(SignalRef::new(ent(1), "x"), 0.0, 3.0);
        reg.update_meta(
            SignalRef::new(ent(2), "z.speed"),
            crate::signal::SignalMeta {
                unit: Some("m/s".into()),
                ..Default::default()
            },
        );

        let tree = build_tree(
            &reg,
            |e| (e == ent(1)).then(|| "Alpha Rover".to_string()),
            |_| None,
            |_| None,
            |_| false,
        );
        assert_eq!(tree.children.len(), 2);
        let alpha = tree.children.get(&entity_key(ent(1))).unwrap();
        let unnamed = tree.children.get(&entity_key(ent(2))).unwrap();
        assert_eq!(alpha.label.as_ref(), "Alpha Rover");
        assert_eq!(alpha.rows.len(), 1);
        assert!(unnamed.rows.is_empty());
        let a = unnamed.children.get("signal-structure:a").unwrap();
        let z = unnamed.children.get("signal-structure:z").unwrap();
        // Rows sorted by path within the group.
        assert_eq!(a.rows[0].sig.path, "a.torque");
        assert_eq!(z.rows[0].sig.path, "z.speed");
        assert_eq!(z.rows[0].unit.as_deref(), Some("m/s"));
    }

    #[test]
    fn prepared_filter_matches_normalized_catalog_fields() {
        let row_fields = normalized_search_fields("wheel.speed", None, None, None, None);
        let label = "Rover".to_lowercase();
        assert!(filter_match_prepared("", &label, &row_fields));
        assert!(filter_match_prepared(
            "SPEED".to_lowercase().as_str(),
            &label,
            &row_fields
        ));
        assert!(filter_match_prepared(
            "rov".to_lowercase().as_str(),
            &label,
            &row_fields
        ));
        assert!(!filter_match_prepared(
            "thrust".to_lowercase().as_str(),
            &label,
            &row_fields
        ));

        let metadata_fields = normalized_search_fields(
            "science_power",
            None,
            Some("LunCo.Electrical.CameraPayload"),
            Some("power_draw_w"),
            Some("lunco://models/LunCo/Electrical/CameraPayload.mo"),
        );
        assert!(filter_match_prepared(
            "camerapayload".to_lowercase().as_str(),
            &"power draw".to_lowercase(),
            &metadata_fields
        ));
    }

    #[test]
    fn focus_membership_is_an_ancestor_test_not_an_equality_test() {
        // rover ← rocker ← motor: the channel sits on the motor, the user
        // selected the rover.
        let rover = ent(1);
        let rocker = ent(2);
        let motor = ent(3);
        let other = ent(9);
        let parent = |e: Entity| match e {
            e if e == motor => Some(rocker),
            e if e == rocker => Some(rover),
            _ => None,
        };
        assert!(entity_in_focus(motor, &[rover], parent));
        assert!(entity_in_focus(rover, &[rover], parent));
        assert!(!entity_in_focus(other, &[rover], parent));
        assert!(!entity_in_focus(motor, &[], parent), "no focus ⇒ no member");
    }

    #[test]
    fn a_hierarchy_cycle_terminates_the_walk() {
        let a = ent(1);
        let b = ent(2);
        // Corrupt hierarchy: a → b → a. Must return, not hang.
        let parent = |e: Entity| Some(if e == a { b } else { a });
        assert!(!entity_in_focus(a, &[ent(7)], parent));
    }

    #[test]
    fn tree_uses_parentage_for_subsystems_and_signal_structure_for_values() {
        let mut reg = SignalRegistry::default();
        let rover = ent(1);
        let motors = ent(2);
        let left_motor = ent(3);
        let comms = ent(4);
        reg.push_scalar(SignalRef::new(left_motor, "current"), 0.0, 1.0);
        reg.push_scalar(SignalRef::new(left_motor, "temperature"), 0.0, 2.0);
        reg.push_scalar(SignalRef::new(comms, "beam.locked"), 0.0, 1.0);
        let parent = |e| match e {
            e if e == motors => Some(rover),
            e if e == left_motor => Some(motors),
            e if e == comms => Some(rover),
            _ => None,
        };
        let tree = build_tree(
            &reg,
            |e| match e {
                e if e == rover => Some("Skid Rover".into()),
                e if e == motors => Some("Motors".into()),
                e if e == left_motor => Some("Left Motor".into()),
                e if e == comms => Some("Comms".into()),
                _ => None,
            },
            parent,
            |_| None,
            |_| false,
        );
        let rover = tree.children.get(&entity_key(rover)).unwrap();
        assert_eq!(
            rover.children[&entity_key(motors)].children[&entity_key(left_motor)]
                .rows
                .len(),
            2
        );
        let comms = &rover.children[&entity_key(comms)];
        let beam = &comms.children["signal-structure:beam"];
        assert_eq!(beam.rows.len(), 1);
    }

    #[test]
    fn usd_path_hierarchy_ignores_runtime_reparenting() {
        let mut reg = SignalRegistry::default();
        let physics_world = ent(1);
        let joint = ent(2);
        let wheel = ent(3);
        reg.push_scalar(SignalRef::new(wheel, "axle_torque"), 0.0, 0.9);
        let parent = |e| match e {
            // The runtime physics backend has reparented the wheel underneath
            // a joint. This must not leak into operator telemetry navigation.
            e if e == wheel => Some(joint),
            e if e == joint => Some(physics_world),
            _ => None,
        };
        let tree = build_tree(
            &reg,
            |_| None,
            parent,
            |e| (e == wheel).then(|| "/SandboxScene/Skid_Rover/Wheel_FL".to_string()),
            |_| false,
        );
        assert_eq!(tree.children.len(), 1);
        let scene = tree.children.get("/SandboxScene").unwrap();
        let rover = &scene.children["/SandboxScene/Skid_Rover"];
        let wheel = &rover.children["/SandboxScene/Skid_Rover/Wheel_FL"];
        assert_eq!(wheel.label.as_ref(), "Wheel FL");
        assert_eq!(wheel.rows[0].sig.path, "axle_torque");
    }

    #[test]
    fn generated_signal_uses_composed_presentation_path_and_network_root() {
        let mut reg = SignalRegistry::default();
        let network_root = ent(1);
        let signal = SignalRef::new(network_root, "Motor__FL.p.v");
        reg.push_scalar(signal.clone(), 0.0, 24.0);
        reg.update_meta(
            signal,
            crate::signal::SignalMeta {
                group_path: Some("/SandboxScene/Rover/Motor_FL".into()),
                model_variable: Some("p.v".into()),
                ..Default::default()
            },
        );

        let tree = build_tree(
            &reg,
            |_| None,
            |_| None,
            |entity| (entity == network_root).then(|| "/SandboxScene/Rover".to_string()),
            |_| false,
        );

        let rover = &tree.children["/SandboxScene"].children["/SandboxScene/Rover"];
        assert!(
            !rover.children.contains_key("/SandboxScene/Rover/Power"),
            "the removed domain child must not reappear as a telemetry node"
        );
        let motor = &rover.children["/SandboxScene/Rover/Motor_FL"];
        assert_eq!(motor.label.as_ref(), "Motor FL");
        let p = &motor.children["signal-structure:p"];
        assert_eq!(p.rows[0].sig.path, "Motor__FL.p.v");
    }

    #[test]
    fn authored_group_path_merges_channels_from_different_producers() {
        let mut reg = SignalRegistry::default();
        let readback = SignalRef::new(ent(1), "torque");
        let modelica = SignalRef::new(ent(2), "electrical_power");
        for signal in [&readback, &modelica] {
            reg.push_scalar(signal.clone(), 0.0, 1.0);
            reg.update_meta(
                signal.clone(),
                crate::signal::SignalMeta {
                    group_path: Some("/Traverse/Rover/Motor_L0".into()),
                    ..Default::default()
                },
            );
        }

        let tree = build_tree(
            &reg,
            |_| None,
            |_| None,
            |entity| match entity {
                e if e == ent(1) => Some("/Traverse/Rover/Motor_L0".into()),
                e if e == ent(2) => Some("/Traverse/Rover".into()),
                _ => None,
            },
            |_| false,
        );
        let rover = &tree.children["/Traverse"].children["/Traverse/Rover"];
        let motor = &rover.children["/Traverse/Rover/Motor_L0"];
        assert_eq!(motor.rows.len(), 2);
        assert!(!rover.children.contains_key("/Traverse/Rover/Power"));
    }

    #[test]
    fn semantic_presentations_group_components_and_summaries() {
        let mut reg = SignalRegistry::default();
        let entity = ent(1);
        let component_signals = [
            ("linear_velocity.x", "x"),
            ("linear_velocity.y", "y"),
            ("linear_velocity.z", "z"),
        ];
        for (path, component) in component_signals {
            let signal = SignalRef::new(entity, path);
            reg.push_scalar(signal.clone(), 0.0, 1.0);
            reg.update_meta(
                signal,
                crate::signal::SignalMeta {
                    group_path: Some("/Traverse/Rover".into()),
                    presentation: SignalPresentation::Component {
                        group: "linear_velocity".into(),
                        component: component.into(),
                    },
                    ..Default::default()
                },
            );
        }
        let summary = SignalRef::new(entity, "linear_speed");
        reg.push_scalar(summary.clone(), 0.0, 1.0);
        reg.update_meta(
            summary,
            crate::signal::SignalMeta {
                group_path: Some("/Traverse/Rover".into()),
                presentation: SignalPresentation::Summary {
                    group: "linear_velocity".into(),
                    label: "speed".into(),
                    formula: "magnitude".into(),
                },
                ..Default::default()
            },
        );

        let tree = build_tree(
            &reg,
            |_| Some("Rover".into()),
            |_| None,
            |_| None,
            |_| false,
        );
        let rover = tree
            .children
            .get("/Traverse")
            .unwrap()
            .children
            .get("/Traverse/Rover")
            .unwrap();
        let velocity = rover.children.get("signal-group:linear_velocity").unwrap();
        assert_eq!(velocity.label.as_ref(), "linear velocity");
        assert_eq!(velocity.rows.len(), 4);
        assert_eq!(
            velocity
                .rows
                .iter()
                .find(|row| matches!(
                    &row.presentation,
                    SignalPresentation::Component { component, .. } if component == "x"
                ))
                .map(|row| telemetry_row_label(row, false)),
            Some("x".into())
        );
        assert_eq!(
            velocity
                .rows
                .iter()
                .find(|row| matches!(&row.presentation, SignalPresentation::Summary { .. }))
                .map(|row| telemetry_row_label(row, false)),
            Some("speed".into())
        );
    }

    #[test]
    fn focus_fingerprint_moves_with_the_selection() {
        use lunco_signal::TelemetryFocus;
        let empty = TelemetryFocus::default();
        let one = TelemetryFocus {
            roots: vec![ent(1)],
        };
        let two = TelemetryFocus {
            roots: vec![ent(2)],
        };
        assert_ne!(empty.fingerprint(), one.fingerprint());
        assert_ne!(one.fingerprint(), two.fingerprint());
        assert_eq!(one.fingerprint(), one.fingerprint());
    }

    #[test]
    fn dropped_node_goes_through_the_plot_substrate() {
        let payload = ChannelDragPayload {
            entity_bits: ent(7).to_bits(),
            path: Arc::from("P.y"),
        };
        let node = plot_node_at(
            lunco_canvas::scene::NodeId(1),
            lunco_canvas::Pos::new(10.0, 20.0),
            &payload,
        );
        assert_eq!(node.kind, PLOT_NODE_KIND);
        let data = node
            .data
            .downcast_ref::<PlotNodeData>()
            .expect("payload must downcast in the visual factory");
        assert_eq!(data.signal_path, "P.y");
        assert_eq!(
            data.binding,
            PlotBinding::Pinned {
                entity: ent(7).to_bits()
            }
        );
        assert!(node.resizable);
    }

    #[test]
    fn drop_queue_round_trips() {
        let ctx = egui::Context::default();
        let req = PlotDropRequest {
            payload: ChannelDragPayload {
                entity_bits: 1,
                path: Arc::from("x"),
            },
            world_pos: None,
        };
        queue_plot_drop(&ctx, req.clone());
        assert_eq!(drain_plot_drops(&ctx), vec![req]);
        assert!(drain_plot_drops(&ctx).is_empty(), "drain consumes");
    }

    #[test]
    fn fmt_value_is_four_significant_digits_and_never_rescales() {
        let settings = TelemetryDisplaySettings::default();
        assert_eq!(fmt_value(0.0, &settings), "0");
        // The bug this replaced: a state of charge is 0.9, NOT "900.000m".
        assert_eq!(fmt_value(0.9, &settings), "0.9");
        assert_eq!(fmt_value(26_000.0, &settings), "26000");
        assert_eq!(fmt_value(-0.0042, &settings), "-0.0042");
        assert_eq!(fmt_value(1.234_5, &settings), "1.235");
        assert_eq!(fmt_value(12.345, &settings), "12.35");
        assert_eq!(fmt_value(9.67e-5, &settings), "0");
        // Wide magnitudes stay readable rather than becoming a wall of digits.
        assert_eq!(fmt_value(1.2e260, &settings), "1.2e260");
        assert_eq!(fmt_value(f64::NAN, &settings), "—");
    }

    #[test]
    fn channel_label_removes_the_owning_category_only_at_a_name_boundary() {
        assert_eq!(
            compact_channel_label("Motor__L0.electrical_power", "Motor L0", Some("W")),
            "electrical power"
        );
        assert_eq!(
            compact_channel_label("Motor L0 terminal_voltage_v", "Motor L0", Some("V")),
            "terminal voltage"
        );
        assert_eq!(
            compact_channel_label("Motor L01.speed", "Motor L0", Some("rad/s")),
            "Motor L01.speed"
        );
    }

    #[test]
    fn telemetry_labels_explain_modelica_connector_state() {
        let mut internal = Row {
            sig: SignalRef::new(ent(1), "network_system.Battery.p.v"),
            drag_payload: ChannelDragPayload {
                entity_bits: ent(1).to_bits(),
                path: Arc::from("network_system.Battery.p.v"),
            },
            unit: Some("V".into()),
            description: Some("Electrical pin voltage".into()),
            provenance: Some("modelica".into()),
            group_path: Some("/Rover/Battery".into()),
            model_class: Some("LunCo.Electrical.Battery".into()),
            model_variable: Some("p.v".into()),
            source_asset: None,
            canonical_name: None,
            presentation: SignalPresentation::Scalar,
            exposure: SignalExposure::Internal,

            active: true,
            search_fields: Default::default(),
        };
        assert_eq!(telemetry_row_label(&internal, false), "pin voltage");
        internal.model_variable = Some("p.i".into());
        internal.unit = Some("A".into());
        assert_eq!(telemetry_row_label(&internal, false), "pin current");
    }

    #[test]
    fn dimensionless_units_render_blank_and_products_use_a_middle_dot() {
        // SI spells a ratio's unit `1`; printing it beside the value reads as
        // the number one.
        assert_eq!(pretty_unit(Some("1")), "");
        assert_eq!(pretty_unit(Some("")), "");
        assert_eq!(pretty_unit(None), "");
        assert!(
            !unit_tooltip(Some("1")).is_empty(),
            "blank cell must explain itself"
        );

        assert_eq!(pretty_unit(Some("N*m")), "N·m");
        assert_eq!(pretty_unit(Some("m/s")), "m/s");
        assert_eq!(
            pretty_unit(Some("1/s")),
            "1/s",
            "a rate is not dimensionless"
        );
        assert!(unit_tooltip(Some("m/s")).is_empty());
    }
}
