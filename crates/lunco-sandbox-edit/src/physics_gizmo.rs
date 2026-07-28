//! Rigid-body dynamics debug gizmo for the SELECTED vessel: center of
//! mass, moments of inertia, applied forces, and body frames.
//!
//! Serves the "operate-and-observe" gap (deep review 2026-07-27 §2.7):
//! a robotics engineer debugging vehicle dynamics selects a rover and
//! sees, for the chassis and every rigid-body descendant (wheels,
//! rockers, articulated links):
//!
//! - **CoM** — a small sphere + cross at each body's world center of
//!   mass (avian `ComputedCenterOfMass`, body-local → world via the
//!   body's `GlobalTransform` rotation + translation).
//! - **Inertia ellipsoid** — the solid ellipsoid with the same mass and
//!   principal moments as the body, drawn as three principal-plane
//!   ellipses + three principal-axis ticks at the CoM. Semi-axes from
//!   the standard inversion `a_i = sqrt(2.5 · (I_j + I_k − I_i) / m)`
//!   (radicand clamped ≥ 0 for degenerate tensors).
//! - **Forces** — arrows, length ∝ newtons (linear, resource-tunable
//!   `newtons_per_meter`, clamped to [`MAX_ARROW_LEN`]):
//!   - tire force (`WheelRaycast::tire_force`) at each wheel hub;
//!   - normal load (`WheelRaycast::last_normal_force`) at the hub —
//!     drawn along the *vessel root's* up axis because the raycast
//!     stores only the scalar, not the contact normal;
//!   - gravity `m·g` at each body's CoM;
//!   - net integrator force (`VelocityIntegrationData × ComputedMass`,
//!     same source as `physics_viz.rs` — captures cosim / constant
//!     forces; excludes gravity, contacts and joint impulses, which
//!     never flow through that accumulator).
//! - **Frames** (separate toggle) — an XYZ triad (RGB = XYZ) at every
//!   rigid body's origin, oriented by its `GlobalTransform` rotation,
//!   plus anchor dots for `RevoluteJoint`s connecting bodies of the
//!   selection. Other joint types are already covered scene-wide by
//!   `joint_viz.rs` (`ToggleJointViz`).
//!
//! ## Legend
//!
//! | element                  | color (tailwind) |
//! |--------------------------|------------------|
//! | CoM marker               | `AMBER_400`      |
//! | inertia ellipsoid + axes | `CYAN_400`       |
//! | tire force               | `ORANGE_500`     |
//! | normal load              | `LIME_400`       |
//! | gravity                  | `PURPLE_400`     |
//! | net integrator force     | `ROSE_400`       |
//! | frame triads             | `RED/GREEN/BLUE_500` (X/Y/Z) |
//! | revolute anchors         | `AMBER_400`      |
//!
//! ## Frame law
//!
//! Everything is drawn from `GlobalTransform` — the render frame —
//! never from avian `Position` (grid-absolute is NOT the render frame,
//! see `lunco_core::coords`). Grid-frame *vectors* (`tire_force`,
//! `Gravity`) are used as directions only: big_space grids never
//! rotate, so grid and render frames share orientation.
//!
//! ## Headless safety
//!
//! This module takes a `Gizmos` system param, which panics without
//! `GizmoPlugin`. It is therefore registered only inside
//! [`crate::SandboxEditPlugin`], which itself is only added by the
//! render-side UI plugin (`lunco-sandbox`'s `ui` module) — the same
//! gate `physics_viz.rs` and `joint_viz.rs` live behind. Headless
//! hosts link only `lunco-scene-commands` and never see this code.
//!
//! ## Toggle path
//!
//! Off by default. Three independent layers (`show_mass` = CoM +
//! inertia, `show_forces` = force arrows, `show_frames` = triads +
//! pins), each reachable three equivalent ways:
//! - Workbench **Settings menu → Debug Visualization → "Selected-body
//!   mass" / "Selected-body forces" / "Selected-body frames"**
//!   (registered in `ui/mod.rs`);
//! - typed command, UI/API/Rhai parity:
//!   `cmd("TogglePhysicsGizmo", #{show_mass: true, show_forces: true, show_frames: true})`;
//! - the [`PhysicsGizmoSettings`] resource directly.

