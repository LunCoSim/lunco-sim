//! # lunco-environment
//!
//! Per-entity environmental state computed from celestial body providers.
//!
//! See `README.md` for the full architecture, rationale, and how to add new
//! environment domains (atmosphere, radiation, magnetic field, etc.).
//!
//! Currently implements **gravity only**. Other domains follow the same
//! pattern — see the README for templates.

use avian3d::prelude::{Forces, Mass, RigidBody, WriteRigidBodyForces};
use bevy::prelude::*;
use bevy::light::GlobalAmbientLight;
use bevy::math::DVec3;
use lunco_celestial::{Gravity, GravityBody, GravityProvider};
use lunco_core::{Command, on_command, register_commands};

/// System sets for environment computation and consumption.
///
/// Ordered chain in [`FixedUpdate`]:
/// 1. [`Compute`](EnvironmentSet::Compute) — write `Local*` components from providers
/// 2. [`Apply`](EnvironmentSet::Apply) — consumers like Avian gravity force application
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnvironmentSet {
    /// Computes per-entity environment components from body providers.
    Compute,
    /// Applies environment effects (e.g., gravity force on RigidBodies).
    Apply,
}

// ─────────────────────────────────────────────────────────────────────────────
// LocalGravity — the gravity vector at an entity's position
// ─────────────────────────────────────────────────────────────────────────────

/// Gravity vector at this entity's position, in world space (m/s²).
///
/// Computed each [`FixedUpdate`] from the [`Gravity`] resource and (for
/// surface gravity) the [`GravityProvider`] on the entity's gravitational
/// parent body (linked via [`GravityBody`]).
///
/// - **Magnitude:** `length()` gives `g` in m/s²
/// - **Direction:** `normalize()` gives the gravity unit vector
///
/// Read this instead of querying the [`Gravity`] resource directly — it's
/// position-dependent and cached. Multiple consumers (Avian force application,
/// cosim input injection, UI display) can read it without recomputation.
#[derive(Component, Debug, Clone, Copy, Reflect, Default)]
#[reflect(Component)]
pub struct LocalGravity(pub DVec3);

impl LocalGravity {
    /// Magnitude in m/s² (always non-negative).
    pub fn magnitude(&self) -> f64 {
        self.0.length()
    }

    /// Unit vector in the direction of gravity (downward).
    /// Returns [`DVec3::NEG_Y`] if the gravity vector is zero.
    pub fn direction(&self) -> DVec3 {
        if self.0.length_squared() > 0.0 {
            self.0.normalize()
        } else {
            DVec3::NEG_Y
        }
    }
}

