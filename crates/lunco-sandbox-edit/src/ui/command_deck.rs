//! Command Deck panel — the control surface for the currently-selected vessel.
//!
//! Composes three pieces of state into one Workbench panel (Layer 4, pure
//! reader — all mutations dispatch typed commands per §4.2):
//!
//! - **Selection** — the primary `SelectedEntities` entry is the vessel the
//!   deck addresses. (No selection → the panel renders an idle hint.)
//! - **Possession** — read from the avatar's `ControllerLink`. Shows "Driving:
//!   <vessel>" when the selected vessel is possessed, else "Free flight".
//! - **Behaviour / route** — reads the vessel's `AutopilotBehaviorSpec`, which is
//!   DERIVED from its BT.CPP mission + the waypoint prims it references. The route
//!   readout is therefore strictly read-only: a waypoint is edited in the scene (drag
//!   the pin, press Delete), not from a list here. Authoring is Ctrl+LMB in the
//!   viewport, rhai, or the `.usda` / Groot2 directly.
//!
//! Buttons emit the EXISTING typed commands — `PossessVessel`, `ReleaseVessel`,
//! `EngageAutopilot`, `DisengageAutopilot`. One input shape, every surface (§4.2):
//! the same verbs the rhai prelude and the HTTP API expose.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_autopilot::{Autopilot, AutopilotBehaviorSpec, BehaviorSpec};
use lunco_controller::ControllerLink;
use lunco_core::{Avatar, GlobalEntityId};
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};


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
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Tools
    }

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
                        world.trigger(lunco_avatar::PossessVessel { avatar: None, target: v, bind_camera: true });
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
            // Alt, NOT Ctrl: `on_scene_click_checkpoint` gates on AltLeft/AltRight
            // (plain click possesses, Shift+click selects). This hint is the only
            // place most users learn the gesture — if it names the wrong modifier
            // they cannot give a rover a route at all, and the autopilot then looks
            // broken for every vessel that did not ship with waypoints.
            ui.weak("none — Alt+click the ground to add a checkpoint");
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
                        // panel is displaying. Read the spec mirror off the vessel
                        // and pass it.
                        let spec_json = world
                            .get::<AutopilotBehaviorSpec>(v)
                            .and_then(|s| s.to_json().ok())
                            .unwrap_or_default();
                        // NO throttle. "Engage autopilot" means "run your route" —
                        // it never means "drive forward". A vessel with no route
                        // holds; sending a cruise setpoint from here made a
                        // routeless rover leave in a straight line.
                        world.trigger(lunco_autopilot::EngageAutopilot {
                            vessel: v,
                            index: 0,
                            throttle: 0.0,
                            spec_json,
                        });
                    });
                }
            }
        });

        // ── Route readout ────────────────────────────────────────────────
        // READ-ONLY. A waypoint is a USD prim, so it is edited in the scene, not in
        // this list: select the pin to move it with the transform gizmo, or press
        // Delete to remove it. There is no delete button here because there is no
        // checkpoint command to call — the prim's own Delete path is the verb.
        if view.is_patrol && !view.patrol.is_empty() {
            ui.separator();
            ui.label("Route");
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, wp) in view.patrol.iter().enumerate() {
                    // Action marker: 🛰 when this waypoint fires a tool on arrival, so
                    // a geometry-only pin is distinguishable from an armed one.
                    let marker = view
                        .patrol_actions
                        .get(i)
                        .filter(|n| **n > 0)
                        .map(|n| format!(" 🛰×{}", n))
                        .unwrap_or_default();
                    ui.label(format!(
                        "{}.  [{:.1}, {:.1}, {:.1}]{marker}",
                        i + 1,
                        wp[0],
                        wp[1],
                        wp[2]
                    ));
                }
            });
        }

        ui.separator();
        ui.small("Ctrl+Left-click ground: drop a waypoint · click a pin to select, drag to move, Delete to remove");
    }
}