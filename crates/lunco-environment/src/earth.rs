//! Earth-direction domain — where Earth stands in the local sky, as a
//! co-simulation source.
//!
//! The exact twin of [`crate::solar`], for the other body a lunar surface asset
//! spends its life pointing at. A high-gain dish is aimed at EARTH, not at the
//! sun, so an antenna model needs the same pair of angles the sun-tracker gets —
//! published the same way, through ordinary `SimComponent` outputs that an
//! authored wire reads.
//!
//! ## Why the provider is a RESOURCE and not a query here
//!
//! The sun bridge can find its provider in the scene: the brightest
//! `DirectionalLight` IS the sun, and its `GlobalTransform` is the answer. Earth
//! emits no key light, so there is nothing in the render world to read. Its
//! direction comes from the ephemeris, which lives in `lunco-celestial` — and
//! `lunco-celestial` depends on THIS crate, so this crate cannot call it.
//!
//! So the resource is declared here and WRITTEN there, exactly as
//! [`LunarSun`](crate::LunarSun) already is: the domain that owns the physics
//! publishes into a slot the domain that owns the ports defined. A scene with no
//! celestial hierarchy simply never has the resource written, and the bridge
//! publishes nothing rather than publishing a guess.

use bevy::prelude::*;

use lunco_cosim::{EARTH_AZIMUTH_CONNECTOR, EARTH_ELEVATION_CONNECTOR};

/// The direction **toward Earth** in world (site-ENU) axes, written each frame
/// by `lunco_celestial`'s sun/sky update once the ecliptic→world rotation is
/// established.
///
/// `None` — the resource absent or holding a zero vector — means "not known",
/// which is the state of every scene that did not opt into the celestial
/// hierarchy, and of an anchored one before its ephemeris resolves. Consumers
/// must treat it as no-data, never as "Earth is at the origin": a zero vector
/// through [`crate::solar::solar_angles`] reads as due north on the horizon,
/// which is a perfectly plausible-looking wrong answer and would park every dish
/// on the skyline.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct EarthDirectionWorld(pub Vec3);

/// Earth's azimuth/elevation as seen from the prim carrying the model, in
/// radians — azimuth in the **MOUNT's own frame**, elevation above the horizon.
///
/// # The frame is the vessel's, not the site's
///
/// A pointing model's output goes to a JOINT, and a revolute joint's angle is
/// measured against its parent body. So an antenna controller handed a
/// site-referenced bearing aims the dish at `site_azimuth + vessel_heading` —
/// correct only while the rover happens to face north, and wrong by exactly the
/// heading the moment it turns. `EarthTracker.mo` says so in its own port doc
/// ("direction to Earth, vessel frame"); this is the frame that makes that true.
///
/// The elevation needs no such correction while vessels stay roughly level; a
/// pitching one would need the full mount frame, and this is where that goes.
///
/// This deliberately differs from [`LocalSolar`](crate::LocalSolar), which is
/// documented and tested as world-axis. The two should converge on the mount
/// frame — a solar tracker on a turning rover has the identical bug — but that
/// is a change to a shipped, test-pinned contract and belongs in its own step,
/// not smuggled in beside a new feature.
///
/// Cached per-entity, which is now load-bearing rather than forward-looking: two
/// models on differently-oriented mounts get genuinely different azimuths.
///
/// Note what this does NOT model: Earth hangs nearly FIXED in the lunar sky
/// (libration wobbles it a few degrees over a month), so an Earth-tracker looks
/// almost static next to a sun-tracker. That is the physics, not a stuck port —
/// which is exactly why the connector carries an angle and not a rate.
#[derive(Component, Debug, Clone, Copy, PartialEq, Reflect, Default)]
#[reflect(Component)]
pub struct LocalEarth {
    /// Earth azimuth in radians, **clockwise from north** (0 = N, +π/2 = E).
    pub azimuth: f64,
    /// Earth elevation in radians (negative below the horizon — the far side).
    pub elevation: f64,
}

/// Computes [`LocalEarth`] for every co-sim model entity from
/// [`EarthDirectionWorld`].
///
/// Change-guarded (writes only when the angles actually move) — mirrors
/// `compute_local_solar` and `compute_local_gravity`. Earth barely moves, so
/// without the guard this would dirty every model entity every tick to write the
/// same two numbers.
pub fn compute_local_earth(
    mut commands: Commands,
    dir: Option<Res<EarthDirectionWorld>>,
    q_targets: Query<
        (Entity, Option<&LocalEarth>, Option<&GlobalTransform>),
        With<lunco_cosim::SimComponent>,
    >,
) {
    if q_targets.is_empty() {
        return;
    }
    // Absent OR degenerate is the same fact: nobody has told us where Earth is.
    let Some(dir) = dir.filter(|d| d.0.is_finite() && d.0.length_squared() > 1.0e-12) else {
        return;
    };
    let site = crate::solar::solar_angles(dir.0);

    for (entity, existing, gt) in &q_targets {
        let next = LocalEarth {
            azimuth: wrap_pi(site.azimuth - mount_yaw(gt)),
            elevation: site.elevation,
        };
        if existing == Some(&next) {
            continue;
        }
        commands.entity(entity).try_insert(next);
    }
}

