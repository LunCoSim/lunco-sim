//! Earth-direction domain — where Earth stands in the local sky, as a
//! co-simulation source.
//!
//! The exact twin of [`crate::solar`], for the other body a lunar surface asset
//! spends its life pointing at. A high-gain dish is aimed at EARTH, not at the
//! sun, so an antenna model needs the target direction published through ordinary
//! `SimComponent` outputs that an authored wire reads.
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

use lunco_cosim::{EARTH_MOUNT_X_CONNECTOR, EARTH_MOUNT_Y_CONNECTOR, EARTH_MOUNT_Z_CONNECTOR};

/// The direction **toward Earth** in world (site-ENU) axes, written each frame
/// by `lunco_celestial`'s sun/sky update once the ecliptic→world rotation is
/// established.
///
/// `None` — the resource absent or holding a zero vector — means "not known",
/// which is the state of every scene that did not opt into the celestial
/// hierarchy, and of an anchored one before its ephemeris resolves. Consumers
/// must treat it as no-data, never as "Earth is at the origin": a zero vector
/// would otherwise look like a valid direction and park every dish on the
/// skyline.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct EarthDirectionWorld(pub Vec3);

/// Startup frames in which a celestial scene may still be constructing its
/// site frame.  Missing data that resolves during this window is a load-order
/// condition, not an antenna fault.
const EARTH_DIRECTION_WARN_AFTER_FRAMES: u8 = 10;

/// Unit direction toward Earth in the coordinate frame of the prim carrying the
/// model. `+X` is mount-right, `+Y` mount-up, and `-Z` mount-forward.
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
/// This uses the same mount-frame convention as [`LocalSolar`](crate::LocalSolar),
/// so Earth and Sun trackers remain correct as their vehicle turns.
///
/// Cached per-entity, which is now load-bearing rather than forward-looking: two
/// models on differently-oriented mounts get genuinely different directions.
///
/// Note what this does NOT model: Earth hangs nearly FIXED in the lunar sky
/// (libration wobbles it a few degrees over a month), so an Earth-tracker looks
/// almost static next to a sun-tracker. That is the physics, not a stuck port —
/// which is exactly why the connector carries a direction and not a rate.
#[derive(Component, Debug, Clone, Copy, PartialEq, Reflect, Default)]
#[reflect(Component)]
pub struct LocalEarth {
    /// The complete world→mount rotation, not a partial heading correction.
    pub direction: Vec3,
}

/// Computes [`LocalEarth`] for every explicit environment probe from
/// [`EarthDirectionWorld`].
///
/// Change-guarded (writes only when the direction actually moves) — mirrors
/// `compute_local_solar` and `compute_local_gravity`. Earth barely moves, so
/// without the guard this would dirty every model entity every tick to write the
/// same two numbers.
pub fn compute_local_earth(
    mut commands: Commands,
    dir: Option<Res<EarthDirectionWorld>>,
    q_targets: Query<
        (Entity, Option<&LocalEarth>, Option<&GlobalTransform>),
        With<crate::EnvironmentProbe>,
    >,
    // One-shot latch for the no-data diagnostic below. Latched rather than
    // rate-limited: "nobody has said where Earth is" is a structural fact about
    // the scene, so it would otherwise repeat every frame for the scene's life.
    mut warned: Local<bool>,
    mut missing_frames: Local<u8>,
) {
    if q_targets.is_empty() {
        return;
    }
    // Absent OR degenerate is the same fact: nobody has told us where Earth is.
    //
    // SAY SO. Refusing to publish is correct — a zero direction through the angle
    // math is `atan2(0,0) = 0`, due north on the horizon, which a dish would swing
    // to and hold (see the test below). But returning in silence is how an
    // Earth-tracking antenna comes to sit perfectly still on a vehicle that is
    // otherwise working, on every host in the scene at once, with the model
    // compiled, the program bound and every wire landed. There is no other
    // symptom, and nothing downstream can tell "Earth is dead ahead" from "no
    // ephemeris".
    let present = dir.is_some();
    let Some(dir) = dir.filter(|d| d.0.is_finite() && d.0.length_squared() > 1.0e-12) else {
        *missing_frames = missing_frames
            .saturating_add(1)
            .min(EARTH_DIRECTION_WARN_AFTER_FRAMES);
        if !*warned && *missing_frames >= EARTH_DIRECTION_WARN_AFTER_FRAMES {
            *warned = true;
            warn!(
                "[environment] {} co-sim model(s) want a local Earth direction, but \
                 `EarthDirectionWorld` is {} — no Earth-relative port will be published and \
                 every Earth-tracking mechanism will hold its authored pose. This is written \
                 by `lunco-celestial` from the ephemeris: the scene needs celestial content \
                 (a `celestial/solar_system.usda` reference) AND a solar frame it could \
                 actually anchor.",
                q_targets.iter().count(),
                if present { "degenerate" } else { "absent" },
            );
        }
        return;
    };
    *missing_frames = 0;
    if *warned {
        *warned = false;
        info!("[environment] local Earth direction is available again");
    }
    for (entity, existing, gt) in &q_targets {
        // A joint's target is measured in its parent/mount frame.  Subtracting
        // only a compass heading is not a frame transform: once a rover pitches
        // or rolls, it sends site-frame elevation to a mount-frame hinge.  Rotate
        // the world direction through the complete inverse mount attitude first,
        // then use the one shared ENU azimuth/elevation convention on that local
        // vector.  This makes the command invariant under arbitrary vehicle
        // attitude, not merely yaw.
        let mount_direction = crate::mount_frame::direction_in_mount_frame(dir.0, gt);
        let next = LocalEarth {
            direction: mount_direction,
        };
        if existing == Some(&next) {
            continue;
        }
        commands.entity(entity).try_insert(next);
    }
}

