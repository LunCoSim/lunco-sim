//! Telemetry channel browser — the real replacement for the deleted
//! `lunco-ui/src/telemetry.rs` tombstone panel.
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
//! registry has no revision counter, so we derive a cheap
//! order-independent fingerprint of the channel *set* (xor of per-ref
//! hashes + count — same spirit as the history fingerprints the
//! canvas-plot snapshot producer uses). Sample pushes don't move it;
//! only channels appearing/disappearing do. Latest-value cells are
//! O(1) `samples.back()` reads per visible row.
//!
//! ## Scoping to the selection
//!
//! A channel is owned by the prim it measures — a motor, a battery, a wheel —
//! while the user selects a VESSEL. "Selected only" therefore filters by an
//! ANCESTOR test against [`lunco_signal::TelemetryFocus`]
//! ([`entity_in_focus`]), not by entity equality, which would show nothing for
//! every rover ever selected. The focus resource is written by whichever app owns
//! selection (`lunco-luncosim-edit` mirrors `SelectedEntities` into it); a host
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

use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::egui;
use egui_plot::{Line, Plot, PlotPoints};
use lunco_core::{on_command, register_commands, Command};
use lunco_usd_bevy::UsdPrimPath;
use lunco_workbench::{OpenTab, Panel, PanelCtx, PanelId, PanelMenuGroup, PanelSlot};

use crate::kinds::canvas_plot_node::{PlotBinding, PlotNodeData, PLOT_NODE_KIND};
use crate::registry::VisualizationRegistry;
use crate::signal::{ScalarHistory, SignalRef, SignalRegistry, TelemetryFocus};
use crate::view::ViewTarget;
use crate::viz::{SignalBinding, VisualizationConfig, VizId};
use crate::{LINE_PLOT_KIND, VIZ_PANEL_KIND};

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
    /// Signal path, e.g. `"P.y"`.
    pub path: String,
}

