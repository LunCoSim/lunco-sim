//! Interactive checkpoint authoring — Ctrl+Left‑click on the ground appends a
//! waypoint to the selected vessel's patrol, Right‑click on a pin opens a
//! small context menu (Delete today; extensible).
//!
//! Both edits route through the **existing** [`SetAutopilotBehavior`] /
//! [`EngageAutopilot`] typed commands (§4.2 — one input shape, every surface).
//! The patrol spec lives in `AutopilotBehaviorSpec` (mirrored on the vessel);
//! this module only reads it, mutates the data, and re‑emits the command. No
//! new verb, no new journal domain — same path the `patrol.rhai` prelude and
//! the Command Deck use.
//!
//! # Click partition (global `Pointer<Click>` observers all run for one click)
//!
//! - **Plain Left** → avatar possession (`lunco_avatar::avatar_raycast_possession`).
//! - **Shift+Left** → entity selection (`selection::on_scene_click_select`).
//! - **Ctrl+Left** (this observer) → append checkpoint to the primary-selected
//!   vessel's patrol; no‑op when nothing is selected (possession then takes it).
//! - **Right (Secondary)** (this observer) → open the checkpoint context menu on
//!   the pin under the cursor; no‑op when no pin is near.
//!
//! Spawn‑tool / terrain‑tool armed → this observer stands down (matches the
//! existing three-observer gate convention).

use bevy::prelude::*;
use bevy::picking::events::{Click, Pointer};
use bevy::picking::pointer::PointerButton;
use bevy_egui::{egui, EguiContexts};
use lunco_autopilot::{Autopilot, AutopilotBehaviorSpec, BehaviorSpec};
use lunco_core::{on_command, register_commands, Command, EguiFocus, SpawnToolActive, TerrainToolActive};

use crate::SelectedEntities;
use crate::spawn::{terrain_ray_hit, TerrainOracles};

/// Right-click context menu state. Set by the Secondary click observer;
/// consumed by the egui popup system. One menu at a time.
#[derive(Resource, Default, Clone)]
pub enum CheckpointContextMenu {
    #[default]
    Closed,
    Open {
        /// Vessel whose patrol owns the pin.
        vessel: Entity,
        /// Index in the patrol waypoint list (the deletion target).
        index: usize,
        /// Screen-space position for the popup (pixels).
        screen_pos: [f64; 2],
    },
}

/// Append a checkpoint to the selected vessel's patrol. Triggered by the
/// Ctrl+LMB observer below. A typed command (not a UI poke) so the same action
/// is reachable from rhai / the HTTP API / MCP — the UI is just one dispatch
/// surface for it (§4.2). Addressed by the vessel `Entity` (the UI has it
/// directly; rhai callers address it via the selection or by spawning).
#[Command(reflect_default)]
pub struct AppendCheckpoint {
    /// Vessel to add the checkpoint to.
    pub vessel: Entity,
    /// World-space position `[x, y, z]`.
    pub position: [f64; 3],
}

impl Default for AppendCheckpoint {
    fn default() -> Self {
        Self { vessel: Entity::PLACEHOLDER, position: [0.0; 3] }
    }
}

/// Remove a checkpoint by patrol index from the vessel's patrol. Triggered by
/// the right-click "Delete" menu entry (and reusable from rhai / API).
#[Command(reflect_default)]
pub struct DeleteCheckpoint {
    pub vessel: Entity,
    pub index: u32,
}

impl Default for DeleteCheckpoint {
    fn default() -> Self {
        Self { vessel: Entity::PLACEHOLDER, index: 0 }
    }
}