/// Computes [`LocalGravity`] for every entity that has a [`Transform`].
///
/// Sources the gravity vector from:
/// - [`Gravity::Flat`] — same vector for all entities (sandbox / flat-world)
/// - [`Gravity::Surface`] — per-entity, requires [`GravityBody`] +
///   [`GravityProvider`] on the linked body
pub fn compute_local_gravity(
    mut commands: Commands,
    gravity: Res<Gravity>,
    q_bodies: Query<&GravityProvider>,
    q_entities: Query<(Entity, Ref<Transform>, Option<&GravityBody>, Option<&LocalGravity>)>,
) {
    // Recompute an entity's gravity only when something it depends on changed:
    // the global `Gravity` definition (Flat vector / Flat↔Surface switch) or
    // this entity's own Transform (Surface gravity is position-dependent; Flat
    // is not). Entities that don't yet have a `LocalGravity` always run once.
    // This stops both the per-frame provider lookups and the change-detection
    // storm caused by blindly re-inserting an identical value every frame.
    let gravity_changed = gravity.is_changed();
    for (entity, tf, gravity_body, existing) in &q_entities {
        if existing.is_some() && !gravity_changed && !tf.is_changed() {
            continue;
        }
        let g = match gravity.as_ref() {
            Gravity::Flat { g, direction } => *direction * *g,
            Gravity::Surface => {
                let Some(body_link) = gravity_body else { continue };
                let Ok(provider) = q_bodies.get(body_link.body_entity) else { continue };
                provider.model.acceleration(tf.translation.as_dvec3())
            }
        };
        // Don't re-insert (and re-trigger change detection) when the value is
        // unchanged — e.g. a `gravity_changed` pass that recomputes the same g.
        if let Some(LocalGravity(prev)) = existing {
            if *prev == g {
                continue;
            }
        }
        commands.entity(entity).insert(LocalGravity(g));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Consumer: apply gravity force to Avian RigidBodies
// ─────────────────────────────────────────────────────────────────────────────

/// Applies the cached [`LocalGravity`] vector as a force on every entity that
/// has a [`RigidBody`] and a [`Mass`].
///
/// Replaces the recomputing-each-tick `gravity_system` that previously lived
/// in `lunco-celestial`. Reading `LocalGravity` instead of recomputing means
/// every consumer (this system, cosim injection, future systems) sees the same
/// authoritative value with no duplicated work.
pub fn apply_gravity_to_rigid_bodies(
    q: Query<(Entity, &LocalGravity, &Mass), With<RigidBody>>,
    mut forces: Query<Forces>,
) {
    for (entity, gravity, mass) in &q {
        let force = gravity.0 * mass.0 as f64;
        if let Ok(mut f) = forces.get_mut(entity) {
            f.apply_force(force);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Consumer: feed local gravity into the co-simulation graph
// ─────────────────────────────────────────────────────────────────────────────

/// Publishes each entity's [`LocalGravity`] magnitude as a [`SimComponent`]
/// **output** named [`lunco_cosim::GRAVITY_SOURCE_CONNECTOR`], so co-sim models
/// that take a gravity input (`g`, `gravity`, …) receive the *real* local value
/// through an ordinary output→input wire.
///
/// This is the domain half of keeping `lunco-cosim` pure: the master
/// propagation algorithm has no gravity special-case and no hardcoded constant
/// (it used to inject Earth's `9.81` for a magic `__gravity__` source, which was
/// wrong on the Moon). Gravity now flows like any other signal, correct on any
/// body, because the value comes from the position-dependent `LocalGravity`.
///
/// Runs in [`EnvironmentSet::Apply`] (after `LocalGravity` is computed) and
/// before cosim's propagation, so the freshly-written output is read the same
/// tick. Writes every tick because a model's own output sync may rewrite its
/// outputs map.
pub fn inject_local_gravity_into_cosim(
    mut q: Query<(&LocalGravity, &mut lunco_cosim::SimComponent)>,
) {
    for (gravity, mut comp) in &mut q {
        comp.outputs
            .insert(lunco_cosim::GRAVITY_SOURCE_CONNECTOR.to_string(), gravity.magnitude());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SetEnvironmentLight — runtime sun direction + ambient brightness
// ─────────────────────────────────────────────────────────────────────────────

/// Sets scene environment lighting at runtime: the sun's direction and the
/// global ambient level.
///
/// All three fields are optional — only the ones provided change, the rest
/// keep their current value. So a curl that just lowers the sun looks like:
///
/// ```jsonc
/// {"type":"SetEnvironmentLight","sun_pitch":-0.15}
/// ```
///
/// - **`sun_yaw` / `sun_pitch`** — direction of the single `DirectionalLight`
///   in radians, using the same `EulerRot::YXZ` (yaw-then-pitch) convention as
///   the sandbox settings panel. A small negative `sun_pitch` (e.g. `-0.15`,
///   ~8.5° above the horizon) gives long, raking lunar shadows; `-0.8` is a
///   high ~46° sun with short shadows.
/// - **`ambient_brightness`** — the [`GlobalAmbientLight`] level (the *real*
///   scene-wide fill; the per-camera `AmbientLight` component is only an
///   override). Lower it (~30–60) for deep, high-contrast lunar shadow cores;
///   the airless Moon has near-black shadows.
#[Command(default)]
pub struct SetEnvironmentLight {
    /// Sun azimuth in radians (`EulerRot::YXZ` yaw). `None` keeps current.
    pub sun_yaw: Option<f32>,
    /// Sun elevation in radians (`EulerRot::YXZ` pitch); negative tilts the
    /// light down. `None` keeps current.
    pub sun_pitch: Option<f32>,
    /// Global ambient brightness (cd/m²-scaled). `None` keeps current.
    pub ambient_brightness: Option<f32>,
}

/// Applies a [`SetEnvironmentLight`] command to the live `DirectionalLight`
/// and `GlobalAmbientLight`. Resources/queries are tolerant of absence so the
/// command is a no-op in headless contexts that have no lights.
#[on_command(SetEnvironmentLight)]
fn on_set_environment_light(
    _cmd: SetEnvironmentLight,
    mut q_sun: Query<&mut Transform, With<DirectionalLight>>,
    ambient: Option<ResMut<GlobalAmbientLight>>,
) {
    if cmd.sun_yaw.is_some() || cmd.sun_pitch.is_some() {
        for mut tf in &mut q_sun {
            // Preserve the unspecified axis by reading it back off the current
            // rotation (same YXZ order the sandbox panel writes with).
            let (cur_yaw, cur_pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
            let yaw = cmd.sun_yaw.unwrap_or(cur_yaw);
            let pitch = cmd.sun_pitch.unwrap_or(cur_pitch);
            tf.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
        }
    }

    if let (Some(b), Some(mut ambient)) = (cmd.ambient_brightness, ambient) {
        ambient.brightness = b;
    }
}

register_commands!(on_set_environment_light);

// ─────────────────────────────────────────────────────────────────────────────
// Plugin
// ─────────────────────────────────────────────────────────────────────────────

/// Registers environment components, computation, and consumption systems.
///
/// Add after [`lunco_celestial::GravityPlugin`]. Ordering in `FixedUpdate`:
/// 1. [`EnvironmentSet::Compute`] — writes `LocalGravity` (and future `Local*`)
/// 2. [`EnvironmentSet::Apply`] — applies gravity forces to Avian RigidBodies
pub struct EnvironmentPlugin;

impl Plugin for EnvironmentPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<LocalGravity>();

        // Register environment commands (SetEnvironmentLight). The macro-built
        // `register_all_commands` does `register_type` + `add_observer` so the
        // HTTP/MCP API can dispatch it by reflected type name.
        register_all_commands(app);

        app.configure_sets(
            FixedUpdate,
            (EnvironmentSet::Compute, EnvironmentSet::Apply).chain(),
        );

        app.add_systems(
            FixedUpdate,
            (
                compute_local_gravity.in_set(EnvironmentSet::Compute),
                apply_gravity_to_rigid_bodies.in_set(EnvironmentSet::Apply),
                // Publish gravity into the cosim graph after it's computed and
                // before cosim copies outputs→inputs, so models read the real
                // local value the same tick.
                inject_local_gravity_into_cosim
                    .in_set(EnvironmentSet::Apply)
                    .before(lunco_cosim::systems::propagate::CosimSet::Propagate),
            ),
        );
    }
}