use avian3d::dynamics::integrator::VelocityIntegrationData;
use avian3d::prelude::{
    ComputedAngularInertia, ComputedCenterOfMass, ComputedMass, Gravity, JointAnchor, JointFrame,
    RevoluteJoint, RigidBody,
};
use bevy::color::palettes::tailwind;
use bevy::prelude::*;
use lunco_core::coords::ancestor_grid_anchor;
use lunco_core::{on_command, register_commands, Command, GridAnchor};
use lunco_mobility::WheelRaycast;

use crate::SelectedEntities;

// ── Settings resource + typed command ────────────────────────────────────

/// Global toggle + tuning for the selected-body dynamics gizmo.
///
/// Off by default; flip via the workbench Settings menu, the
/// [`TogglePhysicsGizmo`] command, or this resource directly.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct PhysicsGizmoSettings {
    /// Draw CoM markers + inertia ellipsoids/axes for the selected
    /// vessel and its rigid-body descendants.
    pub show_mass: bool,
    /// Draw force arrows (tire, normal load, gravity, net integrator)
    /// for the same set. Independent of `show_mass` so a mass-budget
    /// pass isn't cluttered by arrows and vice versa.
    pub show_forces: bool,
    /// Draw body-frame XYZ triads (+ revolute anchors) for the same
    /// set. Independent so an engineer chasing a frame-mixing bug can
    /// see triads without the dynamics clutter.
    pub show_frames: bool,
    /// Force-arrow scale: how many newtons map to one metre of arrow.
    /// Linear, then clamped to [`MAX_ARROW_LEN`]. 500 N/m makes a
    /// ~150 kg lunar rover's per-wheel load (~60 N) read at ~12 cm and
    /// an Earth-weight chassis (~1.5 kN) saturate visibly.
    pub newtons_per_meter: f32,
}

impl Default for PhysicsGizmoSettings {
    fn default() -> Self {
        Self {
            show_mass: false,
            show_forces: false,
            show_frames: false,
            newtons_per_meter: 500.0,
        }
    }
}

/// Toggle the selected-body dynamics / frames gizmo.
///
/// `#[Command(default)]` → all-false; pass only the flags you want on.
/// Rhai: `cmd("TogglePhysicsGizmo", #{show_mass: true, show_forces: true})`.
/// Leaves `newtons_per_meter` untouched.
#[Command(default)]
pub struct TogglePhysicsGizmo {
    /// CoM + inertia layer.
    pub show_mass: bool,
    /// Force-arrows layer.
    pub show_forces: bool,
    /// Body-frame triads layer.
    pub show_frames: bool,
}

#[on_command(TogglePhysicsGizmo)]
fn on_toggle_physics_gizmo(
    trigger: On<TogglePhysicsGizmo>,
    mut settings: ResMut<PhysicsGizmoSettings>,
) {
    let cmd = trigger.event();
    settings.show_mass = cmd.show_mass;
    settings.show_forces = cmd.show_forces;
    settings.show_frames = cmd.show_frames;
}

register_commands!(on_toggle_physics_gizmo,);

// ── Visual constants ─────────────────────────────────────────────────────

/// Longest force arrow, metres — keeps a 10 kN spike on the screen
/// instead of across the crater.
const MAX_ARROW_LEN: f32 = 4.0;
/// CoM sphere radius, metres.
const COM_RADIUS: f32 = 0.05;
/// CoM cross half-edge, metres.
const COM_CROSS_HALF: f32 = 0.12;
/// Frame-triad axis length, metres.
const TRIAD_LEN: f32 = 0.5;
/// Revolute anchor dot radius, metres.
const ANCHOR_RADIUS: f32 = 0.05;
/// Recursion budget for the selection subtree walk.
const MAX_SUBTREE: usize = 512;