impl ChannelDragPayload {
    pub fn from_signal(sig: &SignalRef) -> Self {
        Self {
            entity_bits: sig.entity.to_bits(),
            path: sig.path.clone(),
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
        signal_path: payload.path.clone(),
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
    let source = SignalRef::new(Entity::from_bits(payload.entity_bits), payload.path.clone());
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

#[derive(Clone, Debug)]
struct Row {
    sig: SignalRef,
    unit: Option<String>,
    description: Option<String>,
    provenance: Option<String>,
    in_focus: bool,
    model_internal: bool,
    active: bool,
}

#[derive(Clone, Debug)]
struct TreeNode {
    label: String,
    id: String,
    children: std::collections::BTreeMap<String, TreeNode>,
    rows: Vec<Row>,
}

impl TreeNode {
    fn new(id: String, label: String) -> Self {
        Self {
            label,
            id,
            children: Default::default(),
            rows: Vec::new(),
        }
    }
}

struct Catalog {
    key: u64,
    /// [`TelemetryFocus::fingerprint`] the tree's `in_focus` flags were built
    /// against. Selecting a different rover changes no channel, so the channel-set
    /// key alone would leave every flag stale.
    focus_key: u64,
    root: TreeNode,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            key: 0,
            focus_key: 0,
            root: TreeNode::new("root".to_string(), "Telemetry".to_string()),
        }
    }
}

/// Monotonic revision of the channel catalog. The signal registry owns this
/// change detection because it is the sole owner of the channel set and its
/// metadata; sampling must not make the UI scan and hash every channel.
fn catalog_key(reg: &SignalRegistry) -> u64 {
    reg.catalog_revision()
}

/// Reduce an authored USD prim path to the prim's display name.  Its complete
/// path remains available in the tooltip; the browser uses this label only to
/// keep the live hierarchy readable in a narrow panel.
fn display_entity_name(name: &str) -> String {
    name.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .replace('_', " ")
}

/// Rebuild the catalog from the live ownership hierarchy. This deliberately
/// has no `wheel`, `motor`, `beam`, or other name-based classifier: the USD
/// parent graph supplies the assembly, subsystem, and component grouping for
/// every scene, including ones the editor has never seen before.
fn build_tree(
    reg: &SignalRegistry,
    name_of: impl Fn(Entity) -> Option<String>,
    parent_of: impl Fn(Entity) -> Option<Entity>,
    usd_path_of: impl Fn(Entity) -> Option<String>,
    is_navigation_root: impl Fn(Entity) -> bool,
    in_focus: impl Fn(Entity) -> bool,
) -> TreeNode {
    let mut root = TreeNode::new("root".to_string(), "Telemetry".to_string());
    for (sig, _hist) in reg.iter_scalar() {
        let meta = reg.meta(sig);
        let has_authored_owner = usd_path_of(sig.entity).is_some() || name_of(sig.entity).is_some();
        let provenance = meta.and_then(|m| m.provenance.clone());
        let model_internal = provenance.as_deref() == Some("cosim")
            || sig.path.starts_with("sim.")
            || sig.path.contains("_x2f_");
        let row = Row {
            sig: sig.clone(),
            unit: meta.and_then(|m| m.unit.clone()),
            description: meta.and_then(|m| m.description.clone()),
            provenance,
            in_focus: sig.entity != Entity::PLACEHOLDER && in_focus(sig.entity),
            model_internal,
            active: reg.is_active(sig),
        };

        let mut lineage: Vec<(String, String)> = if let Some(path) = usd_path_of(sig.entity) {
            let mut key = String::new();
            path.trim_matches('/')
                .split('/')
                .filter(|s| !s.is_empty())
                .map(|segment| {
                    key.push('/');
                    key.push_str(segment);
                    (key.clone(), segment.replace('_', " "))
                })
                .collect()
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
                        name_of(entity)
                            .map(|name| display_entity_name(&name))
                            .unwrap_or_else(|| format!("Entity {entity}"))
                    };
                    (format!("entity:{}", entity.to_bits()), label)
                })
                .collect()
        };
        // Extend the authored entity lineage with every structural segment in
        // the signal path. A generated model publishes on its domain wrapper
        // entity, so replace that wrapper's leaf with the output's authored
        // component path. The user sees the model that owns the value rather
        // than an implementation container. This remains producer-neutral:
        // every hierarchical model path gets the same treatment.
        let group_path = meta.and_then(|metadata| {
            metadata
                .group_path
                .as_deref()
                .filter(|path| !path.is_empty())
        });
        let mut structure = signal_structure(
            group_path.unwrap_or(if has_authored_owner { "" } else { &sig.path }),
        );
        // Canonical paths may repeat the USD ancestry already represented by
        // the entity lineage (for example `SandboxScene/Rover/Battery`). Drop
        // that shared prefix so the visual tree does not duplicate the vessel.
        while structure
            .first()
            .is_some_and(|(id, _)| lineage.iter().any(|(parent_id, _)| parent_id == id))
        {
            structure.remove(0);
        }
        // `group_path` is an authored ownership path. Once its shared USD prefix
        // has been removed, the remaining component names are presentation
        // structure, not new USD prims. Keep that distinction explicit so a
        // generated solver wrapper cannot reappear as a second authored branch.
        if group_path.is_some_and(|path| path.trim_start().starts_with('/')) {
            for (id, _) in &mut structure {
                if let Some(segment) = id.rsplit('/').next().filter(|s| !s.is_empty()) {
                    *id = format!("signal-structure:{segment}");
                }
            }
        }
        // A resolved group path means the projection supplied an authored
        // ownership target. Replace the runtime wrapper leaf regardless of
        // whether that projection entity currently has a USD path component.
        if meta
            .and_then(|metadata| metadata.group_path.as_ref())
            .is_some_and(|path| !path.is_empty())
            && !structure.is_empty()
            && !lineage.is_empty()
        {
            lineage.pop();
        }
        lineage.extend(structure);
        let mut node = &mut root;
        for (id, label) in lineage {
            node = node
                .children
                .entry(id.clone())
                .or_insert_with(|| TreeNode::new(id, label));
        }
        node.rows.push(row);
    }
    sort_tree(&mut root);
    root
}

