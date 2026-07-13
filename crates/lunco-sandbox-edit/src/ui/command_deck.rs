//! Command Deck panel — the control surface for the currently-selected vessel.
//!
//! Composes three pieces of state into one Workbench panel (Layer 4, pure
//! reader — all mutations dispatch typed commands per §4.2):
//!
//! - **Selection** — the primary `SelectedEntities` entry is the vessel the
//!   deck addresses. (No selection → the panel renders an idle hint.)
//! - **Possession** — read from the avatar's `ControllerLink`. Shows "Driving:
//!   <vessel>" when the selected vessel is possessed, else "Free flight".
//! - **Behaviour / checkpoints** — reads `AutopilotBehaviorSpec` on the vessel
//!   so the user sees the live patrol (the same data the path-line gizmo
//!   draws) and can clear it. Authoring happens in the 3D viewport (Ctrl+LMB)
//!   or via rhai (`patrol.rhai`); this panel is the read+control surface.
//!
//! Buttons emit the EXISTING typed commands — `PossessVessel`, `ReleaseVessel`,
//! `EngageAutopilot`, `SetAutopilotBehavior` (via `DeleteCheckpoint` /
//! `ClearPatrol` thin wrappers in [`crate::ui::checkpoint_click`]). One input
//! shape, every surface (§4.2): the same verbs the rhai prelude and the HTTP
//! API expose.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_autopilot::{Autopilot, AutopilotBehaviorSpec, BehaviorSpec};
use lunco_controller::ControllerLink;
use lunco_core::{Avatar, GlobalEntityId};
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

use crate::ui::checkpoint_click::{DeleteCheckpoint, CheckpointContextMenu};
use crate::SelectedEntities;

/// Change-driven view-model for the Command Deck. Reads selection, possession
/// and the behaviour spec each `Update` (single-entity O(1) lookups — the
/// sanctioned live-readout exception to §7; no scans). See
/// [`populate_command_deck_view`].
#[derive(Resource, Default, Clone)]
pub struct CommandDeckView {
    /// The primary selected entity (the vessel the deck addresses).
    pub selected: Option<Entity>,
    /// Display label for the selection.
    pub selected_label: String,
    /// True when the local avatar's `ControllerLink` points at `selected`.
    pub driving: bool,
    /// True when an `Autopilot` actor exists for this vessel.
    pub autopilot_engaged: bool,
    /// Patrol waypoints read off the `AutopilotBehaviorSpec` (empty for
    /// non-patrol or no spec).
    pub patrol: Vec<[f32; 3]>,
    /// Count of arrival actions per waypoint (parallel to `patrol`). A mission
    /// authors these in rhai/USD; the deck shows a 🛠 marker when non-zero so
    /// the user can see a waypoint isn't just a geometry pin.
    pub patrol_actions: Vec<u32>,
    /// Whether the spec on the vessel is a patrol (else the panel shows
    /// "behaviour: <kind>").
    pub is_patrol: bool,
    /// Behavour kind label when not a patrol (e.g. "brake", "cruise").
    pub behaviour_kind: String,
}

