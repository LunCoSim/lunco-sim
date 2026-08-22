//! Solar environment domain — the sun's direction as a co-simulation source.
//!
//! The lighting analog of the gravity bridge. The scene **sun** (the brightest
//! non-preview `DirectionalLight`, the same one the horizon shaders and
//! `SetEnvironmentLight` agree on) is the *provider*; this module caches its
//! direction per-entity as [`LocalSolar`] and publishes it into the co-sim graph
//! as ordinary `SimComponent` **outputs**, so a sun-tracking model receives it
//! through a plain output→input wire — the ontology's
//! `RadiationProvider → LocalRadiation → solar models` pipeline.
//!
//! Values are published on explicit [`crate::EnvironmentProbe`] source prims.
//! Models consume them through ordinary USD connections, so provider and
//! consumer remain distinct graph nodes.
//!
//! ## Provider note
//!
//! There is no separate `SolarProvider` component yet: the scene
//! `DirectionalLight` *is* the provider (its direction is the authoritative
//! source, driven by `SetEnvironmentLight`). A richer provider (irradiance
//! model, eclipse occlusion, per-site horizon visibility) would attach here
//! later, exactly as `GravityProvider` carries the gravity model — the
//! [`LocalSolar`] cache already gives each entity its own slot for that.

use bevy::prelude::*;

use lunco_cosim::{SUN_MOUNT_X_CONNECTOR, SUN_MOUNT_Y_CONNECTOR, SUN_MOUNT_Z_CONNECTOR};

/// Unit direction toward the Sun in an entity's authored mount frame.
///
/// The lighting analog of `LocalGravity`. Today the value is global (one sun,
/// no occlusion) so every entity gets the same direction, but it is cached
/// per-entity so a future per-site horizon/eclipse model can vary it without
/// touching consumers.
///
/// The convention is explicit and shared with antenna tracking: `+X` right,
/// `+Y` up, `-Z` forward.  The full world→mount rotation is applied before a
/// model selects joint angles, so vehicle yaw, pitch and roll cannot be
/// mistaken for a solar bearing.
#[derive(Component, Debug, Clone, Copy, PartialEq, Reflect, Default)]
#[reflect(Component)]
pub struct LocalSolar {
    /// Complete world→mount direction, kept as a vector until a consumer needs
    /// its own coordinates.
    pub direction: Vec3,
}

/// Computes [`LocalSolar`] for every explicit environment probe from the scene sun.
///
/// The sun is the brightest `DirectionalLight` without a [`RenderLayers`]
/// scope (preview/RTT suns carry one; max-illuminance also skips the dim
/// earthshine fill). Writes `LocalSolar` only when the angles actually change,
/// to avoid a per-frame change-detection storm — mirrors `compute_local_gravity`.
///
/// Targets entities that carry [`crate::EnvironmentProbe`] so the cache lands
/// exactly where [`inject_local_solar_into_cosim`] will publish it.
pub fn compute_local_solar(
    mut commands: Commands,
    // Structural, not a brightness guess: the scene's sun is the top-level
    // `DistantLight`; a body's reflected fill hangs under that body's prim and
    // carries `Earthshine`. See `lunco_environment::horizon::SunQuery`.
    q_sun: crate::horizon::SunQuery,
    q_targets: Query<
        (Entity, Option<&LocalSolar>, Option<&GlobalTransform>),
        With<crate::EnvironmentProbe>,
    >,
) {
    if q_targets.is_empty() {
        return;
    }
    let Some((sun_gt, _, _)) = crate::horizon::pick_sun(&q_sun) else {
        for (entity, existing, _) in &q_targets {
            if existing.is_some() {
                commands.entity(entity).remove::<LocalSolar>();
            }
        }
        return;
    };

    // `back()` is the direction the light points *from* → toward the sun.
    let d: Vec3 = *sun_gt.back();
    if !d.is_finite() || d.length_squared() < 1e-12 {
        for (entity, existing, _) in &q_targets {
            if existing.is_some() {
                commands.entity(entity).remove::<LocalSolar>();
            }
        }
        return;
    }
    for (entity, existing, mount) in &q_targets {
        let next = LocalSolar {
            direction: crate::mount_frame::direction_in_mount_frame(d, mount),
        };
        if existing == Some(&next) {
            continue;
        }
        commands.entity(entity).try_insert(next);
    }
}

