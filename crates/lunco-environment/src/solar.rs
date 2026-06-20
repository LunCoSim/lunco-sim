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
//! This is the data-driven replacement for the ad-hoc `inject_sun_signals`
//! prototype (which pushed straight into model *inputs*, bypassing wires). Here
//! the value flows like any other signal: cosim stays domain-agnostic, and the
//! USD wiring is explicit (`sun_azimuth` out → `sun_azimuth` in).
//!
//! ## Provider note
//!
//! There is no separate `SolarProvider` component yet: the scene
//! `DirectionalLight` *is* the provider (its direction is the authoritative
//! source, driven by `SetEnvironmentLight`). A richer provider (irradiance
//! model, eclipse occlusion, per-site horizon visibility) would attach here
//! later, exactly as `GravityProvider` carries the gravity model — the
//! [`LocalSolar`] cache already gives each entity its own slot for that.

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;

use lunco_cosim::{SOLAR_AZIMUTH_CONNECTOR, SOLAR_ELEVATION_CONNECTOR};

/// The sun direction sampled at an entity's location: azimuth and elevation in
/// radians.
///
/// The lighting analog of `LocalGravity`. Today the value is global (one sun,
/// no occlusion) so every entity gets the same angles, but it is cached
/// per-entity so a future per-site horizon/eclipse model can vary it without
/// touching consumers.
///
/// - `azimuth` — `atan2(dir.x, dir.z)` of the direction *toward* the sun.
/// - `elevation` — `asin(dir.y)`; negative when the sun is below the horizon.
#[derive(Component, Debug, Clone, Copy, PartialEq, Reflect, Default)]
#[reflect(Component)]
pub struct LocalSolar {
    /// Sun azimuth in radians.
    pub azimuth: f64,
    /// Sun elevation in radians (negative below the horizon).
    pub elevation: f64,
}

/// Computes [`LocalSolar`] for every co-sim model entity from the scene sun.
///
/// The sun is the brightest `DirectionalLight` without a [`RenderLayers`]
/// scope (preview/RTT suns carry one; max-illuminance also skips the dim
/// earthshine fill). Writes `LocalSolar` only when the angles actually change,
/// to avoid a per-frame change-detection storm — mirrors `compute_local_gravity`.
///
/// Targets entities that carry a [`lunco_cosim::SimComponent`] (the co-sim
/// models) so the cache lands exactly where [`inject_local_solar_into_cosim`]
/// will publish it.
pub fn compute_local_solar(
    mut commands: Commands,
    q_sun: Query<(&GlobalTransform, &DirectionalLight), Without<RenderLayers>>,
    q_targets: Query<(Entity, Option<&LocalSolar>), With<lunco_cosim::SimComponent>>,
) {
    if q_targets.is_empty() {
        return;
    }
    let Some((sun_gt, _)) = q_sun
        .iter()
        .max_by(|a, b| a.1.illuminance.total_cmp(&b.1.illuminance))
    else {
        return;
    };

    // `back()` is the direction the light points *from* → toward the sun.
    let d: Vec3 = *sun_gt.back();
    if !d.is_finite() || d.length_squared() < 1e-12 {
        return;
    }
    let elevation = d.y.clamp(-1.0, 1.0).asin() as f64;
    let azimuth = d.x.atan2(d.z) as f64;
    let next = LocalSolar { azimuth, elevation };

    for (entity, existing) in &q_targets {
        if existing == Some(&next) {
            continue;
        }
        commands.entity(entity).insert(next);
    }
}

/// Publishes each entity's [`LocalSolar`] as `SimComponent` **outputs**
/// [`SOLAR_AZIMUTH_CONNECTOR`] / [`SOLAR_ELEVATION_CONNECTOR`], so a model that
/// takes a sun input receives the real value through an ordinary output→input
/// wire.
///
/// Runs after [`compute_local_solar`] and before cosim propagation, so the
/// fresh outputs are read the same tick. Writes every tick because a model's
/// own output sync may rewrite its outputs map (same reasoning as the gravity
/// bridge).
pub fn inject_local_solar_into_cosim(
    mut q: Query<(&LocalSolar, &mut lunco_cosim::SimComponent)>,
) {
    for (solar, mut comp) in &mut q {
        comp.outputs
            .insert(SOLAR_AZIMUTH_CONNECTOR.to_string(), solar.azimuth);
        comp.outputs
            .insert(SOLAR_ELEVATION_CONNECTOR.to_string(), solar.elevation);
    }
}