/// The heading of the prim carrying the model, radians clockwise from north —
/// the amount its own frame is rotated away from the site's.
///
/// Zero for a model with no transform at all (a scene-level program is not
/// mounted on anything, so site frame IS its frame).
fn mount_yaw(gt: Option<&GlobalTransform>) -> f64 {
    let Some(gt) = gt else { return 0.0 };
    let fwd = gt.rotation() * Vec3::NEG_Z;
    // Same convention as `solar_angles`: atan2(east, north) with East=+X, North=−Z.
    (fwd.x as f64).atan2(-fwd.z as f64)
}

/// Wrap to `(-π, π]` — a bearing difference, not an accumulated angle.
fn wrap_pi(a: f64) -> f64 {
    use std::f64::consts::PI;
    let a = (a + PI).rem_euclid(2.0 * PI);
    a - PI
}

/// Publishes each entity's [`LocalEarth`] as `SimComponent` **outputs**
/// [`EARTH_AZIMUTH_CONNECTOR`] / [`EARTH_ELEVATION_CONNECTOR`].
///
/// The authored wire is a SELF-loop on the controller prim — the same shape the
/// sun-tracker uses, and it is not redundant:
///
/// ```usda
/// float inputs:earth_azimuth.connect = </…/EarthTrackerController.outputs:earth_azimuth>
/// ```
///
/// This function fills the *output* half from outside the model; the connection
/// is what carries it into the model's *input* half. Cosim itself never learns
/// what Earth is.
///
/// Writes every tick rather than on change, because a model's own output sync
/// rewrites its outputs map — same reasoning as the gravity and solar bridges.
pub fn inject_local_earth_into_cosim(mut q: Query<(&LocalEarth, &mut lunco_cosim::SimComponent)>) {
    for (earth, mut comp) in &mut q {
        comp.outputs
            .insert(EARTH_AZIMUTH_CONNECTOR.to_string(), earth.azimuth);
        comp.outputs
            .insert(EARTH_ELEVATION_CONNECTOR.to_string(), earth.elevation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The no-data case must publish NOTHING, not zero.
    ///
    /// A zero direction through the angle math is `atan2(0,0) = 0`, `asin(0) = 0`
    /// — due north, on the horizon. That is a legal-looking pair of angles, so a
    /// dish would swing to it and hold, and every symptom would point at the
    /// model rather than at the missing ephemeris.
    #[test]
    fn an_unknown_earth_direction_publishes_no_angles() {
        let mut app = App::new();
        app.insert_resource(EarthDirectionWorld(Vec3::ZERO));
        app.add_systems(Update, compute_local_earth);
        let e = app
            .world_mut()
            .spawn(lunco_cosim::SimComponent::default())
            .id();
        app.update();
        assert!(
            app.world().get::<LocalEarth>(e).is_none(),
            "a degenerate direction is missing data — it must not become 'due north, \
             on the horizon'"
        );
    }

    /// A real direction lands as site-ENU angles, on the shared convention.
    #[test]
    fn a_known_direction_becomes_clockwise_from_north_angles() {
        let mut app = App::new();
        // Due EAST, 30° up: East=+X, Up=+Y.
        app.insert_resource(EarthDirectionWorld(
            Vec3::new(
                30.0_f32.to_radians().cos(),
                30.0_f32.to_radians().sin(),
                0.0,
            )
            .normalize(),
        ));
        app.add_systems(Update, compute_local_earth);
        let e = app
            .world_mut()
            .spawn(lunco_cosim::SimComponent::default())
            .id();
        app.update();
        let got = app
            .world()
            .get::<LocalEarth>(e)
            .copied()
            .expect("published");
        assert!(
            (got.azimuth - std::f64::consts::FRAC_PI_2).abs() < 1e-6,
            "Earth due east must read +π/2 (clockwise from NORTH), got {}",
            got.azimuth
        );
        assert!(got.elevation > 0.0, "above the horizon: {}", got.elevation);
    }

    /// The published azimuth must be relative to the MOUNT, because that is what
    /// a joint on the mount can act on.
    ///
    /// A rover turned 90° east has Earth 90° further round in its own frame than
    /// the site says. Publishing the site bearing anyway aims the dish at
    /// `site + heading`, so it points correctly only while the rover faces north
    /// — which is exactly how it was authored and exactly why the error was
    /// invisible on a stationary rover.
    #[test]
    fn azimuth_is_relative_to_the_mount_not_to_the_site() {
        let mut app = App::new();
        // Earth due NORTH of the site.
        app.insert_resource(EarthDirectionWorld(Vec3::NEG_Z));
        app.add_systems(Update, compute_local_earth);
        // …on a vessel yawed 90° to face EAST (+X).
        let facing_east =
            Transform::from_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2));
        let e = app
            .world_mut()
            .spawn((
                lunco_cosim::SimComponent::default(),
                GlobalTransform::from(facing_east),
            ))
            .id();
        app.update();
        let got = app
            .world()
            .get::<LocalEarth>(e)
            .copied()
            .expect("published");
        assert!(
            (got.azimuth + std::f64::consts::FRAC_PI_2).abs() < 1e-5,
            "facing east with Earth due north, the dish must swing to its own LEFT \
             (−π/2), not to 0: got {}",
            got.azimuth
        );
    }
}
