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

use std::{collections::HashMap, sync::Arc};

use bevy::prelude::*;
use bevy_egui::egui;
use egui_plot::{Line, Plot, PlotPoints};
use lunco_core::{on_command, register_commands, Command};
use lunco_settings::SettingsSection;
use lunco_usd_bevy::UsdPrimPath;
use lunco_workbench::{OpenTab, Panel, PanelCtx, PanelId, PanelMenuGroup, PanelSlot};

use crate::kinds::canvas_plot_node::{PlotBinding, PlotNodeData, PLOT_NODE_KIND};
use crate::registry::VisualizationRegistry;
use crate::signal::{
    display_channel_label, humanize_identifier, operator_identifier_label, ScalarHistory,
    SignalExposure, SignalRef, SignalRegistry, TelemetryFocus,
};
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
    group_path: Option<String>,
    model_class: Option<String>,
    model_variable: Option<String>,
    source_asset: Option<String>,
    canonical_name: Option<String>,
    exposure: SignalExposure,
    in_focus: bool,
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

fn deduplicated_rows(reg: &SignalRegistry) -> Vec<Row> {
    let mut selected = HashMap::<ModelStateIdentity, Row>::new();
    let mut standalone = Vec::new();

    for (sig, _history) in reg.iter_scalar() {
        let meta = reg.meta(sig);
        let row = Row {
            sig: sig.clone(),
            unit: meta.and_then(|m| m.unit.clone()),
            description: meta.and_then(|m| m.description.clone()),
            provenance: meta.and_then(|m| m.provenance.clone()),
            group_path: meta.and_then(|m| m.group_path.clone()),
            model_class: meta.and_then(|m| m.model_class.clone()),
            model_variable: meta.and_then(|m| m.model_variable.clone()),
            source_asset: meta.and_then(|m| m.source_asset.clone()),
            canonical_name: meta.and_then(|m| m.canonical_name.clone()),
            exposure: meta.map_or(SignalExposure::Public, |m| m.exposure),
            in_focus: sig.entity != Entity::PLACEHOLDER,
            active: reg.is_active(sig),
        };

        let Some(identity) = model_state_identity(&row) else {
            standalone.push(row);
            continue;
        };
        match selected.get(&identity) {
            Some(current)
                if model_state_priority(&row) <= model_state_priority(current)
                    && row.sig.path >= current.sig.path => {}
            _ => {
                selected.insert(identity, row);
            }
        }
    }

    standalone.extend(selected.into_values());
    standalone.sort_by(|left, right| {
        left.sig
            .entity
            .to_bits()
            .cmp(&right.sig.entity.to_bits())
            .then(left.sig.path.cmp(&right.sig.path))
    });
    standalone
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

/// Rebuild the catalog from the live ownership hierarchy. This deliberately
/// has no `wheel`, `motor`, `beam`, or other name-based classifier: the USD
/// parent graph supplies the assembly, subsystem, and component grouping for
/// every scene, including ones the editor has never seen before.
fn build_tree(
    reg: &SignalRegistry,
    label_of: impl Fn(Entity) -> Option<String>,
    parent_of: impl Fn(Entity) -> Option<Entity>,
    usd_path_of: impl Fn(Entity) -> Option<String>,
    is_navigation_root: impl Fn(Entity) -> bool,
    in_focus: impl Fn(Entity) -> bool,
) -> TreeNode {
    let mut root = TreeNode::new("root".to_string(), "Telemetry".to_string());
    for row in deduplicated_rows(reg) {
        // Keep the signal identity independent from the row move below.
        let sig = row.sig.clone();
        let row = Row {
            in_focus: sig.entity != Entity::PLACEHOLDER && in_focus(sig.entity),
            ..row
        };

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
        // A group path already names the owning component. Use the authored
        // Modelica variable for its optional value namespace; generated solver
        // spellings never become a second hierarchy. Without a group path,
        // retain the ordinary producer path structure.
        let structure_path = group_path
            .and_then(|_| row.model_variable.as_deref())
            .unwrap_or(&sig.path);
        let mut structure = signal_structure(structure_path);
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
fn filter_match(
    filter: &str,
    label: &str,
    path: &str,
    description: Option<&str>,
    model_class: Option<&str>,
    model_variable: Option<&str>,
    source_asset: Option<&str>,
) -> bool {
    if filter.is_empty() {
        return true;
    }
    let f = filter.to_lowercase();
    path.to_lowercase().contains(&f)
        || label.to_lowercase().contains(&f)
        || description.is_some_and(|text| text.to_lowercase().contains(&f))
        || model_class.is_some_and(|text| text.to_lowercase().contains(&f))
        || model_variable.is_some_and(|text| text.to_lowercase().contains(&f))
        || source_asset.is_some_and(|text| text.to_lowercase().contains(&f))
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
) -> bool {
    (show_archived || row.active)
        && (show_model_variables || row.exposure == SignalExposure::Public)
        && (!scoped || row.in_focus)
        && filter_match(
            filter,
            label,
            &row.sig.path,
            row.description.as_deref(),
            row.model_class.as_deref(),
            row.model_variable.as_deref(),
            row.source_asset.as_deref(),
        )
}

/// Display the authored/operator channel name.  Public rows stay concise; the
/// exact Modelica class, variable, and source asset remain in the row detail
/// tooltip.  Internal rows retain their generated identity so implementation
/// channels cannot collapse into apparent duplicates.
fn telemetry_row_label(row: &Row, show_generated_names: bool) -> String {
    if row.exposure == SignalExposure::Internal && !show_generated_names {
        // The tree already identifies the authored component.  Show the
        // Modelica variable here and keep the exact solver address in the
        // detail strip, so inspecting internal state does not require reading
        // a generated namespace.
        let variable = row
            .model_variable
            .as_deref()
            .or_else(|| row.sig.path.rsplit('.').next())
            .map(|variable| operator_identifier_label(variable, row.unit.as_deref()))
            .unwrap_or_else(|| "state".to_string());
        return format!("{variable} · internal");
    }
    let base = display_channel_label(
        &row.sig.path,
        row.group_path.as_deref(),
        row.unit.as_deref(),
        show_generated_names,
    );
    if show_generated_names {
        return base;
    }
    base
}

fn tree_any_row(node: &TreeNode, predicate: impl Fn(&Row) -> bool + Copy) -> bool {
    node.rows.iter().any(predicate)
        || node
            .children
            .values()
            .any(|child| tree_any_row(child, predicate))
}

fn visible_count(
    node: &TreeNode,
    scoped: bool,
    show_model_variables: bool,
    show_archived: bool,
    filter: &str,
) -> usize {
    node.rows
        .iter()
        .filter(|r| {
            row_visible(
                r,
                scoped,
                show_model_variables,
                show_archived,
                filter,
                &node.label,
            )
        })
        .count()
        + node
            .children
            .values()
            .map(|c| visible_count(c, scoped, show_model_variables, show_archived, filter))
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
    show_model_variables: bool,
    show_archived: bool,
    display_settings: &TelemetryDisplaySettings,
    filter: &str,
    selected: Option<&SignalRef>,
    depth: usize,
    clicked: &mut Option<SignalRef>,
) {
    let visible = visible_count(node, scoped, show_model_variables, show_archived, filter);
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
                show_model_variables,
                show_archived,
                display_settings,
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
                for row in node.rows.iter().filter(|r| {
                    row_visible(
                        r,
                        scoped,
                        show_model_variables,
                        show_archived,
                        filter,
                        &node.label,
                    )
                }) {
                    let latest = registry
                        .scalar_history(&row.sig)
                        .and_then(|h| h.samples.back())
                        .map(|s| s.value);
                    let channel_label =
                        telemetry_row_label(row, display_settings.show_generated_names);
                    let payload = ChannelDragPayload::from_signal(&row.sig);
                    let inner = ui.dnd_drag_source(
                        ui.id().with(("tb_row", &row.sig)),
                        payload.clone(),
                        |ui| {
                            ui.selectable_label(
                                selected == Some(&row.sig),
                                egui::RichText::new(if row.active {
                                    channel_label.clone()
                                } else {
                                    format!("{channel_label} (archived)")
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
                                latest
                                    .map(|value| fmt_value(value, display_settings))
                                    .unwrap_or_else(|| "—".into()),
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
    let mut text = if exponent >= 7 || exponent < -4 {
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
    catalog: Catalog,
    selected: Option<SignalRef>,
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

impl Default for TelemetryBrowserPanel {
    fn default() -> Self {
        Self {
            filter: String::new(),
            catalog: Catalog::default(),
            selected: None,
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
        let stored_display_settings = ctx.resource_expect::<TelemetryDisplaySettings>().clone();
        let mut display_settings = stored_display_settings.clone();

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
            ui.checkbox(&mut self.show_model_variables, "Internal variables")
                .on_hover_text(
                    "Include generated Modelica inputs, connector values, and component state. \
                     Canonical USD-facing channels remain visible; implementation rows are \
                     marked internal and keep their generated identity.",
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
                        .small()
                        .color(subdued),
                );
            });
        });
        if display_settings != stored_display_settings {
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
                |e| {
                    let label = lunco_core::entity_display_name(
                        ctx.get::<Name>(e),
                        ctx.get::<lunco_core::markers::Callsign>(e),
                        ctx.get::<lunco_core::CatalogEntryId>(e),
                    );
                    (!label.is_empty()).then_some(label)
                },
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
        if scoped
            && !tree_any_row(&self.catalog.root, |row| {
                row.in_focus && (row.active || display_settings.show_archived)
            })
        {
            ui.label(
                egui::RichText::new(
                    "The selection publishes no telemetry yet — start its simulation or untick \
                     “Selected only” to inspect the rest of the live scene.",
                )
                .color(subdued),
            );
            return;
        }
        if visible_count(
            &self.catalog.root,
            scoped,
            self.show_model_variables,
            display_settings.show_archived,
            &self.filter,
        ) == 0
        {
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
            let public_count = visible_count(
                &self.catalog.root,
                scoped,
                false,
                display_settings.show_archived,
                &self.filter,
            );
            let complete_count = visible_count(
                &self.catalog.root,
                scoped,
                true,
                display_settings.show_archived,
                &self.filter,
            );
            let hidden_count = complete_count.saturating_sub(public_count);
            if hidden_count > 0 {
                ui.label(
                    egui::RichText::new(format!(
                        "{hidden_count} internal variables hidden — enable Internal variables to inspect the complete model state."
                    ))
                    .small()
                    .color(subdued),
                );
            }
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
                        self.show_model_variables,
                        display_settings.show_archived,
                        &display_settings,
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
        if !display_settings.show_archived && !registry.is_active(&sel) {
            self.selected = None;
            return;
        }
        ui.separator();
        let metadata = registry.meta(&sel);
        if !self.show_model_variables
            && metadata.is_some_and(|meta| meta.exposure == SignalExposure::Internal)
        {
            self.selected = None;
            return;
        }
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
            ui.label(egui::RichText::new(description).small().color(subdued));
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
    use crate::signal::compact_channel_label;
    use crate::signal::SignalRef;

    fn ent(n: u32) -> Entity {
        Entity::from_raw_u32(n).unwrap()
    }

    #[test]
    fn telemetry_labels_keep_public_names_concise_and_internal_names_distinct() {
        let public = Row {
            sig: SignalRef::new(ent(1), "electrical_power"),
            unit: None,
            description: None,
            provenance: None,
            group_path: None,
            model_class: Some("LunCo.Electrical.DCMotor".into()),
            model_variable: Some("electrical_power".into()),
            source_asset: None,
            canonical_name: None,
            exposure: SignalExposure::Public,
            in_focus: false,
            active: true,
        };
        assert_eq!(telemetry_row_label(&public, false), "electrical power");

        let mut internal = public.clone();
        internal.sig = SignalRef::new(
            ent(1),
            "__member_Traverse_x2f_Rover_x2f_Motor_L0.electrical_power",
        );
        internal.exposure = SignalExposure::Internal;
        assert_eq!(
            telemetry_row_label(&internal, false),
            "electrical power · internal"
        );
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
        assert!(!row_visible(&rows[0], false, true, false, "", "Rover"));
        assert!(row_visible(&rows[0], false, true, true, "", "Rover"));
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
        assert!(filter_match(
            "",
            "Rover",
            "wheel.speed",
            None,
            None,
            None,
            None
        ));
        assert!(filter_match(
            "SPEED",
            "Rover",
            "wheel.speed",
            None,
            None,
            None,
            None
        ));
        assert!(filter_match(
            "rov",
            "Rover",
            "wheel.speed",
            None,
            None,
            None,
            None
        ));
        assert!(!filter_match(
            "thrust",
            "Rover",
            "wheel.speed",
            None,
            None,
            None,
            None
        ));
        assert!(filter_match(
            "camerapayload",
            "power draw",
            "science_power",
            None,
            Some("LunCo.Electrical.CameraPayload"),
            Some("power_draw_w"),
            Some("lunco://models/LunCo/Electrical/CameraPayload.mo"),
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
            |_| false,
        );

        let rover = &tree.children["/SandboxScene"].children["/SandboxScene/Rover"];
        assert!(
            !rover.children.contains_key("/SandboxScene/Rover/Power"),
            "the removed domain child must not reappear as a telemetry node"
        );
        let motor = &rover.children["/SandboxScene/Rover/Motor_FL"];
        assert_eq!(motor.label, "Motor FL");
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
            |_| false,
        );
        let rover = &tree.children["/Traverse"].children["/Traverse/Rover"];
        let motor = &rover.children["/Traverse/Rover/Motor_L0"];
        assert_eq!(motor.rows.len(), 2);
        assert!(!rover.children.contains_key("/Traverse/Rover/Power"));
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
            unit: Some("V".into()),
            description: Some("Electrical pin voltage".into()),
            provenance: Some("modelica".into()),
            group_path: Some("/Rover/Battery".into()),
            model_class: Some("LunCo.Electrical.Battery".into()),
            model_variable: Some("p.v".into()),
            source_asset: None,
            canonical_name: None,
            exposure: SignalExposure::Internal,
            in_focus: false,
            active: true,
        };
        assert_eq!(
            telemetry_row_label(&internal, false),
            "pin voltage · internal"
        );
        internal.model_variable = Some("p.i".into());
        internal.unit = Some("A".into());
        assert_eq!(
            telemetry_row_label(&internal, false),
            "pin current · internal"
        );
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