const COM_COLOR: Srgba = tailwind::AMBER_400;
const INERTIA_COLOR: Srgba = tailwind::CYAN_400;
const TIRE_FORCE_COLOR: Srgba = tailwind::ORANGE_500;
const NORMAL_FORCE_COLOR: Srgba = tailwind::LIME_400;
const GRAVITY_COLOR: Srgba = tailwind::PURPLE_400;
const NET_FORCE_COLOR: Srgba = tailwind::ROSE_400;
const AXIS_X_COLOR: Srgba = tailwind::RED_500;
const AXIS_Y_COLOR: Srgba = tailwind::GREEN_500;
const AXIS_Z_COLOR: Srgba = tailwind::BLUE_500;

// ── Selection scope ──────────────────────────────────────────────────────

/// Resolve the primary selection to its vessel root ([`GridAnchor`]
/// ancestor when one exists — clicks are already anchor-resolved, but
/// API `SelectEntity` may hand us a sub-part) and collect the root plus
/// every descendant, capped at [`MAX_SUBTREE`]. Returns `None` when
/// nothing is selected — the gizmo then draws nothing.
fn selected_subtree(
    selected: &SelectedEntities,
    q_parents: &Query<&ChildOf>,
    q_anchors: &Query<(), With<GridAnchor>>,
    q_children: &Query<&Children>,
) -> Option<Vec<Entity>> {
    let primary = selected.primary()?;
    let root = ancestor_grid_anchor(primary, q_parents, q_anchors).unwrap_or(primary);
    let mut out = Vec::with_capacity(32);
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if out.len() >= MAX_SUBTREE {
            break;
        }
        out.push(e);
        if let Ok(children) = q_children.get(e) {
            stack.extend(children.iter());
        }
    }
    Some(out)
}

/// Linear force→arrow mapping: `|F| / newtons_per_meter`, clamped to
/// [`MAX_ARROW_LEN`]. Returns the scaled vector, or `None` when the
/// force is too small to draw.
fn force_arrow(force: Vec3, newtons_per_meter: f32) -> Option<Vec3> {
    let n = force.length();
    if n < 1e-3 || newtons_per_meter <= 0.0 {
        return None;
    }
    let len = (n / newtons_per_meter).min(MAX_ARROW_LEN);
    Some(force / n * len)
}

// ── Dynamics layer: CoM + inertia (`show_mass`) + forces (`show_forces`) ─