fn sort_tree(node: &mut TreeNode) {
    node.rows.sort_by(|a, b| a.sig.path.cmp(&b.sig.path));
    for child in node.children.values_mut() {
        sort_tree(child);
    }
}

/// Depth cap for the ancestor walk that decides focus membership. A vessel is a
/// handful of levels deep (rover → rocker → motor); the cap exists so a cyclic or
/// corrupt hierarchy can't spin the UI thread, not because 32 is a real limit.
const MAX_ANCESTOR_DEPTH: usize = 32;

/// Is `entity` one of `roots`, or a descendant of one?
///
/// This is why the focus resource carries ROOTS and not channel owners: the user
/// selects a rover, but its channels sit on the motor / battery / wheel prims
/// underneath it. `parent_of` is the caller's hierarchy accessor (the panel passes
/// `PanelCtx::get::<ChildOf>`), so this stays a pure function and is testable
/// without a `World`.
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

/// Case-insensitive substring filter over authored labels, descriptions, and
/// stable signal paths.
fn filter_match(filter: &str, label: &str, path: &str, description: Option<&str>) -> bool {
    if filter.is_empty() {
        return true;
    }
    let f = filter.to_lowercase();
    path.to_lowercase().contains(&f)
        || label.to_lowercase().contains(&f)
        || description.is_some_and(|text| text.to_lowercase().contains(&f))
}

