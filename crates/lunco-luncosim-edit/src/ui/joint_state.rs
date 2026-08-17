//! Joint State panel — live joint positions / velocities / torques for the
//! selected vessel (deep-review §2.7 gap 2).
//!
//! **WP-8 reactive shape:** an explicitly live view-model producer
//! ([`populate_joint_state_view`]) flattens the selected vessel's joint and
//! wheel state into [`JointStateView`]; the panel ([`JointStatePanel`]) is a
//! pure reader via [`PanelCtx::resource`]. The producer returns before any
//! joint or wheel scan while nothing is selected — while a vessel *is*
//! selected the values are live physics and change every tick, so it
//! intentionally runs each frame (same bounded-by-vessel scale as the
//! `joint_viz` gizmo pass).
//!
//! Row sources, per joint kind:
//! - **Revolute** (physical wheels, rocker-bogie pins, doors): angle = twist of
//!   the relative joint-basis rotation about `hinge_axis`; ω = relative angular
//!   velocity projected on the world hinge axis; target / torque cap read
//!   straight from the joint's [`AngularMotor`] — the exact values
//!   `lunco_hardware::MotorActuator` wrote this tick (its live command-scaled
//!   DC-curve cap), so no second torque law is transcribed here.
//! - **Raycast wheels** (no avian joint): θ/ω are the canonical
//!   `WheelRaycast::spin_angle` / `spin_velocity`; target = throttle ·
//!   `max_rotation_speed` from the wheel's drive [`Port`]; drive torque is
//!   reconstructed with the SAME authored curve both realizations use,
//!   [`lunco_hardware::axle_torque`] — one definition, per project rule.
//! - **Steering**: steered wheels (either kind) carry a
//!   [`lunco_hardware::SteeringActuator`]; its `output_angle` is the single
//!   shared steer output both realizations consume, shown in the steer column.

use avian3d::prelude::{AngularVelocity, JointBasis, JointFrame, RevoluteJoint, Rotation};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use bevy_egui::egui;
use lunco_core::architecture::Port;
use lunco_hardware::{axle_torque, SteeringActuator};
use lunco_mobility::WheelRaycast;
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

use lunco_scene_commands::SelectedEntities;

// ─────────────────────────────────────────────────────────────────────
// View-model
// ─────────────────────────────────────────────────────────────────────

/// What kind of articulation a [`JointStateRow`] describes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JointRowKind {
    /// An avian [`RevoluteJoint`] (physical wheel axle, suspension pin, door…).
    Revolute,
    /// A [`WheelRaycast`] wheel — no avian joint; spin state is its own.
    RaycastWheel,
}

impl JointRowKind {
    fn label(self) -> &'static str {
        match self {
            Self::Revolute => "revolute",
            Self::RaycastWheel => "raycast",
        }
    }
}

/// One joint / wheel of the selected vessel, flattened for display.
#[derive(Clone)]
pub struct JointStateRow {
    /// Display name — the entity's `Name` (falls back through the jointed
    /// body's name to the raw entity id).
    pub name: String,
    /// Which articulation kind produced this row.
    pub kind: JointRowKind,
    /// Joint angle (rad). Revolute: signed twist about the hinge axis in
    /// (−π, π]. Raycast wheel: accumulated spin angle wrapped to [0, 2π).
    pub angle: f64,
    /// Angular velocity about the joint axis (rad/s).
    pub omega: f64,
    /// Commanded motor target velocity (rad/s), when a motor is driving.
    pub target: Option<f64>,
    /// Torque (N·m). Revolute: the motor's live torque cap (what
    /// `MotorActuator` computed from the authored DC curve this tick).
    /// Raycast: the current drive torque from the same authored curve at the
    /// current ω and throttle. `None` when no motor drives the joint.
    pub torque: Option<f64>,
    /// Steer angle (rad) for steered wheels ([`SteeringActuator::output_angle`]).
    pub steer: Option<f64>,
}

/// Live view-model for the Joint State panel. Producer:
/// [`populate_joint_state_view`]; reader: [`JointStatePanel`].
#[derive(Resource, Default)]
pub struct JointStateView {
    /// The vessel whose joints are shown (root ancestor of the primary
    /// selection), or `None` when nothing is selected.
    pub vessel: Option<Entity>,
    /// Display name of the vessel.
    pub vessel_name: String,
    /// One row per joint / wheel, revolute joints first, then raycast wheels.
    pub rows: Vec<JointStateRow>,
}

