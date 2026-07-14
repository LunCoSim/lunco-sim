//! Per-entity physics visualization arrows (velocity, force).
//!
//! Mirrors how Unity's PhysicsDebugger and Omniverse's debug-draw
//! layer surface dynamics state: each rigid body that opts in shows
//! its own arrow, multiple can coexist, drawing happens through an
//! immediate-mode gizmo pass with no input or hit-testing. Distinct
//! from the transform gizmo (`gizmo.rs`), which is interactive,
//! singleton, and bound to the active selection.
//!
//! ## Scope today
//!
//! - **Velocity arrow** — `LinearVelocity` * scale, drawn from the
//!   entity's GlobalTransform translation. Always-readable for every
//!   dynamic body; the demo signal that motion is real.
//! - **Force arrow** — `ConstantForce` * scale, same origin. Only
//!   visible on bodies that have an applied constant force (balloon
//!   lift, cosim-driven thrust). Bodies without one simply don't
//!   draw a force arrow even when the flag is on.
//!
//! Both flags live in the per-entity [`PhysicsArrows`] component so a
//! consumer can independently toggle each. Per-class / global
//! toggles can be a thin layer on top of this in a follow-up: stash
//! the desired class name and have a system add/remove the
//! component as entities are spawned.
//!
//! ## Why not in `lunco-viz`?
//!
//! `lunco-viz` is intentionally physics-agnostic so the modelica
//! workbench bin doesn't carry avian3d. This module *needs*
//! avian3d's `LinearVelocity` / `ConstantForce`. Sandbox-edit
//! already depends on avian3d and hosts the transform-gizmo
//! integration, so it's the natural home.

use avian3d::dynamics::integrator::VelocityIntegrationData;
use avian3d::prelude::{ComputedMass, LinearVelocity, RigidBody};
use bevy::prelude::*;
use lunco_core::{Command, on_command, register_commands};

/// Per-entity opt-in for physics-state visualization arrows.
///
/// Independent flags: a wheel might show its velocity but not its
/// applied force (only the chassis has the cosim-driven push), so
/// each toggle is separate.
#[derive(Component, Clone, Copy, Debug, Default, Reflect)]
#[reflect(Component, Default)]
pub struct PhysicsArrows {
    /// Draw a green arrow from the entity's COM in the direction of
    /// `LinearVelocity`, length proportional to magnitude.
    pub velocity: bool,
    /// Draw an orange arrow from the entity's COM in the direction
    /// of `ConstantForce`, length proportional to magnitude. No-op
    /// when the entity has no `ConstantForce` component.
    pub force: bool,
}

impl PhysicsArrows {
    /// Quick constructor: show velocity only.
    pub const fn velocity_only() -> Self {
        Self {
            velocity: true,
            force: false,
        }
    }

    /// Quick constructor: show both.
    pub const fn all() -> Self {
        Self {
            velocity: true,
            force: true,
        }
    }
}

/// Global "show arrows on every dynamic body" toggle. When `enabled`,
/// the [`auto_mark_dynamic_bodies`] system inserts (or updates) a
/// [`PhysicsArrows`] component on every [`RigidBody`] in the scene.
/// Lets the user flip viz on for the whole world without picking
/// each entity — same shape Unity's Physics Debugger uses.
///
/// Per-entity components win over the global flag in the sense that
/// a manually-placed `PhysicsArrows::velocity_only()` keeps its own
/// flags while the global is off; turning the global back on
/// overwrites with the global's flags. Good enough for MVP; a "lock"
/// flag per entity is a small follow-up if it becomes annoying.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct GlobalPhysicsArrows {
    /// When `true`, every `RigidBody` gets a `PhysicsArrows` synced
    /// to the flags below.
    pub enabled: bool,
    /// Show velocity arrows on auto-marked bodies.
    pub velocity: bool,
    /// Show force arrows on auto-marked bodies.
    pub force: bool,
}

/// Typed command to flip the global physics-arrows toggle from the
/// API / scripts / UI buttons.
///
/// Empty / default fields mean "don't change that flag" — but
/// `#[Command(default)]` produces a struct of all-false, so callers
/// who want "only velocity" pass `{"velocity": true}` and the rest
/// stays as supplied (or defaults to false). Idempotent.
#[Command(default)]
pub struct TogglePhysicsArrows {
    /// Master enable.
    pub enabled: bool,
    /// Velocity arrows on every dynamic body when `enabled`.
    pub velocity: bool,
    /// Force arrows on every dynamic body when `enabled`. Ignored
    /// for bodies without a `ConstantForce`.
    pub force: bool,
}

#[on_command(TogglePhysicsArrows)]
fn on_toggle_physics_arrows(
    trigger: On<TogglePhysicsArrows>,
    mut settings: ResMut<GlobalPhysicsArrows>,
) {
    let cmd = trigger.event();
    *settings = GlobalPhysicsArrows {
        enabled: cmd.enabled,
        velocity: cmd.velocity,
        force: cmd.force,
    };
}

register_commands!(on_toggle_physics_arrows,);

/// One-shot Startup system: bias the default gizmo group so arrows
/// render on top of opaque meshes. Without this, the crosshair / arrow
/// drawn at a body's COM is buried inside the body's mesh and invisible.
/// `depth_bias = -1.0` is the canonical "always on top" value for Bevy
/// gizmos.
pub fn configure_gizmo_overlay(mut store: ResMut<bevy::gizmos::config::GizmoConfigStore>) {
    let (config, _) = store.config_mut::<bevy::gizmos::config::DefaultGizmoConfigGroup>();
    config.depth_bias = -1.0;
    config.line.width = 4.0;
}