/// Compact channel label for a narrow telemetry pane. The registry path remains
/// the stable identity and is always available in the row tooltip; this is only
/// the operator-facing presentation of it.
fn compact_channel_label(path: &str) -> String {
    let decoded = path.replace("_x2f_", "/");
    let readable = decoded.trim_matches('/').rsplit('/').next().unwrap_or(path);
    readable.replace('_', " ")
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
    let mut authored_path = String::new();
    segments
        .into_iter()
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let label = segment.replace('_', " ");
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

fn row_visible(row: &Row, scoped: bool, show_internals: bool, filter: &str, label: &str) -> bool {
    (!scoped || row.in_focus)
        && (show_internals || !row.model_internal)
        && filter_match(filter, label, &row.sig.path, row.description.as_deref())
}

fn tree_any_row(node: &TreeNode, predicate: impl Fn(&Row) -> bool + Copy) -> bool {
    node.rows.iter().any(predicate)
        || node
            .children
            .values()
            .any(|child| tree_any_row(child, predicate))
}

fn visible_count(node: &TreeNode, scoped: bool, show_internals: bool, filter: &str) -> usize {
    node.rows
        .iter()
        .filter(|r| row_visible(r, scoped, show_internals, filter, &node.label))
        .count()
        + node
            .children
            .values()
            .map(|c| visible_count(c, scoped, show_internals, filter))
            .sum::<usize>()
}

#[cfg(test)]
fn entity_key(entity: Entity) -> String {
    format!("entity:{}", entity.to_bits())
}

/// The virtual catalog root is never shown.
fn display_roots(root: &TreeNode) -> Vec<&TreeNode> {
    root.children.values().collect()
}

#[allow(clippy::too_many_arguments)]
fn render_tree_node(
    ui: &mut egui::Ui,
    node: &TreeNode,
    registry: &SignalRegistry,
    theme: &lunco_theme::Theme,
    scoped: bool,
    show_internals: bool,
    filter: &str,
    selected: Option<&SignalRef>,
    depth: usize,
    clicked: &mut Option<SignalRef>,
) {
    let visible = visible_count(node, scoped, show_internals, filter);
    if visible == 0 {
        return;
    }
    egui::CollapsingHeader::new(
        egui::RichText::new(format!("{} ({visible})", node.label)).strong(),
    )
    .id_salt(("tb_entity", &node.id))
    .default_open(depth < 2)
    .show(ui, |ui| {
        for child in node.children.values() {
            render_tree_node(
                ui,
                child,
                registry,
                theme,
                scoped,
                show_internals,
                filter,
                selected,
                depth + 1,
                clicked,
            );
        }
        egui::Grid::new(("tb_grid", &node.id))
            .num_columns(3)
            .striped(true)
            .spacing(egui::vec2(theme.spacing.item_spacing, 2.0))
            .show(ui, |ui| {
                for row in node
                    .rows
                    .iter()
                    .filter(|r| row_visible(r, scoped, show_internals, filter, &node.label))
                {
                    let latest = registry
                        .scalar_history(&row.sig)
                        .and_then(|h| h.samples.back())
                        .map(|s| s.value);
                    let payload = ChannelDragPayload::from_signal(&row.sig);
                    let inner = ui.dnd_drag_source(
                        ui.id().with(("tb_row", &row.sig)),
                        payload.clone(),
                        |ui| {
                            ui.selectable_label(
                                selected == Some(&row.sig),
                                egui::RichText::new(if row.active {
                                    compact_channel_label(&row.sig.path)
                                } else {
                                    format!("{} (archived)", compact_channel_label(&row.sig.path))
                                })
                                .color(if row.active {
                                    theme.tokens.text
                                } else {
                                    theme.tokens.text_subdued
                                }),
                            )
                        },
                    );
                    let response = inner.inner;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(
                                latest.map(fmt_value).unwrap_or_else(|| "—".into()),
                            )
                            .monospace()
                            .color(if latest.is_some() {
                                theme.tokens.text
                            } else {
                                theme.tokens.text_subdued
                            }),
                        )
                    });
                    let unit_response = ui.label(
                        egui::RichText::new(pretty_unit(row.unit.as_deref()))
                            .small()
                            .color(theme.tokens.text_subdued),
                    );
                    let tip = unit_tooltip(row.unit.as_deref());
                    if !tip.is_empty() {
                        unit_response.on_hover_text(tip);
                    }
                    ui.end_row();
                    if response.double_clicked() {
                        queue_plot_drop(
                            ui.ctx(),
                            PlotDropRequest {
                                payload,
                                world_pos: None,
                            },
                        );
                        *clicked = Some(row.sig.clone());
                    } else if response.clicked() {
                        *clicked = Some(row.sig.clone());
                    }
                    // One tooltip closure per row. `on_hover_ui` registers a
                    // closure for every widget it is called on each frame (the
                    // body only runs when the pointer is over the cell), so
                    // attaching it to the drag label, value cell, AND unit cell
                    // tripled the per-frame closure registrations and was a real
                    // FPS cost on channel-dense scenes. The drag label is the
                    // primary hover target; the unit cell keeps its own cheap
                    // static-text tooltip for the dimensionless case.
                    attach_row_tooltip(response, row);
                }
            });
    });
}

/// Attach the source-authored explanation to a single telemetry-row cell.
/// Called once per row on the drag label only; see the call site for why the
/// value/unit cells do not each get their own `on_hover_ui` closure.
fn attach_row_tooltip(response: egui::Response, row: &Row) {
    response.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(&row.sig.path).strong().monospace());
        if let Some(description) = &row.description {
            ui.label(description);
        } else {
            ui.label(
                egui::RichText::new("No description is authored for this value.")
                    .small()
                    .weak(),
            );
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
        if !row.active {
            ui.label(
                egui::RichText::new("Publisher despawned; samples are retained for review.")
                    .small()
                    .weak(),
            );
        }
        ui.label(
            egui::RichText::new("Drag to a canvas; double-click to plot.")
                .small()
                .weak(),
        );
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
        first_t: h.samples.front().map_or(0, |s| s.time.to_bits()),
        last_t: h.samples.back().map_or(0, |s| s.time.to_bits()),
    }
}

