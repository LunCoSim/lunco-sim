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
//! This is the data-driven replacement for the earlier ad-hoc sun-injection
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
/// # Conventions (a sun-tracker that gets these wrong points at the ground)
///
/// Angles are computed from the direction **toward** the sun in scene axes,
/// which `lunco_celestial::geo` fixes as **East = +X, North = −Z, Up = +Y**:
///
/// - `azimuth` — radians **clockwise from NORTH** (the standard solar
///   convention): 0 = north, +π/2 = east, ±π = south. Computed as
///   `atan2(east, north)` = `atan2(d.x, −d.z)`.
/// - `elevation` — `asin(d.y)`; negative when the sun is below the horizon.
///
/// It used to be `atan2(d.x, d.z)`, which reads zero when the sun is along +Z —
/// and +Z is **SOUTH**. Every Modelica sun-tracker consuming
/// [`SOLAR_AZIMUTH_CONNECTOR`] therefore got a south-referenced azimuth (180°
/// out) with nothing anywhere saying so.
///
/// # Precondition on the frame
///
/// These are **world-axis** angles: the direction comes from the scene
/// `DirectionalLight`'s `GlobalTransform`. They equal true site **ENU** angles
/// only in a **site-anchored scene**, where the local scene axes ARE the site's
/// ENU basis (`lunco_celestial::SiteAnchor` — the scene origin sits at a
/// geodetic point with East=+X, North=−Z, Up=+Y). In a scene that is not
/// site-anchored, "north" is the scene's −Z and means nothing geographic. A
/// per-site solar provider that resolves the real tangent frame
/// (`geo::solar_tangent_frame`) is the correct future fix; see the provider note
/// above.
#[derive(Component, Debug, Clone, Copy, PartialEq, Reflect, Default)]
#[reflect(Component)]
pub struct LocalSolar {
    /// Sun azimuth in radians, **clockwise from north** (0 = N, +π/2 = E).
    pub azimuth: f64,
    /// Sun elevation in radians (negative below the horizon).
    pub elevation: f64,
}

/// Sun angles from a unit-ish direction **toward** the sun, in scene axes
/// (East=+X, North=−Z, Up=+Y). The pure core of [`compute_local_solar`].
///
/// Azimuth is clockwise-from-north — see [`LocalSolar`] for why that is not the
/// obvious `atan2(d.x, d.z)`.
pub fn solar_angles(d: Vec3) -> LocalSolar {
    // Promote BEFORE the trig, not after: the outputs are f64 ports, and doing
    // `asin`/`atan2` in f32 and widening the result throws away ~4e-8 rad for
    // nothing (it made a due-east sun read 1.57079637 instead of π/2).
    let d = d.as_dvec3().normalize_or_zero();
    LocalSolar {
        elevation: d.y.clamp(-1.0, 1.0).asin(),
        azimuth: d.x.atan2(-d.z),
    }
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
    let next = solar_angles(d);

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
/// # Port contract (what a Modelica sun-tracker is being handed)
///
/// - `sun_azimuth` — **radians, clockwise from NORTH** (0 = N, +π/2 = E, ±π = S).
/// - `sun_elevation` — radians above the horizon, negative below.
/// - Both are **scene-world** angles, and equal true site ENU angles only in a
///   site-anchored scene. See [`LocalSolar`] for both caveats in full — they are
///   the two things that silently break a tracker, and the azimuth one shipped
///   broken (south-referenced) until now.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    /// **P6 regression — azimuth must be referenced to NORTH.**
    ///
    /// `lunco_celestial::geo` defines the scene axes as East=+X, North=−Z,
    /// Up=+Y. The old `atan2(d.x, d.z)` read **zero when the sun was due
    /// SOUTH**, handing every Modelica sun-tracker a 180°-rotated azimuth.
    ///
    /// Pin all four cardinals against the codebase's own north.
    #[test]
    fn azimuth_is_clockwise_from_north() {
        let cases = [
            ("north", Vec3::new(0.0, 0.0, -1.0), 0.0),          // −Z is NORTH
            ("east", Vec3::new(1.0, 0.0, 0.0), FRAC_PI_2),      // +X is EAST
            ("south", Vec3::new(0.0, 0.0, 1.0), PI),            // +Z is SOUTH
            ("west", Vec3::new(-1.0, 0.0, 0.0), -FRAC_PI_2),
        ];
        for (name, dir, expect) in cases {
            let az = solar_angles(dir).azimuth;
            assert!(
                (az.rem_euclid(2.0 * PI) - expect.rem_euclid(2.0 * PI)).abs() < 1e-9,
                "sun due {name} ({dir:?}) → azimuth {az} rad, expected {expect} \
                 (0 = north, clockwise +)"
            );
        }
    }

    /// Elevation is unchanged by the azimuth fix, and still signed.
    #[test]
    fn elevation_is_signed_about_the_horizon() {
        assert!((solar_angles(Vec3::Y).elevation - FRAC_PI_2).abs() < 1e-9);
        assert!(solar_angles(Vec3::new(0.0, -0.5, -0.866)).elevation < 0.0);
        assert!(solar_angles(Vec3::new(0.0, 0.0, -1.0)).elevation.abs() < 1e-9);
    }
}
