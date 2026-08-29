//! Joint + wheel-force visualization gizmos.
//!
//! Mirrors `physics_viz.rs`'s pattern: a global [`JointVizSettings`]
//! resource + a [`ToggleJointViz`] [`Command`](lunco_core::Command) for
//! UI / API / Rhai parity (`cmd("ToggleJointViz", #{show_joints: true})`).
//!
//! Two independent layers, each toggled separately:
//!
//! - **Joints** — draws anchor dots + axis lines for every Avian joint
//!   (Revolute, Prismatic, Fixed, Spherical, Distance) in the scene.
//!   Lets you see the rocker-bogie suspension topology at a glance.
//!
//! - **Wheel forces** — draws a wireframe box plus the force observations
//!   owned by each wheel realization. Raycast wheels expose the solved tire
//!   and normal forces; joint wheels expose Avian's solved constraint force.
//!   A wheel does not own the rigid body's integration accumulator, so that
//!   accumulator is never projected as a per-wheel force.
//!
//! Both systems early-return when their flag is off, so the cost is
//! effectively zero when visualization is disabled.

use avian3d::dynamics::joints::{DistanceJoint, SphericalJoint};
use avian3d::prelude::{
    FixedJoint, JointAnchor, JointForces, JointFrame, LinearVelocity, PrismaticJoint,
    RevoluteJoint, RigidBody,
};
use bevy::prelude::*;
use lunco_core::{on_command, register_commands, Command};
use lunco_mobility::{JointedWheelTire, WheelBodyMount, WheelRaycast};
use lunco_usd_sim::PhysicalWheel;

// ── Settings resource + typed command ────────────────────────────────────

/// Global toggle for joint + wheel-force visualization.
///
/// Flip via [`ToggleJointViz`] command (UI / API / Rhai).
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
pub struct JointVizSettings {
    /// Draw anchor dots + axis lines for all Avian joints.
    pub show_joints: bool,
    /// Draw a force box + arrow at every wheel.
    pub show_wheel_forces: bool,
}

/// Toggle joint / wheel-force visualization.
///
/// `#[Command(default)]` → all-false. Pass only the flags you want on.
/// Rhai: `cmd("ToggleJointViz", #{show_joints: true, show_wheel_forces: true})`.
#[Command(default)]
pub struct ToggleJointViz {
    /// Show joint anchors + axes.
    pub show_joints: bool,
    /// Show wheel force boxes + arrows.
    pub show_wheel_forces: bool,
}

#[on_command(ToggleJointViz)]
fn on_toggle_joint_viz(trigger: On<ToggleJointViz>, mut settings: ResMut<JointVizSettings>) {
    let cmd = trigger.event();
    settings.show_joints = cmd.show_joints;
    settings.show_wheel_forces = cmd.show_wheel_forces;
}

register_commands!(on_toggle_joint_viz,);

// ── Visual constants ─────────────────────────────────────────────────────

const ANCHOR_COLOR: Color = Color::srgb(1.0, 0.85, 0.2);
const AXIS_COLOR: Color = Color::srgb(0.2, 0.8, 1.0);
const LINK_COLOR: Color = Color::srgb(0.55, 0.55, 0.55);
const BOX_COLOR: Color = Color::srgb(0.9, 0.9, 0.2);

const ANCHOR_RADIUS: f32 = 0.06;
const AXIS_LEN: f32 = 0.4;
const BOX_HALF: f32 = 0.25;
const MAX_ARROW_LEN: f32 = 4.0;
const NEWTONS_PER_METER: f32 = 500.0;
const METERS_PER_MPS: f32 = 0.5;

const TIRE_FORCE_COLOR: Color = Color::srgb(1.0, 0.45, 0.15);
const NORMAL_FORCE_COLOR: Color = Color::srgb(0.65, 1.0, 0.2);
const JOINT_FORCE_COLOR: Color = Color::srgb(0.3, 0.75, 1.0);
const VELOCITY_COLOR: Color = Color::srgb(0.2, 1.0, 0.4);

// ── Helpers ──────────────────────────────────────────────────────────────

/// World-space position of a [`JointFrame`]'s anchor, given the owning
/// body's [`GlobalTransform`]. `Local` anchors are transformed by the
/// body; `FromGlobal` anchors are already world-space.
fn anchor_world(frame: &JointFrame, body_tf: &GlobalTransform) -> Vec3 {
    let to_vec3 = |v: avian3d::math::Vector| Vec3::new(v.x as f32, v.y as f32, v.z as f32);
    match frame.anchor {
        JointAnchor::Local(v) => body_tf.transform_point(to_vec3(v)),
        JointAnchor::FromGlobal(v) => to_vec3(v),
    }
}