#[on_command(AppendCheckpoint)]
fn on_append_checkpoint(
    trigger: On<AppendCheckpoint>,
    mut commands: Commands,
    q_spec: Query<&AutopilotBehaviorSpec>,
    q_autopilot: Query<&Autopilot>,
    defaults: Res<crate::checkpoint_gizmo::PatrolDefaults>,
) {
    let cmd = trigger.event();
    let new_wp = [cmd.position[0] as f32, cmd.position[1] as f32, cmd.position[2] as f32];
    // Build the new patrol spec: clone the existing Patrol's waypoints + append,
    // or start a fresh patrol from this single waypoint (defaults from the
    // tunable `PatrolDefaults` resource, not literals — §3). New checkpoints
    // added via Ctrl+LMB are bare waypoints (no arrival actions) — a mission
    // authors actions in rhai/USD; interactive editing is geometry-only.
    let fresh = || BehaviorSpec::Patrol {
        waypoints: vec![lunco_autopilot::PatrolWaypoint::at(new_wp)],
        speed: defaults.speed,
        radius: defaults.radius,
        dwell: defaults.dwell,
    };
    let spec_json = match q_spec.get(cmd.vessel) {
        Ok(spec) => {
            let new_spec = match &spec.0 {
                BehaviorSpec::Patrol { waypoints, speed, radius, dwell } => {
                    let mut wps = waypoints.clone();
                    wps.push(lunco_autopilot::PatrolWaypoint::at(new_wp));
                    BehaviorSpec::Patrol {
                        waypoints: wps,
                        speed: *speed,
                        radius: *radius,
                        dwell: *dwell,
                    }
                }
                // Non-patrol spec present → replace with a patrol starting here.
                _ => fresh(),
            };
            serde_json::to_string(&new_spec).unwrap_or_default()
        }
        // No spec yet → start a patrol from this single waypoint.
        Err(_) => serde_json::to_string(&fresh()).unwrap_or_default(),
    };
    // An autopilot actor must exist for `SetAutopilotBehavior` to find it (its
    // observer keys by `ap.vessel`). If none exists, engage one — it will both
    // claim the vessel and adopt the patrol spec.
    let need_engage = !q_autopilot.iter().any(|a| a.vessel == cmd.vessel);
    if need_engage {
        // No autopilot actor for this vessel yet — engage one with the patrol.
        commands.trigger(lunco_autopilot::EngageAutopilot {
            vessel: cmd.vessel,
            index: 0,
            throttle: defaults.engage_throttle,
            spec_json,
        });
    } else {
        // An autopilot is already driving — just hot-swap its behaviour.
        commands.trigger(lunco_autopilot::SetAutopilotBehavior {
            vessel: cmd.vessel,
            spec_json,
        });
    }
    info!("APPEND_CHECKPOINT: vessel {:?} at {:?}", cmd.vessel, cmd.position);
}

#[on_command(DeleteCheckpoint)]
fn on_delete_checkpoint(
    trigger: On<DeleteCheckpoint>,
    mut commands: Commands,
    q_spec: Query<&AutopilotBehaviorSpec>,
) {
    let cmd = trigger.event();
    let Some(spec) = q_spec.get(cmd.vessel).ok() else { return };
    let BehaviorSpec::Patrol { waypoints, speed, radius, dwell } = &spec.0 else { return };
    let idx = cmd.index as usize;
    if idx >= waypoints.len() {
        return;
    }
    let mut wps = waypoints.clone();
    wps.remove(idx);
    if wps.is_empty() {
        // Empty patrol → clear it entirely (brake + drop the spec mirror) via
        // the canonical verb, instead of hand-building a Brake spec JSON.
        commands.trigger(lunco_autopilot::ClearPatrol { vessel: cmd.vessel });
        info!("DELETE_CHECKPOINT: vessel {:?} idx {} (last → cleared)", cmd.vessel, cmd.index);
        return;
    }
    let new_spec = BehaviorSpec::Patrol { waypoints: wps, speed: *speed, radius: *radius, dwell: *dwell };
    let spec_json = serde_json::to_string(&new_spec).unwrap_or_default();
    commands.trigger(lunco_autopilot::SetAutopilotBehavior {
        vessel: cmd.vessel,
        spec_json,
    });
    info!("DELETE_CHECKPOINT: vessel {:?} idx {}", cmd.vessel, cmd.index);
}

register_commands!(on_append_checkpoint, on_delete_checkpoint,);

