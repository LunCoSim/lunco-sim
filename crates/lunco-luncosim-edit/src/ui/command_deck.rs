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
//!   the pin, press Delete), not from a list here. Authoring is the
//!   PlaceWaypoint intent + LMB (Alt+LMB in the bundled keymap) in the
//!   viewport, Groot2, or the `.usda` directly. The full mission topology is
//!   available in the `Autopilot graph` canvas.
//!
//! Buttons emit the EXISTING typed commands — `PossessVessel`, `ReleaseVessel`,
//! `EngageAutopilot`, `DisengageAutopilot`. One input shape, every surface (§4.2):
//! the same verbs the rhai prelude and the HTTP API expose.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_autopilot::{Autopilot, AutopilotBehaviorSpec, BehaviorSpec};
use lunco_controller::ControllerLink;
use lunco_core::{GlobalEntityId, TheLocalAvatar};
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

use lunco_scene_commands::SelectedEntities;

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
    pub patrol: Vec<[f64; 3]>,
    /// Count of arrival actions per waypoint (parallel to `patrol`). A mission
    /// authors these in rhai/USD; the deck shows a 🛠 marker when non-zero so
    /// the user can see a waypoint isn't just a geometry pin.
    pub patrol_actions: Vec<u32>,
    /// Whether the spec on the vessel is a patrol (else the panel shows
    /// "behaviour: <kind>").
    pub is_patrol: bool,
    /// Behavour kind label when not a patrol (e.g. "brake", "cruise").
    pub behaviour_kind: String,
    /// Authored behaviour spec forwarded to the typed engage command.
    pub spec_json: String,
}

/// Producer for [`CommandDeckView`]. Runs every `Update` (cheap O(1) reads).
pub fn populate_command_deck_view(
    mut view: ResMut<CommandDeckView>,
    selected: Res<SelectedEntities>,
    local_avatar: Res<TheLocalAvatar>,
    q_link: Query<&ControllerLink>,
    q_autopilot: Query<&Autopilot>,
    q_spec: Query<&AutopilotBehaviorSpec>,
    q_name: Query<&Name>,
    q_callsign: Query<&lunco_core::markers::Callsign>,
    q_catalog_id: Query<&lunco_core::CatalogEntryId>,
    q_gid: Query<&GlobalEntityId>,
) {
    let sel = selected.primary();
    view.selected = sel;
    view.selected_label = sel
        .map(|e| {
            lunco_core::entity_display_name(
                q_name.get(e).ok(),
                q_callsign.get(e).ok(),
                q_catalog_id.get(e).ok(),
            )
        })
        .filter(|label| !label.is_empty())
        .or_else(|| {
            sel.and_then(|e| q_gid.get(e).ok())
                .map(|g| format!("vessel #{}", g.get()))
        })
        .unwrap_or_default();
    // Possession: the avatar's ControllerLink points at the vessel it drives.
    view.driving = match (sel, local_avatar.0) {
        (Some(v), Some(avatar)) => q_link
            .get(avatar)
            .ok()
            .map(|l| l.vessel_entity == v)
            .unwrap_or(false),
        _ => false,
    };
    // Autopilot + spec.
    view.autopilot_engaged = sel
        .map(|v| q_autopilot.iter().any(|a| a.vessel == v))
        .unwrap_or(false);
    view.is_patrol = false;
    view.patrol.clear();
    view.patrol_actions.clear();
    view.behaviour_kind.clear();
    view.spec_json.clear();
    if let Some(v) = sel {
        if let Ok(spec) = q_spec.get(v) {
            view.spec_json = spec.to_json().unwrap_or_default();
            match &spec.0 {
                BehaviorSpec::Patrol { waypoints, .. } => {
                    view.is_patrol = true;
                    // Project to positions for the list; the count of arrival
                    // actions per waypoint is surfaced via `patrol_actions`.
                    view.patrol = waypoints.iter().map(|w| w.pos).collect();
                    view.patrol_actions = waypoints
                        .iter()
                        .map(|w| w.on_arrival.len() as u32)
                        .collect();
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
    fn id(&self) -> PanelId {
        PanelId("command_deck")
    }
    fn title(&self) -> String {
        "Command Deck".into()
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspector
    }
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
            if view.driving {
                if ui.button("Release control").clicked() {
                    let v = vessel;
                    ctx.trigger(lunco_avatar::ReleaseVessel { target: v });
                }
            } else {
                if ui.button("Take control").clicked() {
                    let v = vessel;
                    ctx.trigger(lunco_avatar::PossessVessel {
                        avatar: None,
                        target: v,
                        bind_camera: true,
                    });
                }
            }
        });

        ui.separator();

        // ── Autopilot / behaviour ────────────────────────────────────────
        ui.label("Behaviour");
        if view.is_patrol {
            ui.label(format!("Patrol — {} waypoint(s)", view.patrol.len()));
        } else if !view.behaviour_kind.is_empty() {
            ui.label(view.behaviour_kind.to_string());
        } else {
            // The bundled keymap binds PlaceWaypoint to either Alt key. The handler
            // reads that semantic intent, so this hint describes the default binding
            // without making the editor depend on a particular physical key.
            ui.weak("none — PlaceWaypoint (Alt+click by default) the ground to add a waypoint");
        }

        ui.horizontal(|ui| {
            let v = vessel;
            if view.autopilot_engaged {
                if ui.button("Disengage autopilot").clicked() {
                    // Disengage: brake the tree but KEEP the patrol data
                    // (distinct from ClearPatrol, which wipes it). A later
                    // re-engage restores the route.
                    ctx.trigger(lunco_autopilot::DisengageAutopilot { vessel: v });
                }
            } else {
                if ui.button("Engage autopilot").clicked() {
                    // NO throttle. "Engage autopilot" means "run your route" —
                    // it never means "drive forward". A vessel with no route
                    // holds; sending a cruise setpoint from here made a
                    // routeless rover leave in a straight line.
                    ctx.trigger(lunco_autopilot::EngageAutopilot {
                        vessel: v,
                        index: 0,
                        throttle: 0.0,
                        spec_json: view.spec_json.clone(),
                    });
                }
            }
        });

        // ── Builder waypoint editor ───────────────────────────────────────
        // Waypoints remain USD prims: this panel exposes their route in Builder,
        // while placement and transforms stay on the scene surface where their
        // coordinates are meaningful.
        if view.is_patrol && !view.patrol.is_empty() {
            ui.separator();
            ui.label("Waypoints");
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, wp) in view.patrol.iter().enumerate() {
                    // Action marker: 🛰 when this waypoint fires a tool on arrival, so
                    // a geometry-only pin is distinguishable from an armed one.
                    let marker = view
                        .patrol_actions
                        .get(i)
                        .filter(|n| **n > 0)
                        .map(|n| format!(" [tool x{}]", n))
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
        ui.small("Builder editor: Alt+Left-click ground to add · select a pin to move it · Delete to remove");
    }
}