/// Draw a joint's anchor dots, inter-body connection line, and optional
/// axis arrow (revolute / prismatic / spherical have one; fixed /
/// distance do not).
fn draw_joint_gizmo(
    gizmos: &mut Gizmos,
    a1: Vec3,
    a2: Vec3,
    axis: Option<(avian3d::math::Vector, &GlobalTransform)>,
) {
    gizmos.sphere(a1, ANCHOR_RADIUS, ANCHOR_COLOR);
    gizmos.sphere(a2, ANCHOR_RADIUS, ANCHOR_COLOR);
    gizmos.line(a1, a2, LINK_COLOR);
    if let Some((local_axis, body_tf)) = axis {
        let dir = body_tf.rotation()
            * Vec3::new(
                local_axis.x as f32,
                local_axis.y as f32,
                local_axis.z as f32,
            );
        let dir = dir.normalize_or_zero() * AXIS_LEN;
        gizmos.arrow(a1 - dir, a1 + dir, AXIS_COLOR);
    }
}

/// Map a physical vector to a readable, bounded gizmo arrow.
///
/// Debug geometry must remain local to the body being inspected. An
/// unbounded velocity or force vector turns a normal transient into a line
/// across the scene, which is both unreadable and easy to mistake for a
/// frame/physics failure.
fn arrow_vector(vector: Vec3, meters_per_unit: f32) -> Option<Vec3> {
    let magnitude = vector.length();
    if !magnitude.is_finite() || magnitude < 1.0e-3 || meters_per_unit <= 0.0 {
        return None;
    }
    let length = (magnitude * meters_per_unit).min(MAX_ARROW_LEN);
    Some(vector / magnitude * length)
}

// ── Joint drawing system ─────────────────────────────────────────────────

/// Draw every Avian joint in the scene when `show_joints` is on.
///
/// Five separate queries (one per joint type) because Bevy ECS can't
/// OR component queries. Each calls [`draw_joint_gizmo`] with the
/// anchor positions and axis (if any) extracted from the joint data.
pub fn draw_joint_viz(
    mut gizmos: Gizmos,
    settings: Res<JointVizSettings>,
    q_revolute: Query<&RevoluteJoint>,
    q_prismatic: Query<&PrismaticJoint>,
    q_fixed: Query<&FixedJoint>,
    q_spherical: Query<&SphericalJoint>,
    q_distance: Query<&DistanceJoint>,
    q_transforms: Query<&GlobalTransform>,
) {
    if !settings.show_joints {
        return;
    }

    for j in q_revolute.iter() {
        let (Ok(tf1), Ok(tf2)) = (q_transforms.get(j.body1), q_transforms.get(j.body2)) else {
            continue;
        };
        draw_joint_gizmo(
            &mut gizmos,
            anchor_world(&j.frame1, tf1),
            anchor_world(&j.frame2, tf2),
            Some((j.hinge_axis, tf1)),
        );
    }

    for j in q_prismatic.iter() {
        let (Ok(tf1), Ok(tf2)) = (q_transforms.get(j.body1), q_transforms.get(j.body2)) else {
            continue;
        };
        draw_joint_gizmo(
            &mut gizmos,
            anchor_world(&j.frame1, tf1),
            anchor_world(&j.frame2, tf2),
            Some((j.slider_axis, tf1)),
        );
    }

    for j in q_fixed.iter() {
        let (Ok(tf1), Ok(tf2)) = (q_transforms.get(j.body1), q_transforms.get(j.body2)) else {
            continue;
        };
        draw_joint_gizmo(
            &mut gizmos,
            anchor_world(&j.frame1, tf1),
            anchor_world(&j.frame2, tf2),
            None,
        );
    }

    for j in q_spherical.iter() {
        let (Ok(tf1), Ok(tf2)) = (q_transforms.get(j.body1), q_transforms.get(j.body2)) else {
            continue;
        };
        draw_joint_gizmo(
            &mut gizmos,
            anchor_world(&j.frame1, tf1),
            anchor_world(&j.frame2, tf2),
            Some((j.twist_axis, tf1)),
        );
    }

    for j in q_distance.iter() {
        let (Ok(tf1), Ok(tf2)) = (q_transforms.get(j.body1), q_transforms.get(j.body2)) else {
            continue;
        };
        // DistanceJoint has anchor1/anchor2 (JointAnchor) directly, no JointFrame.
        let to_vec3 = |v: avian3d::math::Vector| Vec3::new(v.x as f32, v.y as f32, v.z as f32);
        let a1 = match j.anchor1 {
            JointAnchor::Local(v) => tf1.transform_point(to_vec3(v)),
            JointAnchor::FromGlobal(v) => to_vec3(v),
        };
        let a2 = match j.anchor2 {
            JointAnchor::Local(v) => tf2.transform_point(to_vec3(v)),
            JointAnchor::FromGlobal(v) => to_vec3(v),
        };
        draw_joint_gizmo(&mut gizmos, a1, a2, None);
    }
}