/// Producer for [`CommandDeckView`]. Runs every `Update` (cheap O(1) reads).
pub fn populate_command_deck_view(
    mut view: ResMut<CommandDeckView>,
    selected: Res<SelectedEntities>,
    avatars: Query<Entity, With<Avatar>>,
    q_link: Query<&ControllerLink>,
    q_autopilot: Query<&Autopilot>,
    q_spec: Query<&AutopilotBehaviorSpec>,
    q_name: Query<&Name>,
    q_gid: Query<&GlobalEntityId>,
) {
    let sel = selected.primary();
    view.selected = sel;
    view.selected_label = sel
        .and_then(|e| q_name.get(e).ok())
        .map(|n| n.as_str().to_string())
        .or_else(|| sel.and_then(|e| q_gid.get(e).ok()).map(|g| format!("vessel #{}", g.get())))
        .unwrap_or_default();
    // Possession: the avatar's ControllerLink points at the vessel it drives.
    view.driving = match (sel, avatars.iter().next()) {
        (Some(v), Some(av)) => q_link.get(av).ok().map(|l| l.vessel_entity == v).unwrap_or(false),
        _ => false,
    };
    // Autopilot + spec.
    view.autopilot_engaged = sel.map(|v| q_autopilot.iter().any(|a| a.vessel == v)).unwrap_or(false);
    view.is_patrol = false;
    view.patrol.clear();
    view.patrol_actions.clear();
    view.behaviour_kind.clear();
    if let Some(v) = sel {
        if let Ok(spec) = q_spec.get(v) {
            match &spec.0 {
                BehaviorSpec::Patrol { waypoints, .. } => {
                    view.is_patrol = true;
                    // Project to positions for the list; the count of arrival
                    // actions per waypoint is surfaced via `patrol_actions`.
                    view.patrol = waypoints.iter().map(|w| w.pos).collect();
                    view.patrol_actions = waypoints.iter().map(|w| w.on_arrival.len() as u32).collect();
                }
                other => {
                    // Variant NAME only. Most `BehaviorSpec` variants are struct
                    // variants, so Debug emits `DriveTo { target: [..] }` —
                    // splitting on '(' alone would leak the whole field dump into
                    // the label. Cut at the first delimiter of either kind.
                    let dbg = format!("{other:?}");
                    view.behaviour_kind = dbg
                        .split(|c: char| c == '(' || c == '{' || c.is_whitespace())
                        .next()
                        .unwrap_or("?")
                        .to_lowercase();
                }
            }
        }
    }
}

/// The Command Deck panel.
pub struct CommandDeck;