// ─────────────────────────────────────────────────────────────────────
// Producer
// ─────────────────────────────────────────────────────────────────────

/// Walk `ChildOf` to the top-most ancestor — the vessel root the transform
/// gizmo / selection machinery operates on.
fn root_of(entity: Entity, q_child_of: &Query<&ChildOf>) -> Entity {
    let mut cur = entity;
    while let Ok(child_of) = q_child_of.get(cur) {
        cur = child_of.parent();
    }
    cur
}

/// World-space joint-basis orientation for one side of a joint.
fn basis_world(rot: &Rotation, frame: &JointFrame) -> DQuat {
    match frame.basis {
        JointBasis::Local(q) => rot.0 * q,
        JointBasis::FromGlobal(q) => q,
    }
}

/// Signed twist angle (rad, in (−π, π]) of `rel` about the unit `axis`
/// (swing–twist decomposition; the swing component is discarded).
fn twist_about(rel: DQuat, axis: DVec3) -> f64 {
    let proj = DVec3::new(rel.x, rel.y, rel.z).dot(axis);
    let mut angle = 2.0 * proj.atan2(rel.w);
    if angle > std::f64::consts::PI {
        angle -= std::f64::consts::TAU;
    } else if angle <= -std::f64::consts::PI {
        angle += std::f64::consts::TAU;
    }
    angle
}

/// A motor `max_torque` of `Scalar::MAX` (avian's "unlimited" default) or any
/// other absurd magnitude is a solver setting, not a physical readout — hide it.
fn readable_torque(t: f64) -> Option<f64> {
    (t.is_finite() && t.abs() < 1.0e9).then_some(t)
}

/// Display name for a row: the entity's `Name`, else the jointed body's
/// `Name`, else the raw entity id.
fn row_name(entity: Entity, body: Option<Entity>, q_name: &Query<&Name>) -> String {
    if let Ok(name) = q_name.get(entity) {
        return name.as_str().to_owned();
    }
    if let Some(body) = body {
        if let Ok(name) = q_name.get(body) {
            return name.as_str().to_owned();
        }
    }
    format!("{entity:?}")
}

/// Producer for [`JointStateView`]: fills the view for the SELECTED vessel
/// only. Registered via `add_view_model_every_frame`: live joint values change
/// every physics tick, while the selection guard below keeps the no-selection
/// path O(1) before any joint or wheel query is iterated.
#[allow(clippy::too_many_arguments)]
pub fn populate_joint_state_view(
    mut view: ResMut<JointStateView>,
    selection: Res<SelectedEntities>,
    q_child_of: Query<&ChildOf>,
    q_name: Query<&Name>,
    q_joints: Query<(Entity, &RevoluteJoint, Option<&SteeringActuator>)>,
    q_wheels: Query<(Entity, &WheelRaycast, Option<&SteeringActuator>)>,
    q_bodies: Query<(&Rotation, &AngularVelocity)>,
    q_ports: Query<&Port>,
) {
    let Some(selected) = selection.primary() else {
        // Deselected: empty the view once; the gate then stands down.
        if view.vessel.is_some() || !view.rows.is_empty() {
            view.vessel = None;
            view.vessel_name.clear();
            view.rows.clear();
        }
        return;
    };

    let root = root_of(selected, &q_child_of);
    view.vessel = Some(root);
    view.vessel_name = row_name(root, None, &q_name);
    view.rows.clear();

    // ── Avian revolute joints (physical wheels, suspension pins, doors) ──
    for (joint_entity, joint, steer) in q_joints.iter() {
        // Membership: the jointed child body hangs under the selected vessel.
        if root_of(joint.body2, &q_child_of) != root && root_of(joint_entity, &q_child_of) != root {
            continue;
        }
        let (Ok((rot1, av1)), Ok((rot2, av2))) =
            (q_bodies.get(joint.body1), q_bodies.get(joint.body2))
        else {
            continue;
        };

        let b1 = basis_world(rot1, &joint.frame1);
        let b2 = basis_world(rot2, &joint.frame2);
        let rel = b1.inverse() * b2;
        let angle = twist_about(rel, joint.hinge_axis);
        let axis_world = b1 * joint.hinge_axis;
        let omega = (av2.0 - av1.0).dot(axis_world);

        // The joint's AngularMotor holds exactly what `MotorActuator` (drive)
        // or the cosim joint backend wrote this tick: target velocity and the
        // live torque cap from the authored DC curve.
        let motor_on = joint.motor.enabled;
        view.rows.push(JointStateRow {
            name: row_name(joint_entity, Some(joint.body2), &q_name),
            kind: JointRowKind::Revolute,
            angle,
            omega,
            target: motor_on.then_some(joint.motor.target_velocity),
            torque: if motor_on {
                readable_torque(joint.motor.max_torque)
            } else {
                None
            },
            steer: steer.map(|s| s.output_angle),
        });
    }

    // ── Raycast wheels (no avian joint; spin state is the wheel's own) ──
    for (wheel_entity, wheel, steer) in q_wheels.iter() {
        if root_of(wheel_entity, &q_child_of) != root {
            continue;
        }
        // Throttle from the wheel's drive port; no port wired reads as 0
        // (free-rolling), matching `update_wheel_spin`.
        let throttle = q_ports
            .get(wheel.drive_port)
            .map(|p| p.value.clamp(-1.0, 1.0))
            .unwrap_or(0.0);
        let driven = wheel.drive_torque_max > 0.0;
        // Same authored torque-speed law both wheel realizations evaluate
        // (`lunco_hardware::axle_torque`) at the current throttle and ω —
        // the drive torque the spin integrator applies this tick (traction
        // saturation aside).
        let torque = driven.then(|| {
            axle_torque(
                wheel.drive_torque_max,
                wheel.max_rotation_speed,
                throttle,
                wheel.spin_velocity,
            )
        });
        view.rows.push(JointStateRow {
            name: row_name(wheel_entity, None, &q_name),
            kind: JointRowKind::RaycastWheel,
            angle: wheel.spin_angle,
            omega: wheel.spin_velocity,
            target: (driven && throttle.abs() > f64::EPSILON)
                .then_some(throttle * wheel.max_rotation_speed),
            torque,
            steer: steer.map(|s| s.output_angle),
        });
    }
}

