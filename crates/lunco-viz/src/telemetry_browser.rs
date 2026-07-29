//! Telemetry channel browser — the real replacement for the deleted
//! `lunco-ui/src/telemetry.rs` tombstone panel.
//!
//! Lists every scalar channel in the [`SignalRegistry`], grouped by
//! owning entity, with unit (from [`crate::signal::SignalMeta`]) and
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

use bevy::prelude::{ChildOf, Entity, Name};
use bevy_egui::egui;
use egui_plot::{Line, Plot, PlotPoints};
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

// ── Cached catalog ───────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Row {
    sig: SignalRef,
    unit: Option<String>,
}

#[derive(Clone, Debug)]
struct Group {
    label: String,
    rows: Vec<Row>,
    /// True when this group's owning entity is the focused root or a descendant
    /// of one (see [`TelemetryFocus`]). Resolved at build time — the ancestor
    /// walk is not something to redo per frame per row.
    in_focus: bool,
}

#[derive(Default)]
struct Catalog {
    key: u64,
    /// [`TelemetryFocus::fingerprint`] the groups' `in_focus` flags were built
    /// against. Selecting a different rover changes no channel, so the channel-set
    /// key alone would leave every flag stale.
    focus_key: u64,
    groups: Vec<Group>,
}

/// Order-independent fingerprint of the channel *set*. Sample pushes
/// don't move it (contents aren't hashed); channels appearing or
/// disappearing do. O(#channels) hashing per frame, no allocation.
fn catalog_key(reg: &SignalRegistry) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut acc: u64 = 0;
    let mut n: u64 = 0;
    for (sig, _hist) in reg.iter_scalar() {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        sig.hash(&mut h);
        acc ^= h.finish();
        n += 1;
    }
    acc ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// Rebuild the grouped/sorted catalog. `name_of` resolves an entity's