struct PreviewCache {
    sig: SignalRef,
    fp: HistFingerprint,
    /// Decimated to the preview strip's pixel budget.
    points: Vec<[f64; 2]>,
}

// ── Value formatting ─────────────────────────────────────────────────

/// Latest-value formatter: **four significant digits, and the number is
/// never rescaled**.
///
/// It used to apply SI prefixes (`0.9` → `900.000m`), which is wrong the
/// moment a value has a unit the prefix cannot legally attach to. A state
/// of charge is `0.9` dimensionless, not 900 milli-anything; a `°C`
/// reading is never `mdegC`; and a prefix silently produced `km` from an
/// already-prefixed authored unit. The unit belongs to the CHANNEL — this
/// function's job is to make the digits readable, not to reinterpret the
/// quantity.
///
/// Wide magnitudes fall back to scientific notation, so a diverged sim
/// still shows `1.20e260` rather than a wall of digits.
fn fmt_value(v: f64) -> String {
    if !v.is_finite() {
        return "—".to_string();
    }
    if v == 0.0 {
        return "0".to_string();
    }
    let av = v.abs();
    if !(1.0e-4..1.0e7).contains(&av) {
        return format!("{v:.2e}");
    }
    // Four significant digits: enough to see a value move, few enough that
    // the column stays a column.
    let decimals = (3 - av.log10().floor() as i32).clamp(0, 6) as usize;
    format!("{v:.decimals$}")
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
    catalog: Catalog,
    selected: Option<SignalRef>,
    preview: Option<PreviewCache>,
    /// Narrow the list to [`TelemetryFocus`] — the selected vessel and everything
    /// under it. On by default: in an editor with a selection, "the thing I clicked"
    /// is what a telemetry panel is being opened to look at. Ignored (with the
    /// checkbox disabled) while nothing is selected, so the panel never goes blank
    /// just because the user hasn't clicked anything yet.
    focus_only: bool,
    /// Generated model variables are part of the vessel's observable state.
    /// Keep them visible by default; users can hide them for a mission-only view.
    show_model_internals: bool,
}