/// Sync `PhysicsArrows` onto every rigid body when the global toggle
/// flips on; remove it when the toggle flips off. Runs every frame
/// but bails early when settings are stable AND no new bodies need
/// marking — frame-discipline gate per `AGENTS.md` §7.1.
pub fn auto_mark_dynamic_bodies(
    settings: Res<GlobalPhysicsArrows>,
    mut commands: Commands,
    q_unmarked: Query<Entity, (With<RigidBody>, Without<PhysicsArrows>)>,
    q_marked: Query<Entity, With<PhysicsArrows>>,
) {
    if settings.is_changed() {
        if settings.enabled {
            let arrows = PhysicsArrows {
                velocity: settings.velocity,
                force: settings.force,
            };
            for e in q_unmarked.iter() {
                commands.entity(e).try_insert(arrows);
            }
            for e in q_marked.iter() {
                commands.entity(e).try_insert(arrows);
            }
        } else {
            for e in q_marked.iter() {
                commands.entity(e).remove::<PhysicsArrows>();
            }
        }
        return;
    }
    // Settings unchanged — only catch newly-spawned bodies (e.g.
    // sync_usd_visuals just spawned a wheel) so they pick up the
    // global flag.
    if !settings.enabled {
        return;
    }
    let arrows = PhysicsArrows {
        velocity: settings.velocity,
        force: settings.force,
    };
    for e in q_unmarked.iter() {
        commands.entity(e).try_insert(arrows);
    }
}

/// Visual scale applied to each arrow's vector before drawing. Bevy
/// units are metres; raw force magnitudes (kN-scale) would dwarf the
/// scene without scaling. Velocity scaled up so even slow-rolling
/// rover wheels (~0.5 m/s) read at a glance.
const VELOCITY_SCALE: f32 = 3.0;
const FORCE_SCALE: f32 = 0.05;

/// Half-edge length of the always-on marker drawn at every tracked
/// entity's COM. Lets users see which bodies have viz attached even
/// when they're at rest (otherwise zero-magnitude arrows are
/// invisible — not great for the "is it on?" mental check).
const MARKER_HALF: f32 = 0.15;

const VELOCITY_COLOR: Color = Color::srgb(0.2, 1.0, 0.4);
const FORCE_COLOR: Color = Color::srgb(1.0, 0.45, 0.15);
const MARKER_COLOR: Color = Color::srgb(1.0, 1.0, 0.3);

/// Draw the opt-in physics arrows each frame via Bevy's
/// immediate-mode gizmo API. Cheap on the GPU (line draws) and
/// gated by component presence so a 1000-entity scene with zero
/// `PhysicsArrows` pays effectively nothing.
///
/// **Force read source:** uses Avian's `VelocityIntegrationData::linear_increment`
/// (the world-space acceleration accumulated from forces this tick)
/// multiplied by `ComputedMass`. This captures cosim-driven forces
/// (e.g. the Modelica balloon's lift) and any `ConstantForce`,
/// without requiring the body to expose a `ConstantForce` component
/// explicitly. Excludes gravity, contact, and joint forces — those
/// don't flow through the integration accumulator.
pub fn draw_physics_arrows(
    mut gizmos: Gizmos,
    q: Query<(
        &PhysicsArrows,
        &GlobalTransform,
        Option<&LinearVelocity>,
        Option<&VelocityIntegrationData>,
        Option<&ComputedMass>,
    )>,
) {

    for (flags, gtf, vel, integration, mass) in q.iter() {
        // Render arrows from a point ABOVE the body's COM so they're
        // visibly outside the mesh. Without this offset, arrows
        // shorter than the body's bounding box are buried inside.
        let origin = gtf.translation() + Vec3::Y * 1.5;
        // Always-on tri-axis crosshair so the user can see which
        // bodies are tracked even at zero velocity / no force.
        gizmos.line(
            origin - Vec3::X * MARKER_HALF,
            origin + Vec3::X * MARKER_HALF,
            MARKER_COLOR,
        );
        gizmos.line(
            origin - Vec3::Y * MARKER_HALF,
            origin + Vec3::Y * MARKER_HALF,
            MARKER_COLOR,
        );
        gizmos.line(
            origin - Vec3::Z * MARKER_HALF,
            origin + Vec3::Z * MARKER_HALF,
            MARKER_COLOR,
        );
        if flags.velocity {
            if let Some(v) = vel {
                let dir = Vec3::new(v.0.x as f32, v.0.y as f32, v.0.z as f32) * VELOCITY_SCALE;
                if dir.length_squared() > 1e-6 {
                    gizmos.arrow(origin, origin + dir, VELOCITY_COLOR);
                }
            }
        }
        if flags.force {
            if let (Some(integ), Some(m)) = (integration, mass) {
                // F = m * a where a is the world-space acceleration
                // accumulated from forces this tick.
                let a = integ.linear_increment;
                let mass_scalar = m.value() as f32;
                let dir = Vec3::new(a.x as f32, a.y as f32, a.z as f32)
                    * mass_scalar
                    * FORCE_SCALE;
                if dir.length_squared() > 1e-6 {
                    gizmos.arrow(origin, origin + dir, FORCE_COLOR);
                }
            }
        }
    }
}