/// display name (the panel passes a `PanelCtx::get::<Name>` closure —
/// O(1) per entity, not a scan).
fn build_groups(
    reg: &SignalRegistry,
    name_of: impl Fn(Entity) -> Option<String>,
    in_focus: impl Fn(Entity) -> bool,
) -> Vec<Group> {
    use std::collections::HashMap;
    let mut by_entity: HashMap<Entity, Vec<Row>> = HashMap::new();
    for (sig, _hist) in reg.iter_scalar() {
        let unit = reg.meta(sig).and_then(|m| m.unit.clone());
        by_entity.entry(sig.entity).or_default().push(Row {
            sig: sig.clone(),
            unit,
        });
    }
    let mut groups: Vec<Group> = by_entity
        .into_iter()
        .map(|(entity, mut rows)| {
            rows.sort_by(|a, b| a.sig.path.cmp(&b.sig.path));
            let label = if entity == Entity::PLACEHOLDER {
                "Global".to_string()
            } else {
                name_of(entity).unwrap_or_else(|| format!("Entity {entity}"))
            };
            Group {
                label,
                rows,
                in_focus: entity != Entity::PLACEHOLDER && in_focus(entity),
            }
        })
        .collect();
    groups.sort_by(|a, b| a.label.cmp(&b.label));
    groups
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

/// Case-insensitive substring filter over `path` and group label.
fn filter_match(filter: &str, group_label: &str, path: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let f = filter.to_lowercase();
    path.to_lowercase().contains(&f) || group_label.to_lowercase().contains(&f)
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
}

impl Default for TelemetryBrowserPanel {
    fn default() -> Self {
        Self {
            filter: String::new(),
            catalog: Catalog::default(),
            selected: None,
            preview: None,
            focus_only: true,
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
        });
        ui.separator();

        let Some(registry) = ctx.resource::<SignalRegistry>() else {
            ui.label(egui::RichText::new("SignalRegistry not installed.").color(subdued));
            return;
        };

        // ── Change-driven catalog rebuild ────────────────────────
        // Two independent invalidators: the channel SET (a sim started, a vessel
        // spawned) and the FOCUS (the user clicked a different rover — same channels,
        // different membership).
        let key = catalog_key(registry);
        if self.catalog.key != key
            || self.catalog.focus_key != focus_key
            || (self.catalog.groups.is_empty() && key != 0)
        {
            let groups = build_groups(
                registry,
                |e| ctx.get::<Name>(e).map(|n| n.as_str().to_string()),
                |e| entity_in_focus(e, &focus, |c| ctx.get::<ChildOf>(c).map(|p| p.parent())),
            );
            self.catalog = Catalog {
                key,
                focus_key,
                groups,
            };
        }

        if self.catalog.groups.is_empty() {
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
        if scoped && !self.catalog.groups.iter().any(|g| g.in_focus) {
            ui.label(
                egui::RichText::new(
                    "The selection publishes no telemetry — author `lunco:telemetry` \
                     on its prims, or untick “Selected only”.",
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
                for group in &self.catalog.groups {
                    if scoped && !group.in_focus {
                        continue;
                    }
                    let visible: Vec<&Row> = group
                        .rows
                        .iter()
                        .filter(|r| filter_match(&self.filter, &group.label, &r.sig.path))
                        .collect();
                    if visible.is_empty() {
                        continue;
                    }
                    egui::CollapsingHeader::new(
                        egui::RichText::new(format!("{} ({})", group.label, visible.len()))
                            .strong(),
                    )
                    .id_salt(("tb_group", &group.label))
                    .default_open(true)
                    .show(ui, |ui| {
                        // THREE ALIGNED COLUMNS — channel, value, unit.
                        //
                        // The rows used to be free `horizontal` layouts with the
                        // value right-pushed and the unit glued onto it, so
                        // nothing lined up: every number sat at a different x, and
                        // `0.900 1` read as one token. A `Grid` gives the value
                        // column a shared right edge (numbers compare by eye) and
                        // gives the unit its own column, which is also what lets a
                        // dimensionless channel render an EMPTY unit cell instead
                        // of a stray `1`.
                        egui::Grid::new(("tb_grid", &group.label))
                            .num_columns(3)
                            .striped(true)
                            .spacing(egui::vec2(theme.spacing.item_spacing, 2.0))
                            .show(ui, |ui| {
                                for row in visible {
                                    let latest = registry
                                        .scalar_history(&row.sig)
                                        .and_then(|h| h.samples.back())
                                        .map(|s| s.value);
                                    let is_sel = self.selected.as_ref() == Some(&row.sig);
                                    let payload = ChannelDragPayload::from_signal(&row.sig);
                                    let drag_id = ui.id().with(("tb_row", &row.sig));

                                    // Column 1 — the channel, and the drag handle.
                                    let inner =
                                        ui.dnd_drag_source(drag_id, payload.clone(), |ui| {
                                            ui.selectable_label(
                                                is_sel,
                                                egui::RichText::new(&row.sig.path)
                                                    .color(theme.tokens.text),
                                            )
                                        });
                                    let resp = inner.inner;

                                    // Column 2 — the number, monospaced and
                                    // right-aligned so digits stack by place value.
                                    let value_text = match latest {
                                        Some(v) => fmt_value(v),
                                        None => "—".to_string(),
                                    };
                                    let stale = latest.is_none();
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new(value_text).monospace().color(
                                                    if stale { subdued } else { theme.tokens.text },
                                                ),
                                            )
                                            .on_hover_text(match latest {
                                                // Full precision on demand: the cell
                                                // shows four significant digits, and
                                                // "is that really zero" is a question
                                                // the tooltip should answer.
                                                Some(v) => format!("{v}"),
                                                None => "no samples yet".to_string(),
                                            });
                                        },
                                    );

                                    // Column 3 — the unit, in its own column so the
                                    // value column keeps a single right edge.
                                    let unit_cell = pretty_unit(row.unit.as_deref());
                                    let unit_label = ui.label(
                                        egui::RichText::new(unit_cell).small().color(subdued),
                                    );
                                    let tip = unit_tooltip(row.unit.as_deref());
                                    if !tip.is_empty() {
                                        unit_label.on_hover_text(tip);
                                    }
                                    ui.end_row();

                                    if resp.double_clicked() {
                                        // Fallback door: queue a plot-node
                                        // request; the canvas host drains it
                                        // and places the node.
                                        queue_plot_drop(
                                            ui.ctx(),
                                            PlotDropRequest {
                                                payload: payload.clone(),
                                                world_pos: None,
                                            },
                                        );
                                        clicked = Some(row.sig.clone());
                                    } else if resp.clicked() {
                                        clicked = Some(row.sig.clone());
                                    }
                                    resp.on_hover_ui(|ui| {
                                        ui.label(
                                            egui::RichText::new(&row.sig.path).strong().monospace(),
                                        );
                                        ui.label(
                                            egui::RichText::new(
                                                "drag onto a canvas to plot; \
                                                 double-click to add at a default spot",
                                            )
                                            .small()
                                            .weak(),
                                        );
                                    });
                                }
                            });
                    });
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
        let unit = registry.meta(&sel).and_then(|m| m.unit.clone());
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
                    .height(80.0)
                    .show_axes([false, false])
                    .show_grid(false)
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
            let cfg = VisualizationConfig {
                id: VizId::next(),
                title: sel.path.clone(),
                kind: LINE_PLOT_KIND,
                view: ViewTarget::Panel2D,
                inputs: vec![SignalBinding {
                    source: sel.clone(),
                    role: "y".to_string(),
                    label: None,
                    color: None,
                    visible: true,
                }],
                style: serde_json::Value::Null,
            };
            let instance = cfg.id.raw();
            ctx.defer(move |world| {
                world.resource_mut::<VisualizationRegistry>().insert(cfg);
            });
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
    fn catalog_key_ignores_pushes_but_sees_membership() {
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

        // Channel removed: key moves back to the original set's key.
        reg.remove_signal(&SignalRef::new(ent(2), "b"));
        assert_eq!(k1, catalog_key(&reg), "key is a pure set fingerprint");
    }

    #[test]
    fn groups_are_per_entity_sorted_and_carry_units() {
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

        let groups = build_groups(
            &reg,
            |e| (e == ent(1)).then(|| "Alpha Rover".to_string()),
            |_| false,
        );
        assert_eq!(groups.len(), 2);
        // Sorted by label: "Alpha Rover" < "Entity …".
        assert_eq!(groups[0].label, "Alpha Rover");
        assert_eq!(groups[0].rows.len(), 1);
        assert_eq!(groups[1].rows.len(), 2);
        // Rows sorted by path within the group.
        assert_eq!(groups[1].rows[0].sig.path, "a.torque");
        assert_eq!(groups[1].rows[1].sig.path, "z.speed");
        assert_eq!(groups[1].rows[1].unit.as_deref(), Some("m/s"));
    }

    #[test]
    fn filter_matches_path_and_group_case_insensitively() {
        assert!(filter_match("", "Rover", "wheel.speed"));
        assert!(filter_match("SPEED", "Rover", "wheel.speed"));
        assert!(filter_match("rov", "Rover", "wheel.speed"));
        assert!(!filter_match("thrust", "Rover", "wheel.speed"));
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
    fn groups_carry_focus_membership() {
        let mut reg = SignalRegistry::default();
        reg.push_scalar(SignalRef::new(ent(1), "a"), 0.0, 1.0);
        reg.push_scalar(SignalRef::new(ent(2), "b"), 0.0, 1.0);
        let groups = build_groups(&reg, |_| None, |e| e == ent(1));
        let focused: Vec<bool> = groups.iter().map(|g| g.in_focus).collect();
        assert_eq!(focused.iter().filter(|f| **f).count(), 1);
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
        assert_eq!(fmt_value(0.9), "0.900");
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