impl Default for TelemetryBrowserPanel {
    fn default() -> Self {
        Self {
            filter: String::new(),
            catalog: Catalog::default(),
            selected: None,
            preview: None,
            focus_only: true,
            show_model_internals: true,
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
        PanelMenuGroup::Design
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        if let Some(view) = ctx.resource::<TelemetryBrowserView>().cloned() {
            if self.filter != view.filter {
                self.filter = view.filter;
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
        let Some(theme) = ctx.resource::<lunco_theme::Theme>().cloned() else {
            ui.label("Theme not installed.");
            return;
        };
        let subdued = theme.tokens.text_subdued;

        // ── Filter box ───────────────────────────────────────────
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("Filter channels…")
                .desired_width(f32::INFINITY),
        );

        // ── Selection scope ──────────────────────────────────────
        // The focus resource is written by whichever app owns selection
        // (`lunco-luncosim-edit` mirrors `SelectedEntities` into it); absent ⇒ a host
        // with no selection concept at all, and the toggle simply has nothing to do.
        let focus: Vec<Entity> = ctx
            .resource::<TelemetryFocus>()
            .map(|f| f.roots.clone())
            .unwrap_or_default();
        let focus_key = ctx
            .resource::<TelemetryFocus>()
            .map(|f| f.fingerprint())
            .unwrap_or(0);
        ui.horizontal(|ui| {
            ui.add_enabled(
                !focus.is_empty(),
                egui::Checkbox::new(&mut self.focus_only, "Selected only"),
            )
            .on_hover_text(
                "Show only the selected vessel's channels — including every part \
                 underneath it (motors, battery, wheels).",
            )
            .on_disabled_hover_text("Select something in the scene to scope the list.");
            if focus.is_empty() {
                ui.label(
                    egui::RichText::new("nothing selected")
                        .small()
                        .color(subdued),
                );
            }
            ui.checkbox(&mut self.show_model_internals, "Model variables")
                .on_hover_text(
                    "Show state published by Modelica and other co-simulation models. \n                     Turn this off for an explicitly authored mission-only view.");
        });
        ui.separator();

        let Some(registry) = ctx.resource::<SignalRegistry>() else {
            ui.label(egui::RichText::new("SignalRegistry not installed.").color(subdued));
            return;
        };

        if let Some(view) = ctx.resource::<TelemetryBrowserView>().cloned() {
            if !view.signal.is_empty() {
                self.selected = registry
                    .iter_scalar()
                    .map(|(signal, _)| signal)
                    .find(|signal| signal.path == view.signal)
                    .cloned();
            }
        }

        // ── Change-driven catalog rebuild ────────────────────────
        // Two independent invalidators: the channel SET (a sim started, a vessel
        // spawned) and the FOCUS (the user clicked a different rover — same channels,
        // different membership).
        let key = catalog_key(registry);
        if self.catalog.key != key
            || self.catalog.focus_key != focus_key
            || (self.catalog.root.children.is_empty() && key != 0)
        {
            let root = build_tree(
                registry,
                |e| ctx.get::<Name>(e).map(|n| n.as_str().to_string()),
                |c| ctx.get::<ChildOf>(c).map(|p| p.parent()),
                |e| ctx.get::<UsdPrimPath>(e).map(|path| path.path.clone()),
                |_| false,
                |e| {
                    let Some(path) = ctx.get::<UsdPrimPath>(e).map(|path| path.path.as_str())
                    else {
                        return entity_in_focus(e, &focus, |c| {
                            ctx.get::<ChildOf>(c).map(|p| p.parent())
                        });
                    };
                    focus.iter().any(|root| {
                        ctx.get::<UsdPrimPath>(*root).is_some_and(|root_path| {
                            path == root_path.path
                                || path
                                    .strip_prefix(root_path.path.as_str())
                                    .is_some_and(|suffix| suffix.starts_with('/'))
                        })
                    })
                },
            );
            self.catalog = Catalog {
                key,
                focus_key,
                root,
            };
        }

        if self.catalog.root.children.is_empty() {
            ui.label(
                egui::RichText::new(
                    "No telemetry channels yet — run a simulation to populate the registry.",
                )
                .color(subdued),
            );
            return;
        }

        // Scoping is applied at RENDER time, not at build time: flipping the
        // checkbox must not invalidate the catalog, and the "selection has no
        // channels" case below needs to know the difference between "no channels"
        // and "none in scope".
        let scoped = self.focus_only && !focus.is_empty();
        if scoped && !tree_any_row(&self.catalog.root, |row| row.in_focus) {
            ui.label(
                egui::RichText::new(
                    "The selection publishes no telemetry yet — start its simulation or untick \
                     “Selected only” to inspect the rest of the live scene.",
                )
                .color(subdued),
            );
            return;
        }

        // Deferred row actions — can't mutate `self.selected` while
        // iterating `self.catalog`.
        let mut clicked: Option<SignalRef> = None;

        // ── Channel list ─────────────────────────────────────────
        let detail_reserve = if self.selected.is_some() { 150.0 } else { 0.0 };
        egui::ScrollArea::vertical()
            .id_salt("telemetry_browser_list")
            .auto_shrink([false, false])
            .max_height((ui.available_height() - detail_reserve).max(60.0))
            .show(ui, |ui| {
                for node in display_roots(&self.catalog.root) {
                    render_tree_node(
                        ui,
                        node,
                        registry,
                        &theme,
                        scoped,
                        self.show_model_internals,
                        &self.filter,
                        self.selected.as_ref(),
                        0,
                        &mut clicked,
                    );
                }
            });

        if let Some(sig) = clicked {
            self.selected = Some(sig);
        }

        // ── Detail strip: latest value + inline preview ──────────
        let Some(sel) = self.selected.clone() else {
            return;
        };
        ui.separator();
        let metadata = registry.meta(&sel);
        let unit = metadata.and_then(|m| m.unit.clone());
        let description = metadata.and_then(|m| m.description.clone());
        let hist = registry.scalar_history(&sel);
        let latest = hist.and_then(|h| h.samples.back()).copied();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(&sel.path)
                    .strong()
                    .monospace()
                    .color(theme.tokens.text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Time is always SECONDS here — it is the channel's own clock
                // reading (`lunco_time::domain_time`), not a wall-clock stamp, so
                // it is labelled with its unit like any other quantity.
                let text = match latest {
                    Some(s) => {
                        let u = pretty_unit(unit.as_deref());
                        let value = fmt_value(s.value);
                        let t = fmt_value(s.time);
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
            ui.label(
                egui::RichText::new(description)
                    .small()
                    .color(subdued),
            );
        }

        // Preview points: re-copied + re-decimated only when the
        // history fingerprint moved (idle sim = fingerprint compare).
        if let Some(h) = hist {
            let fp = hist_fingerprint(h);
            let stale = !matches!(&self.preview, Some(p) if p.sig == sel && p.fp == fp);
            if stale {
                let raw: Vec<[f64; 2]> = h.iter().map(|s| [s.time, s.value]).collect();
                let points =
                    crate::plot_fmt::decimate_min_max(&raw, PREVIEW_PX_WIDTH).unwrap_or(raw);
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
                let color = crate::signal::color_for_signal(&theme, &sel.path);
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
                            Line::new(sel.path.as_str(), PlotPoints::from(p.points.clone()))
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
                title: sel.path.clone(),
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

    fn ent(n: u32) -> Entity {
        Entity::from_raw_u32(n).unwrap()
    }

    #[test]
    fn catalog_key_ignores_pushes_but_sees_catalog_changes() {
        let mut reg = SignalRegistry::default();
        reg.push_scalar(SignalRef::new(ent(1), "a"), 0.0, 1.0);
        let k1 = catalog_key(&reg);

        // More samples on an existing channel: same set, same key.
        reg.push_scalar(SignalRef::new(ent(1), "a"), 1.0, 2.0);
        assert_eq!(k1, catalog_key(&reg), "a push must not invalidate the list");

        // New channel: key moves.
        reg.push_scalar(SignalRef::new(ent(2), "b"), 0.0, 3.0);
        let k2 = catalog_key(&reg);
        assert_ne!(k1, k2, "a new channel must invalidate the list");

        // Channel removal changes the monotonic catalog revision too.
        reg.remove_signal(&SignalRef::new(ent(2), "b"));
        assert_ne!(k2, catalog_key(&reg), "removal must invalidate the list");
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
            |_| false,
        );
        assert_eq!(tree.children.len(), 2);
        let alpha = tree.children.get(&entity_key(ent(1))).unwrap();
        let unnamed = tree.children.get(&entity_key(ent(2))).unwrap();
        assert_eq!(alpha.label, "Alpha Rover");
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
    fn filter_matches_path_and_group_case_insensitively() {
        assert!(filter_match("", "Rover", "wheel.speed", None));
        assert!(filter_match("SPEED", "Rover", "wheel.speed", None));
        assert!(filter_match("rov", "Rover", "wheel.speed", None));
        assert!(!filter_match("thrust", "Rover", "wheel.speed", None));
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
    fn tree_carries_focus_membership() {
        let mut reg = SignalRegistry::default();
        reg.push_scalar(SignalRef::new(ent(1), "a"), 0.0, 1.0);
        reg.push_scalar(SignalRef::new(ent(2), "b"), 0.0, 1.0);
        let tree = build_tree(
            &reg,
            |_| None,
            |_| None,
            |_| None,
            |_| false,
            |e| e == ent(1),
        );
        assert!(tree.children[&entity_key(ent(1))].rows[0].in_focus);
        assert!(!tree.children[&entity_key(ent(2))].rows[0].in_focus);
    }

    #[test]
    fn tree_uses_parentage_not_signal_name_to_group_subsystems() {
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
            |_| false,
        );
        let rover = tree.children.get(&entity_key(rover)).unwrap();
        assert_eq!(
            rover.children[&entity_key(motors)].children[&entity_key(left_motor)]
                .rows
                .len(),
            2
        );
        assert_eq!(rover.children[&entity_key(comms)].rows.len(), 1);
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
            |_| false,
        );
        assert_eq!(tree.children.len(), 1);
        let scene = tree.children.get("/SandboxScene").unwrap();
        let rover = &scene.children["/SandboxScene/Skid_Rover"];
        let wheel = &rover.children["/SandboxScene/Skid_Rover/Wheel_FL"];
        assert_eq!(wheel.label, "Wheel FL");
        assert_eq!(wheel.rows[0].sig.path, "axle_torque");
    }

    #[test]
    fn generated_signal_uses_composed_presentation_path_not_solver_scope() {
        let mut reg = SignalRegistry::default();
        let electrical_scope = ent(1);
        let signal = SignalRef::new(electrical_scope, "Motor__FL.p.v");
        reg.push_scalar(signal.clone(), 0.0, 24.0);
        reg.update_meta(
            signal,
            crate::signal::SignalMeta {
                group_path: Some("/SandboxScene/Rover/Motor_FL".into()),
                ..Default::default()
            },
        );

        let tree = build_tree(
            &reg,
            |_| None,
            |_| None,
            |entity| {
                (entity == electrical_scope).then(|| "/SandboxScene/Rover/Electrical".to_string())
            },
            |_| false,
            |_| false,
        );

        let rover = &tree.children["/SandboxScene"].children["/SandboxScene/Rover"];
        assert!(
            !rover
                .children
                .contains_key("/SandboxScene/Rover/Electrical"),
            "the generated solver scope is an implementation detail"
        );
        let motor = &rover.children["signal-structure:Motor_FL"];
        assert_eq!(motor.label, "Motor FL");
        assert_eq!(motor.rows[0].sig.path, "Motor__FL.p.v");
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
        assert_eq!(one.fingerprint(), one.clone().fingerprint());
    }

    #[test]
    fn dropped_node_goes_through_the_plot_substrate() {
        let payload = ChannelDragPayload {
            entity_bits: ent(7).to_bits(),
            path: "P.y".to_string(),
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
                path: "x".into(),
            },
            world_pos: None,
        };
        queue_plot_drop(&ctx, req.clone());
        assert_eq!(drain_plot_drops(&ctx), vec![req]);
        assert!(drain_plot_drops(&ctx).is_empty(), "drain consumes");
    }

    #[test]
    fn fmt_value_is_four_significant_digits_and_never_rescales() {
        assert_eq!(fmt_value(0.0), "0");
        // The bug this replaced: a state of charge is 0.9, NOT "900.000m".
        assert_eq!(fmt_value(0.9), "0.9000");
        assert_eq!(fmt_value(26_000.0), "26000");
        assert_eq!(fmt_value(-0.0042), "-0.004200");
        assert_eq!(fmt_value(1.234_5), "1.234");
        assert_eq!(fmt_value(12.345), "12.35");
        // Wide magnitudes stay readable rather than becoming a wall of digits.
        assert_eq!(fmt_value(1.2e260), "1.20e260");
        assert_eq!(fmt_value(f64::NAN), "—");
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
