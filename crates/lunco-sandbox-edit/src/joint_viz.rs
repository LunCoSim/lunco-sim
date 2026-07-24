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
//! - **Wheel forces** — draws a wireframe box + net-force arrow at
//!   every wheel (`PhysicalWheel` or `WheelRaycast`). The box makes
//!   it obvious which wheels are loaded vs. airborne; the arrow shows
//!   the force direction. Force is `VelocityIntegrationData ×
//!   ComputedMass` (same source as `physics_viz.rs`'s force arrow —
//!   captures cosim / constant forces, excludes gravity / contacts).
//!
//! Both systems early-return when their flag is off, so the cost is
//! effectively zero when visualization is disabled.

use avian3d::dynamics::integrator::VelocityIntegrationData;
use avian3d::dynamics::joints::{DistanceJoint, SphericalJoint};
use avian3d::prelude::{
    ComputedMass, FixedJoint, JointAnchor, JointFrame, LinearVelocity, PrismaticJoint,
    RevoluteJoint,
};
use bevy::prelude::*;
use lunco_core::{on_command, register_commands, Command};
use lunco_mobility::WheelRaycast;
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
const FORCE_SCALE: f32 = 0.005;
const BOX_HALF: f32 = 0.25;

// XYZ axis colors for force-component arrows (RGB = XYZ convention).
const FORCE_X_COLOR: Color = Color::srgb(1.0, 0.2, 0.2);
const FORCE_Y_COLOR: Color = Color::srgb(0.2, 1.0, 0.2);
const FORCE_Z_COLOR: Color = Color::srgb(0.2, 0.4, 1.0);

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
/// position (makes loaded vs. airborne wheels visually obvious). The
/// arrow is the net force (`VelocityIntegrationData × ComputedMass`),
/// same source as `physics_viz.rs`'s force arrow.
///
/// Covers both wheel kinds: `PhysicalWheel` (joint-based, e.g.
/// rocker-bogie) and `WheelRaycast` (raycast, e.g. skid/Ackermann).
pub fn draw_wheel_force_viz(
    mut gizmos: Gizmos,
    settings: Res<JointVizSettings>,
    q_physical: Query<
        (
            &GlobalTransform,
            Option<&LinearVelocity>,
            Option<&VelocityIntegrationData>,
            Option<&ComputedMass>,
        ),
        With<PhysicalWheel>,
    >,
    q_raycast: Query<
        (
            &GlobalTransform,
            Option<&LinearVelocity>,
            Option<&VelocityIntegrationData>,
            Option<&ComputedMass>,
        ),
        With<WheelRaycast>,
    >,
) {
    if !settings.show_wheel_forces {
        return;
    }

    let draw = |gizmos: &mut Gizmos,
                tf: &GlobalTransform,
                vel: Option<&LinearVelocity>,
                integration: Option<&VelocityIntegrationData>,
                mass: Option<&ComputedMass>| {
        let pos = tf.translation();

        // Wireframe box at the wheel — makes it easy to spot which
        // wheels are tracked even when force is near-zero.
        gizmos.primitive_3d(
            &Cuboid {
                half_size: Vec3::splat(BOX_HALF),
            },
            Isometry3d::from_translation(pos),
            BOX_COLOR,
        );

        // Force broken into XYZ components — three arrows, one per axis,
        // each length proportional to the force component along that axis.
        // Red=X, Green=Y, Blue=Z (standard physics-debug convention).
        if let (Some(integ), Some(m)) = (integration, mass) {
            let a = integ.linear_increment;
            let mass_scalar = m.value() as f32;
            let s = mass_scalar * FORCE_SCALE;
            let fx = a.x as f32 * s;
            let fy = a.y as f32 * s;
            let fz = a.z as f32 * s;
            if fx.abs() > 1e-4 {
                gizmos.arrow(pos, pos + Vec3::X * fx, FORCE_X_COLOR);
            }
            if fy.abs() > 1e-4 {
                gizmos.arrow(pos, pos + Vec3::Y * fy, FORCE_Y_COLOR);
            }
            if fz.abs() > 1e-4 {
                gizmos.arrow(pos, pos + Vec3::Z * fz, FORCE_Z_COLOR);
            }
        }

        // Velocity arrow (green) so you can see drive direction.
        if let Some(v) = vel {
            let dir = Vec3::new(v.0.x as f32, v.0.y as f32, v.0.z as f32) * 3.0;
            if dir.length_squared() > 1e-6 {
                gizmos.arrow(pos, pos + dir, Color::srgb(0.2, 1.0, 0.4));
            }
        }
    };

    for (tf, vel, integ, mass) in q_physical.iter() {
        draw(&mut gizmos, tf, vel, integ, mass);
    }
    for (tf, vel, integ, mass) in q_raycast.iter() {
        draw(&mut gizmos, tf, vel, integ, mass);
    }
}