impl Panel for CommandDeck {
    fn id(&self) -> PanelId { PanelId("command_deck") }
    fn title(&self) -> String { "Command Deck".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        ui.heading("Command Deck");
        ui.separator();

        // Semantic status colours from the active Theme (§3.1 — no hex literals
        // outside `lunco-theme`). Fall back to egui's default text colour when
        // headless / no theme registered.
        let (success_col, warning_col) = ctx
            .resource::<lunco_theme::Theme>()
            .map(|t| (t.tokens.success, t.tokens.warning))
            .unwrap_or((egui::Color32::PLACEHOLDER, egui::Color32::PLACEHOLDER));

        let Some(view) = ctx.resource::<CommandDeckView>().cloned() else {
            ui.label("(no view)");
            return;
        };

        // ── Selection + possession status ────────────────────────────────
        let Some(vessel) = view.selected else {
            ui.label(
                egui::RichText::new("Select a vessel (Shift+click in the 3D view)")
                    .italics()
                    .weak(),
            );
            return;
        };

        ui.horizontal(|ui| {
            ui.label("Vessel:");
            if view.selected_label.is_empty() {
                ui.weak(format!("{:?}", vessel));
            } else {
                ui.strong(&view.selected_label);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Status:");
            if view.driving {
                ui.colored_label(success_col, "Driving (you)");
            } else if view.autopilot_engaged {
                ui.colored_label(warning_col, "Autopilot");
            } else {
                ui.weak("Free flight — click it to drive");
            }
        });

        ui.separator();

        // ── Possession / release ──────────────────────────────────────────
        ui.horizontal(|ui| {
            let vessel = vessel;
            if view.driving {
                if ui.button("Release control").clicked() {
                    let v = vessel;
                    ctx.defer(move |world| {
                        world.trigger(lunco_avatar::ReleaseVessel { target: v });
                    });
                }
            } else {
                if ui.button("🏁 Take control").clicked() {
                    let v = vessel;
                    ctx.defer(move |world| {
                        world.trigger(lunco_avatar::PossessVessel { avatar: None, target: v });
                    });
                }
            }
        });

        ui.separator();

        // ── Autopilot / behaviour ────────────────────────────────────────
        ui.label("Behaviour");
        if view.is_patrol {
            ui.label(format!("Patrol — {} checkpoint(s)", view.patrol.len()));
        } else if !view.behaviour_kind.is_empty() {
            ui.label(format!("{}", view.behaviour_kind));
        } else {
            ui.weak("none — Ctrl+click the ground to add a checkpoint");
        }

        ui.horizontal(|ui| {
            let v = vessel;
            if view.autopilot_engaged {
                if ui.button("Disengage autopilot").clicked() {
                    ctx.defer(move |world| {
                        // Disengage: brake the tree but KEEP the patrol data
                        // (distinct from ClearPatrol, which wipes it). A later
                        // re-engage restores the route.
                        world.trigger(lunco_autopilot::DisengageAutopilot { vessel: v });
                    });
                }
            } else {
                if ui.button("Engage autopilot").clicked() {
                    ctx.defer(move |world| {
                        // Engage with the vessel's OWN behaviour — the patrol this
                        // panel is displaying. An empty `spec_json` makes
                        // `on_engage_autopilot` fall back to a constant forward
                        // setpoint, i.e. the rover would drive blindly straight
                        // ahead while the deck claimed it was running an N-point
                        // patrol. Read the spec mirror off the vessel and pass it.
                        let spec_json = world
                            .get::<AutopilotBehaviorSpec>(v)
                            .and_then(|s| s.to_json().ok())
                            .unwrap_or_default();
                        // Throttle from the tunable resource, not a literal (§3).
                        let throttle = world
                            .get_resource::<crate::checkpoint_gizmo::PatrolDefaults>()
                            .copied()
                            .unwrap_or_default()
                            .engage_throttle;
                        world.trigger(lunco_autopilot::EngageAutopilot {
                            vessel: v,
                            index: 0,
                            throttle,
                            spec_json,
                        });
                    });
                }
            }
        });

        // ── Checkpoint list ──────────────────────────────────────────────
        if view.is_patrol && !view.patrol.is_empty() {
            ui.separator();
            ui.label("Checkpoints");
            let mut to_delete: Option<u32> = None;
            let mut to_clear = false;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, wp) in view.patrol.iter().enumerate() {
                    ui.horizontal(|ui| {
                        // Action marker: 🛰 when this waypoint fires a tool on
                        // arrival (authored in rhai/USD), so a geometry-only
                        // Ctrl+LMB pin is distinguishable from an armed one.
                        let marker = view
                            .patrol_actions
                            .get(i)
                            .filter(|n| **n > 0)
                            .map(|n| format!(" 🛰×{}", n))
                            .unwrap_or_default();
                        ui.label(format!("{}.  [{:.1}, {:.1}, {:.1}]{marker}", i + 1, wp[0], wp[1], wp[2]));
                        if ui.small_button("🗑").on_hover_text("Delete this checkpoint").clicked() {
                            to_delete = Some(i as u32);
                        }
                    });
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Clear patrol").clicked() {
                    to_clear = true;
                }
            });
            // Defer the deletes AFTER the egui borrow ends.
            if let Some(idx) = to_delete {
                let v = vessel;
                ctx.defer(move |world| {
                    world.trigger(DeleteCheckpoint { vessel: v, index: idx });
                });
            }
            if to_clear {
                let v = vessel;
                ctx.defer(move |world| {
                    // Clear patrol: the single canonical verb — brakes the tree
                    // AND removes the spec mirror so the gizmo/deck update. No
                    // hand-built Brake JSON (§4.2 — one input shape, every surface).
                    world.trigger(lunco_autopilot::ClearPatrol { vessel: v });
                    // Also close any open context menu.
                    if let Some(mut m) = world.get_resource_mut::<CheckpointContextMenu>() {
                        *m = CheckpointContextMenu::Closed;
                    }
                });
            }
        }

        ui.separator();
        ui.small(
            "Ctrl+Left-click ground: add checkpoint · Right-click pin: delete",
        );
    }
}