// ── Wheel force drawing system ───────────────────────────────────────────

/// Draw a wireframe box + force arrow at every wheel when
/// `show_wheel_forces` is on.
///
/// The box is a fixed-size `Cuboid` outline at the wheel's world
/// position (makes loaded vs. airborne wheels visually obvious). Force
/// arrows come from the wheel realization's own solved force boundary.
///
/// Covers both wheel kinds: `PhysicalWheel` (joint-based, e.g.
/// rocker-bogie) and `WheelRaycast` (raycast, e.g. skid/Ackermann).
pub fn draw_wheel_force_viz(
    mut gizmos: Gizmos,
    settings: Res<JointVizSettings>,
    q_physical: Query<
        (&GlobalTransform, Option<&LinearVelocity>, &JointedWheelTire),
        With<PhysicalWheel>,
    >,
    q_raycast: Query<
        (
            &GlobalTransform,
            Option<&LinearVelocity>,
            &WheelRaycast,
            &WheelBodyMount,
        ),
        With<WheelRaycast>,
    >,
    q_bodies: Query<&GlobalTransform, With<RigidBody>>,
    q_joint_forces: Query<&JointForces>,
) {
    if !settings.show_wheel_forces {
        return;
    }

    let draw_box = |gizmos: &mut Gizmos, pos: Vec3| {
        // Wireframe box at the wheel — makes it easy to spot which
        // wheels are tracked even when force is near-zero.
        gizmos.primitive_3d(
            &Cuboid {
                half_size: Vec3::splat(BOX_HALF),
            },
            Isometry3d::from_translation(pos),
            BOX_COLOR,
        );
    };
    let draw_velocity = |gizmos: &mut Gizmos, pos: Vec3, vel: Option<&LinearVelocity>| {
        let Some(vel) = vel else { return };
        let velocity = Vec3::new(vel.0.x as f32, vel.0.y as f32, vel.0.z as f32);
        if let Some(dir) = arrow_vector(velocity, METERS_PER_MPS) {
            gizmos.arrow(pos, pos + dir, VELOCITY_COLOR);
        }
    };

    for (tf, vel, tire) in q_physical.iter() {
        let pos = tf.translation();
        draw_box(&mut gizmos, pos);
        // Physical-wheel tire forces are applied through the wheel body, while
        // the revolute joint carries the solved reaction. Read that explicit
        // Avian writeback instead of treating the body accumulator as a wheel
        // force. The synthesized wheel joint owns this optional readback.
        if let Ok(joint_forces) = q_joint_forces.get(tire.drive_joint) {
            if let Some(dir) = arrow_vector(joint_forces.force().as_vec3(), 1.0 / NEWTONS_PER_METER)
            {
                gizmos.arrow(pos, pos + dir, JOINT_FORCE_COLOR);
            }
        }
        draw_velocity(&mut gizmos, pos, vel);
    }

    for (tf, vel, wheel, mount) in q_raycast.iter() {
        let pos = tf.translation();
        draw_box(&mut gizmos, pos);
        if let Some(dir) = arrow_vector(wheel.tire_force.as_vec3(), 1.0 / NEWTONS_PER_METER) {
            gizmos.arrow(pos, pos + dir, TIRE_FORCE_COLOR);
        }
        let root_up = q_bodies
            .get(mount.body)
            .map(|body| body.rotation() * Vec3::Y)
            .unwrap_or(Vec3::Y);
        let normal = root_up * wheel.last_normal_force as f32;
        if let Some(dir) = arrow_vector(normal, 1.0 / NEWTONS_PER_METER) {
            gizmos.arrow(pos, pos + dir, NORMAL_FORCE_COLOR);
        }
        draw_velocity(&mut gizmos, pos, vel);
    }
}
