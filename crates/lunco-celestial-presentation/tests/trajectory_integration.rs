use std::sync::Arc;

use bevy::prelude::*;
use lunco_celestial::{EphemerisProvider, EphemerisResource};
use lunco_celestial_presentation::CelestialPresentationPlugin;
use lunco_celestial_spatial::CelestialPlugin;

#[derive(Debug)]
struct StubEphemeris;

impl EphemerisProvider for StubEphemeris {
    fn position(
        &self,
        _body_id: i32,
        epoch_jd: f64,
    ) -> Option<lunco_celestial::frames::EclipticAu> {
        Some(lunco_celestial::frames::EclipticAu::new(
            bevy::math::DVec3::new(epoch_jd, 0.0, 0.0),
        ))
    }

    fn maximum_angular_rate_rad_per_day(&self) -> f64 {
        0.0
    }

    fn motion_revision(&self) -> u64 {
        0
    }
}

fn presentation_test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::input::InputPlugin);
    app.add_plugins(bevy::transform::TransformPlugin);
    let _ = lunco_assets_runtime::register_lunco_asset_sources(&mut app);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.init_resource::<Assets<Mesh>>();
    app.init_asset::<Image>();
    app.add_plugins(CelestialPlugin);
    app.add_plugins(CelestialPresentationPlugin);
    for naif in [
        lunco_celestial::ephemeris_id::SUN,
        lunco_celestial::ephemeris_id::EARTH,
        lunco_celestial::ephemeris_id::MOON,
    ] {
        app.world_mut()
            .spawn(lunco_celestial_spatial_core::CelestialBodyDecl { naif });
    }
    app
}

#[test]
fn trajectories_mount_only_in_their_declared_frame_class() {
    let mut app = presentation_test_app();
    app.insert_resource(EphemerisResource {
        provider: Arc::new(StubEphemeris),
    });
    app.update();
    app.update();

    let moon = lunco_celestial::ephemeris_id::MOON;
    let earth = lunco_celestial::ephemeris_id::EARTH;
    let body_fixed = app
        .world_mut()
        .spawn((
            lunco_celestial_spatial_core::TrajectoryView {
                tracked_id: earth,
                reference_id: moon,
                frame: lunco_celestial_spatial_core::TrajectoryFrame::BodyFixed,
                ..Default::default()
            },
            lunco_celestial_spatial_core::TrajectoryPath::default(),
            Transform::default(),
            GlobalTransform::default(),
        ))
        .id();
    let inertial = app
        .world_mut()
        .spawn((
            lunco_celestial_spatial_core::TrajectoryView {
                tracked_id: -10_001,
                reference_id: moon,
                frame: lunco_celestial_spatial_core::TrajectoryFrame::Inertial,
                ..Default::default()
            },
            lunco_celestial_spatial_core::TrajectoryPath::default(),
            Transform::default(),
            GlobalTransform::default(),
        ))
        .id();

    app.update();
    app.update();

    let fixed_parent = app.world().get::<ChildOf>(body_fixed).unwrap().parent();
    let fixed_frame = app
        .world()
        .get::<lunco_celestial::ReferenceFrame>(fixed_parent)
        .expect("a body-fixed trajectory must parent to a body-fixed frame Grid");
    assert_eq!(
        *fixed_frame,
        lunco_celestial::ReferenceFrame::BodyFixed { body: moon }
    );

    let inertial_parent = app.world().get::<ChildOf>(inertial).unwrap().parent();
    let inertial_frame = app
        .world()
        .get::<lunco_celestial::ReferenceFrame>(inertial_parent)
        .expect("an inertial trajectory must parent to an inertial frame Grid");
    assert_eq!(
        *inertial_frame,
        lunco_celestial::ReferenceFrame::EclipticJ2000 { center: moon }
    );
}