// ─────────────────────────────────────────────────────────────────────
// Panel
// ─────────────────────────────────────────────────────────────────────

/// Joint State panel — live θ / ω / target / τ table for the selected vessel.
pub struct JointStatePanel;

impl Panel for JointStatePanel {
    fn id(&self) -> PanelId {
        PanelId("sandbox_joint_state")
    }
    fn title(&self) -> String {
        "Joint State".into()
    }
    fn default_slot(&self) -> PanelSlot {
        // Wide table — reads like Console/Plots, so it docks at the bottom.
        PanelSlot::Bottom
    }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }
    fn transparent_background(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        ctx.panel_content_frame().show(ui, |ui| {
            egui::ScrollArea::both()
                .auto_shrink([false, true])
                .show(ui, |ui| joint_state_content(ui, ctx));
        });
    }
}

/// Fixed-width value cell so columns don't jitter as digits change.
fn value_cell(ui: &mut egui::Ui, v: Option<f64>) {
    match v {
        Some(v) => {
            ui.monospace(format!("{v:+9.3}"));
        }
        None => {
            ui.weak("—");
        }
    }
}

fn joint_state_content(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    let Some(view) = ctx.resource::<JointStateView>() else {
        ui.weak("joint state view unavailable");
        return;
    };

    let Some(_vessel) = view.vessel else {
        ui.weak("no vessel selected");
        return;
    };

    ui.horizontal(|ui| {
        ui.strong(&view.vessel_name);
        ui.weak(format!("· {} joint(s)", view.rows.len()));
    });

    if view.rows.is_empty() {
        ui.weak("selected vessel has no joints");
        return;
    }

    egui::Grid::new("joint_state_grid")
        .striped(true)
        .min_col_width(56.0)
        .show(ui, |ui| {
            // Header — units spelled out once, per column.
            ui.strong("joint");
            ui.strong("type");
            ui.strong("θ  rad");
            ui.strong("ω  rad/s");
            ui.strong("target  rad/s");
            ui.strong("τ  N·m");
            ui.strong("steer  rad");
            ui.end_row();

            for row in &view.rows {
                ui.label(&row.name);
                ui.weak(row.kind.label());
                value_cell(ui, Some(row.angle));
                value_cell(ui, Some(row.omega));
                value_cell(ui, row.target);
                value_cell(ui, row.torque);
                value_cell(ui, row.steer);
                ui.end_row();
            }
        });
}