/// Draw CoM markers + equivalent-inertia ellipsoids (`show_mass`) and
/// force arrows (`show_forces`) for the selected vessel's rigid bodies.
/// Early-returns when both flags are off or nothing is selected; bodies
/// missing avian mass data are skipped silently (all physics components
/// are `Option`).
#[allow(clippy::too_many_arguments)]
pub fn draw_physics_gizmo(
    mut gizmos: Gizmos,
    settings: Res<PhysicsGizmoSettings>,
    selected: Res<SelectedEntities>,
    q_parents: Query<&ChildOf>,
    q_anchors: Query<(), With<GridAnchor>>,
    q_children: Query<&Children>,
    q_bodies: Query<
        (
            &GlobalTransform,
            Option<&ComputedMass>,
            Option<&ComputedCenterOfMass>,
            Option<&ComputedAngularInertia>,
            Option<&VelocityIntegrationData>,
        ),
        With<RigidBody>,
    >,
    q_wheels: Query<(&WheelRaycast, &GlobalTransform)>,
    q_gtf: Query<&GlobalTransform>,
    gravity: Option<Res<Gravity>>,
) {
    if !settings.show_mass && !settings.show_forces {
        return;
    }
    let Some(subtree) = selected_subtree(&selected, &q_parents, &q_anchors, &q_children) else {
        return;
    };
    // Root up axis — fallback direction for the scalar normal load.
    let root_up = q_gtf
        .get(subtree[0])
        .map(|tf| tf.rotation() * Vec3::Y)
        .unwrap_or(Vec3::Y);
    let g = gravity.map(|g| g.0.as_vec3());
    let npm = settings.newtons_per_meter;

    for &entity in &subtree {
        // Rigid-body layer: CoM, inertia, gravity, net force.
        if let Ok((gtf, mass, com, inertia, integ)) = q_bodies.get(entity) {
            let rot = gtf.rotation();
            // CoM is a body-local physical point: rotation + translation
            // only — GlobalTransform scale is a render affordance and
            // must not stretch a mass property.
            let com_world = com
                .map(|c| gtf.translation() + rot * c.0.as_vec3())
                .unwrap_or_else(|| gtf.translation());

            // CoM marker — sphere + cross so it reads at any zoom.
            if settings.show_mass {
                gizmos.sphere(com_world, COM_RADIUS, COM_COLOR);
                for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                    gizmos.line(
                        com_world - axis * COM_CROSS_HALF,
                        com_world + axis * COM_CROSS_HALF,
                        COM_COLOR,
                    );
                }
            }

            // Equivalent solid ellipsoid from the principal moments.
            if let (true, Some(m), Some(inertia)) = (settings.show_mass, mass, inertia) {
                let m = m.value() as f32;
                if m > 0.0 {
                    let (principal, local_frame) =
                        inertia.principal_angular_inertia_with_local_frame();
                    let (i1, i2, i3) = (principal.x as f32, principal.y as f32, principal.z as f32);
                    // a_i = sqrt(2.5 (I_j + I_k − I_i) / m), radicand ≥ 0.
                    let semi = Vec3::new(
                        (2.5 * (i2 + i3 - i1) / m).max(0.0).sqrt(),
                        (2.5 * (i3 + i1 - i2) / m).max(0.0).sqrt(),
                        (2.5 * (i1 + i2 - i3) / m).max(0.0).sqrt(),
                    );
                    let world_q = rot * local_frame.as_quat();
                    // Three principal-plane ellipses. Each permutation
                    // maps the ellipse's local XY plane onto a principal
                    // plane (right-handed cyclic, det = +1).
                    let planes = [
                        // XY plane, normal Z.
                        (Quat::IDENTITY, Vec2::new(semi.x, semi.y)),
                        // YZ plane, normal X.
                        (
                            Quat::from_mat3(&Mat3::from_cols(Vec3::Y, Vec3::Z, Vec3::X)),
                            Vec2::new(semi.y, semi.z),
                        ),
                        // ZX plane, normal Y.
                        (
                            Quat::from_mat3(&Mat3::from_cols(Vec3::Z, Vec3::X, Vec3::Y)),
                            Vec2::new(semi.z, semi.x),
                        ),
                    ];
                    for (perm, half) in planes {
                        if half.x > 1e-4 && half.y > 1e-4 {
                            gizmos.ellipse(
                                Isometry3d::new(com_world, world_q * perm),
                                half,
                                INERTIA_COLOR,
                            );
                        }
                    }
                    // Principal-axis ticks, length = the semi-axis.
                    for (axis, len) in [(Vec3::X, semi.x), (Vec3::Y, semi.y), (Vec3::Z, semi.z)] {
                        if len > 1e-4 {
                            let dir = world_q * axis * len;
                            gizmos.line(com_world - dir, com_world + dir, INERTIA_COLOR);
                        }
                    }
                }
            }

            if let (true, Some(m)) = (settings.show_forces, mass) {
                let m = m.value() as f32;
                // Gravity m·g at the CoM. `Gravity` is a grid-frame
                // vector; grids never rotate, so it is direction-valid
                // in the render frame as-is.
                if let Some(g) = g {
                    if let Some(v) = force_arrow(g * m, npm) {
                        gizmos.arrow(com_world, com_world + v, GRAVITY_COLOR);
                    }
                }
                // Net integrator force — accumulated external forces
                // (cosim thrust, ConstantForce). Same read as
                // `physics_viz.rs`; avian's `Forces` accumulator itself
                // is not readably exposed outside the physics schedule.
                if let Some(integ) = integ {
                    let a = integ.linear_increment;
                    let f = Vec3::new(a.x as f32, a.y as f32, a.z as f32) * m;
                    if let Some(v) = force_arrow(f, npm) {
                        gizmos.arrow(com_world, com_world + v, NET_FORCE_COLOR);
                    }
                }
            }
        }

        // Wheel layer: tire + normal forces at the hub. `tire_force`
        // is a grid-frame vector (direction-valid in render, see
        // above); the hub anchor point comes from `GlobalTransform`.
        if !settings.show_forces {
            continue;
        }
        if let Ok((wheel, gtf)) = q_wheels.get(entity) {
            let hub = gtf.translation();
            let tire = wheel.tire_force.as_vec3();
            if let Some(v) = force_arrow(tire, npm) {
                gizmos.arrow(hub, hub + v, TIRE_FORCE_COLOR);
            }
            // Scalar-only normal load: the raycast keeps no contact
            // normal, so draw along the vessel root's up axis.
            let normal = root_up * wheel.last_normal_force as f32;
            if let Some(v) = force_arrow(normal, npm) {
                gizmos.arrow(hub, hub + v, NORMAL_FORCE_COLOR);
            }
        }
    }
}