/// Global `Pointer<Click>` observer — Ctrl+LMB appends a checkpoint, Right
/// opens the context menu. Stands down when the spawn / terrain sculpt tool is
/// armed (matches the existing observers).
pub fn on_scene_click_checkpoint(
    mut click: On<Pointer<Click>>,
    keys: Res<ButtonInput<KeyCode>>,
    egui_focus: Res<EguiFocus>,
    spawn_tool: Res<SpawnToolActive>,
    terrain_tool: Res<TerrainToolActive>,
    selected: Res<SelectedEntities>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    terrains: TerrainOracles,
    q_spec: Query<&AutopilotBehaviorSpec>,
    gizmo_settings: Res<crate::checkpoint_gizmo::CheckpointGizmoSettings>,
    mut menu: ResMut<CheckpointContextMenu>,
    mut commands: Commands,
) {
    click.propagate(false);
    if egui_focus.wants_pointer {
        return;
    }
    if spawn_tool.0 || terrain_tool.0 {
        return;
    }
    match click.button {
        PointerButton::Primary => {
            // Ctrl+LMB only — plain click belongs to possession, Shift+click to
            // selection (the partition documented in `selection.rs`).
            if !(keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight)) {
                return;
            }
            let Some(vessel) = selected.primary() else { return };
            let Some((camera, cam_gtf)) = cameras.iter().find(|(c, _)| c.is_active) else { return };
            let Some(ray) = lunco_core::scene_click_ray(
                &egui_focus,
                camera,
                cam_gtf,
                click.pointer_location.position,
            ) else { return };
            // Ground-truth terrain hit (the DEM oracle, not the band-limited
            // collider ring) — same path `spawn::on_scene_click_spawn` uses.
            let origin = ray.origin.as_dvec3();
            let dir = ray.direction.as_dvec3();
            let Some((_, hit)) = terrain_ray_hit(&terrains, origin, dir, 1.0e6) else { return };
            commands.trigger(AppendCheckpoint {
                vessel,
                position: [hit.x, hit.y, hit.z],
            });
        }
        PointerButton::Secondary => {
            // Right-click: find nearest pin to the cursor and open the menu.
            let Some(vessel) = selected.primary() else { return };
            let Ok(spec) = q_spec.get(vessel) else { return };
            let BehaviorSpec::Patrol { waypoints, .. } = &spec.0 else { return };
            let Some((camera, cam_gtf)) = cameras.iter().find(|(c, _)| c.is_active) else { return };
            let cursor = click.pointer_location.position;
            let mut best: Option<(usize, f32)> = None;
            for (i, wp) in waypoints.iter().enumerate() {
                let wp_world = Vec3::from_array(wp.pos);
                if let Ok(viewport) = camera.world_to_viewport(cam_gtf, wp_world) {
                    let d = Vec2::new(viewport.x - cursor.x, viewport.y - cursor.y).length_squared();
                    if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                        best = Some((i, d));
                    }
                }
            }
            // Pick radius comes from `CheckpointGizmoSettings` so it tracks the
            // visual pin size the gizmo draws (§3 — no magic numbers).
            let pick_r = gizmo_settings.pin_pick_radius_px;
            if let Some((idx, d2)) = best {
                if d2 <= pick_r * pick_r {
                    *menu = CheckpointContextMenu::Open {
                        vessel,
                        index: idx,
                        screen_pos: [cursor.x as f64, cursor.y as f64],
                    };
                }
            }
        }
        _ => {}
    }
}

/// Egui popup for the open checkpoint context menu. Drawn in the workbench's
/// `EguiPrimaryContextPass`. "Delete" fires [`DeleteCheckpoint`]; "Cancel", a
/// click outside the popup, or the target checkpoint going away closes it.
pub fn draw_checkpoint_context_menu(
    mut egui_ctx: EguiContexts,
    mut menu: ResMut<CheckpointContextMenu>,
    q_spec: Query<&AutopilotBehaviorSpec>,
    mut commands: Commands,
) {
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let (vessel, index, pos) = match &*menu {
        CheckpointContextMenu::Open { vessel, index, screen_pos } => {
            (*vessel, *index, *screen_pos)
        }
        CheckpointContextMenu::Closed => return,
    };
    // `index` is a SNAPSHOT taken at right-click time, but the patrol underneath
    // it can change while the popup sits open (delete a pin from the Command
    // Deck, clear the patrol, despawn the vessel). Re-validate against the live
    // spec every frame: otherwise "Delete" would happily remove whatever waypoint
    // now happens to occupy that index — a different pin than the user opened.
    let target_exists = q_spec
        .get(vessel)
        .ok()
        .and_then(|s| s.patrol_waypoints())
        .is_some_and(|w| index < w.len());
    if !target_exists {
        *menu = CheckpointContextMenu::Closed;
        return;
    }
    let pos = egui::pos2(pos[0] as f32, pos[1] as f32);
    let resp = egui::Area::new(egui::Id::new("lunco_checkpoint_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            ui.set_max_width(160.0);
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                if ui.button("🗑  Delete checkpoint").clicked() {
                    commands.trigger(DeleteCheckpoint {
                        vessel,
                        index: index as u32,
                    });
                    true // close
                } else if ui.button("Cancel").clicked() {
                    true
                } else {
                    false
                }
            }).inner
        });
    // A button was hit → close. Otherwise dismiss on any click that lands outside
    // the popup (the doc's "any outside interaction closes it" — without this the
    // popup survives left-clicks, deselection, and camera moves).
    let clicked_away = ctx.input(|i| i.pointer.any_click()) && !resp.response.contains_pointer();
    if resp.inner || clicked_away {
        *menu = CheckpointContextMenu::Closed;
    }
}