/// Publishes each entity's [`LocalEarth`] as `SimComponent` **outputs** in the
/// explicit mount-frame vector convention.
///
/// The authored wire starts on a distinct environment probe:
///
/// ```usda
/// float inputs:earth_mount_x.connect = </…/Environment.outputs:earth_mount_x>
/// ```
///
/// This function fills the probe output; the connection carries it into the
/// model input. Cosim itself never learns what Earth is.
///
/// Writes every tick rather than on change, because a model's own output sync
/// rewrites its outputs map — same reasoning as the gravity and solar bridges.
pub fn inject_local_earth_into_cosim(
    mut q: Query<
        (&LocalEarth, &mut lunco_cosim::SimComponent),
        With<crate::EnvironmentProbe>,
    >,
) {
    for (earth, mut comp) in &mut q {
        comp.outputs.insert(
            EARTH_MOUNT_X_CONNECTOR.to_string(),
            earth.direction.x as f64,
        );
        comp.outputs.insert(
            EARTH_MOUNT_Y_CONNECTOR.to_string(),
            earth.direction.y as f64,
        );
        comp.outputs.insert(
            EARTH_MOUNT_Z_CONNECTOR.to_string(),
            earth.direction.z as f64,
        );
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

    /// An unmounted model receives the world/site direction unchanged.
    #[test]
    fn an_unmounted_model_receives_the_site_direction() {
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
            got.direction.abs_diff_eq(
                Vec3::new(
                    30.0_f32.to_radians().cos(),
                    30.0_f32.to_radians().sin(),
                    0.0
                ),
                1e-6,
            ),
            "unmounted target must remain in the site frame, got {:?}",
            got.direction
        );
    }

    /// The published direction must be relative to the MOUNT, because that is
    /// what a joint on the mount can act on.
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
            got.direction.abs_diff_eq(Vec3::NEG_X, 1e-5),
            "facing east with Earth due north must see Earth to mount-left, got {:?}",
            got.direction
        );
    }

    /// The mount frame is three-dimensional.  A pitch used to leave the site
    /// elevation unchanged, so a rover tipped through 90 degrees still told its
    /// elevation hinge that Earth was on the horizon.  The full inverse mount
    /// rotation must instead report Earth below the antenna's local horizon.
    #[test]
    fn elevation_is_relative_to_the_full_mount_attitude() {
        let mut app = App::new();
        // Earth due north on the site's horizon.
        app.insert_resource(EarthDirectionWorld(Vec3::NEG_Z));
        app.add_systems(Update, compute_local_earth);
        // A 90 degree nose-up pitch maps site-north to the mount's -Y axis.
        let e = app
            .world_mut()
            .spawn((
                lunco_cosim::SimComponent::default(),
                GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_x(
                    std::f32::consts::FRAC_PI_2,
                ))),
            ))
            .id();
        app.update();
        let got = app
            .world()
            .get::<LocalEarth>(e)
            .copied()
            .expect("published");
        assert!(
            got.direction.abs_diff_eq(Vec3::NEG_Y, 1e-5),
            "site-north must be straight below this pitched mount, got {:?}",
            got.direction
        );
    }
}