// ── Frames layer: body triads + revolute anchors ─────────────────────────

/// Draw an XYZ triad (RGB = XYZ) at every rigid body of the selection,
/// oriented by its `GlobalTransform` rotation, plus anchor dots for
/// revolute joints whose both bodies belong to the selection. The
/// direct countermeasure to frame-mixing bugs: what you see IS the
/// render-frame pose the body actually has.
pub fn draw_frame_gizmo(
    mut gizmos: Gizmos,
    settings: Res<PhysicsGizmoSettings>,
    selected: Res<SelectedEntities>,
    q_parents: Query<&ChildOf>,
    q_anchors: Query<(), With<GridAnchor>>,
    q_children: Query<&Children>,
    q_bodies: Query<&GlobalTransform, With<RigidBody>>,
    q_revolute: Query<&RevoluteJoint>,
) {
    if !settings.show_frames {
        return;
    }
    let Some(subtree) = selected_subtree(&selected, &q_parents, &q_anchors, &q_children) else {
        return;
    };

    for &entity in &subtree {
        let Ok(gtf) = q_bodies.get(entity) else {
            continue;
        };
        let origin = gtf.translation();
        let rot = gtf.rotation();
        gizmos.line(origin, origin + rot * Vec3::X * TRIAD_LEN, AXIS_X_COLOR);
        gizmos.line(origin, origin + rot * Vec3::Y * TRIAD_LEN, AXIS_Y_COLOR);
        gizmos.line(origin, origin + rot * Vec3::Z * TRIAD_LEN, AXIS_Z_COLOR);
    }

    // Revolute anchors between selected bodies (the suspension pins an
    // engineer actually cares about). Other joint kinds stay with the
    // scene-wide `joint_viz.rs` layer.
    for j in q_revolute.iter() {
        if !(subtree.contains(&j.body1) && subtree.contains(&j.body2)) {
            continue;
        }
        for (frame, body) in [(&j.frame1, j.body1), (&j.frame2, j.body2)] {
            let Ok(tf) = q_bodies.get(body) else {
                continue;
            };
            let world = anchor_world(frame, tf);
            gizmos.sphere(world, ANCHOR_RADIUS, COM_COLOR);
        }
    }
}

/// World-space anchor of a [`JointFrame`] given the owning body's
/// `GlobalTransform` (same shape as `joint_viz::anchor_world`, which is
/// private to that module).
fn anchor_world(frame: &JointFrame, body_tf: &GlobalTransform) -> Vec3 {
    let to_vec3 = |v: avian3d::math::Vector| Vec3::new(v.x as f32, v.y as f32, v.z as f32);
    match frame.anchor {
        JointAnchor::Local(v) => body_tf.transform_point(to_vec3(v)),
        JointAnchor::FromGlobal(v) => to_vec3(v),
    }
}