/// Publishes each entity's [`LocalSolar`] as `SimComponent` **outputs**
/// [`SUN_MOUNT_X_CONNECTOR`] / [`SUN_MOUNT_Y_CONNECTOR`] /
/// [`SUN_MOUNT_Z_CONNECTOR`].
///
/// Runs after [`compute_local_solar`] and before cosim propagation, so the
/// fresh outputs are read the same tick. Writes every tick because a model's
/// own output sync may rewrite its outputs map (same reasoning as the gravity
/// bridge). If no scene sun is available, removes only the solar outputs while
/// retaining the schema-declared source contract for later binding.
pub fn inject_local_solar_into_cosim(
    mut q: Query<
        (Option<&LocalSolar>, &mut lunco_cosim::SimComponent),
        With<crate::EnvironmentProbe>,
    >,
) {
    for (solar, mut comp) in &mut q {
        let Some(solar) = solar else {
            comp.outputs.remove(SUN_MOUNT_X_CONNECTOR);
            comp.outputs.remove(SUN_MOUNT_Y_CONNECTOR);
            comp.outputs.remove(SUN_MOUNT_Z_CONNECTOR);
            continue;
        };
        comp.outputs
            .insert(SUN_MOUNT_X_CONNECTOR.to_string(), solar.direction.x as f64);
        comp.outputs
            .insert(SUN_MOUNT_Y_CONNECTOR.to_string(), solar.direction.y as f64);
        comp.outputs
            .insert(SUN_MOUNT_Z_CONNECTOR.to_string(), solar.direction.z as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_sun_removes_cached_local_direction() {
        let mut app = App::new();
        app.add_systems(Update, compute_local_solar);
        let probe = app
            .world_mut()
            .spawn((
                crate::EnvironmentProbe,
                LocalSolar {
                    direction: Vec3::NEG_Z,
                },
            ))
            .id();
        app.update();
        assert!(
            app.world().get::<LocalSolar>(probe).is_none(),
            "a scene without a sun must not retain a stale solar direction"
        );
    }

    #[test]
    fn missing_solar_direction_removes_only_solar_outputs() {
        let mut app = App::new();
        let mut sim = lunco_cosim::SimComponent::default();
        sim.outputs.insert(SUN_MOUNT_X_CONNECTOR.to_owned(), 1.0);
        sim.outputs.insert(SUN_MOUNT_Y_CONNECTOR.to_owned(), 2.0);
        sim.outputs.insert(SUN_MOUNT_Z_CONNECTOR.to_owned(), 3.0);
        sim.outputs
            .insert(lunco_cosim::GRAVITY_SOURCE_CONNECTOR.to_owned(), 9.81);
        let entity = app.world_mut().spawn((crate::EnvironmentProbe, sim)).id();
        app.add_systems(Update, inject_local_solar_into_cosim);

        app.update();

        let outputs = &app
            .world()
            .get::<lunco_cosim::SimComponent>(entity)
            .unwrap()
            .outputs;
        assert!(!outputs.contains_key(SUN_MOUNT_X_CONNECTOR));
        assert!(!outputs.contains_key(SUN_MOUNT_Y_CONNECTOR));
        assert!(!outputs.contains_key(SUN_MOUNT_Z_CONNECTOR));
        assert_eq!(
            outputs.get(lunco_cosim::GRAVITY_SOURCE_CONNECTOR),
            Some(&9.81)
        );
    }
}
