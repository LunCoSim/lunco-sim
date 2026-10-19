//! Physical celestial samples at the shared communication boundaries.
//!
//! Input publication uses the interval start, when native poses still describe
//! that boundary. Endpoint publication follows native writeback. Both stamp the
//! admitted master tick; presentation cadence never selects a physical epoch.

use bevy::{
    ecs::system::SystemParam,
    math::{DQuat, DVec3},
    prelude::*,
};
use big_space::prelude::{CellCoord, Grid};
use lunco_celestial::frames::Center;
use lunco_celestial::transform::FrameTree;
use lunco_celestial::{CelestialBody, CelestialBodyRegistry, EphemerisResource, ReferenceFrame};
use lunco_celestial_spatial_core::ReferenceFrameIndex;
use lunco_core_runtime::SimTick;
use lunco_physics::PrescribedSimulationPose;
use lunco_spatial::coords::{GridPos, GridRot, grid_relative_pose, pose_in_grid};

pub(crate) fn claim_body(trigger: On<Add, CelestialBody>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .try_insert(PrescribedSimulationPose::Pending);
}

pub(crate) fn release_body(trigger: On<Remove, CelestialBody>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .try_remove::<PrescribedSimulationPose>();
}

#[derive(SystemParam)]
pub(crate) struct CelestialSampling<'w, 's> {
    tick: Res<'w, SimTick>,
    clock: lunco_time::domain::CelestialClock<'w, 's>,
    frame: Option<Res<'w, lunco_spatial::ActivePhysicsFrame>>,
    ephemeris: Option<Res<'w, EphemerisResource>>,
    registry: Res<'w, CelestialBodyRegistry>,
    index: Res<'w, ReferenceFrameIndex>,
    frames: Query<'w, 's, &'static ReferenceFrame>,
    parents: Query<'w, 's, &'static ChildOf>,
    grids: Query<'w, 's, &'static Grid>,
    spatial: Query<'w, 's, (Option<&'static CellCoord>, &'static Transform)>,
    bodies: Query<
        'w,
        's,
        (
            Entity,
            &'static CelestialBody,
            &'static mut PrescribedSimulationPose,
        ),
    >,
    sites: Query<
        'w,
        's,
        &'static lunco_celestial::geo::GeodeticAnchor,
        With<lunco_celestial::geo::SiteAnchor>,
    >,
    calibration: Option<Res<'w, lunco_environment::LunarSun>>,
    sun: ResMut<'w, lunco_environment::SunState>,
    faults: ResMut<'w, lunco_core::RuntimeFaults>,
}

impl CelestialSampling<'_, '_> {
    fn publish_irradiance(
        sun: &mut ResMut<lunco_environment::SunState>,
        faults: &mut ResMut<lunco_core::RuntimeFaults>,
        sample: Option<f64>,
    ) {
        match sun.bypass_change_detection().publish_physical(sample) {
            Ok(true) => sun.set_changed(),
            Ok(false) => {}
            Err(error) => {
                sun.set_changed();
                faults.raise(
                    "celestial-irradiance-invalid",
                    None,
                    "celestial sampling",
                    error,
                );
            }
        }
    }

    fn withdraw_poses(&mut self) {
        for (_, _, mut pose) in &mut self.bodies {
            pose.set_if_neq(PrescribedSimulationPose::Pending);
        }
    }

    fn publish(&mut self, boundary: Result<SimTick, &'static str>) {
        if self.bodies.is_empty() {
            return;
        }
        let epoch = match boundary.and_then(|tick| self.clock.epoch_at_tick(tick)) {
            Ok(epoch) => epoch,
            Err(error) => {
                self.withdraw_poses();
                Self::publish_irradiance(&mut self.sun, &mut self.faults, None);
                self.faults.raise(
                    "celestial-physical-clock-invalid",
                    None,
                    "celestial sampling",
                    error,
                );
                return;
            }
        };
        let Some(ephemeris) = self.ephemeris.as_deref() else {
            self.withdraw_poses();
            Self::publish_irradiance(&mut self.sun, &mut self.faults, None);
            return;
        };
        let tree = FrameTree::new(epoch, &self.registry, ephemeris.provider.as_ref());
        let physical_irradiance = self.sites.single().ok().and_then(|site| {
            let sun = tree.center_in_solar(Center::Body(lunco_celestial::ephemeris_id::SUN))?;
            let observer = tree.center_in_solar(Center::Body(site.body))?;
            let distance_au =
                (sun.raw() - observer.raw()).length() / lunco_celestial::coords::AU_TO_M;
            let calibration = self.calibration.as_deref()?;
            (distance_au > 0.01).then_some(calibration.irradiance_w_m2 / distance_au.powi(2))
        });
        Self::publish_irradiance(&mut self.sun, &mut self.faults, physical_irradiance);
        let Some(frame) = self.frame.as_deref().map(|frame| frame.0) else {
            self.withdraw_poses();
            return;
        };
        let Some(semantic) =
            lunco_celestial::inherited_reference_frame(frame, &self.parents, &self.frames)
        else {
            self.withdraw_poses();
            return;
        };
        let Some(semantic_grid) = self.index.resolve(semantic) else {
            self.withdraw_poses();
            return;
        };
        let Some((offset, local_axes)) = grid_relative_pose(
            frame,
            semantic_grid,
            &self.parents,
            &self.grids,
            &self.spatial,
        ) else {
            self.withdraw_poses();
            return;
        };
        let semantic_pose = |semantic| -> Option<(DVec3, DQuat)> {
            match semantic {
                ReferenceFrame::World => {
                    let world = self.index.resolve(ReferenceFrame::World)?;
                    let solar = self.index.resolve(ReferenceFrame::EclipticJ2000 {
                        center: lunco_celestial::ephemeris_id::SUN,
                    })?;
                    pose_in_grid(world, solar, &self.parents, &self.grids, &self.spatial)
                }
                ReferenceFrame::EclipticJ2000 { center } => Some((
                    tree.center_in_solar(Center::Body(center))?.raw(),
                    DQuat::IDENTITY,
                )),
                ReferenceFrame::BodyFixed { body } => Some((
                    tree.center_in_solar(Center::Body(body))?.raw(),
                    lunco_celestial::geo::body_rotation(self.registry.get(body)?, epoch),
                )),
            }
        };
        let Some((center, axes)) = semantic_pose(semantic) else {
            self.withdraw_poses();
            return;
        };
        let inverse_local_axes = local_axes.normalize().inverse();
        for (entity, body, mut pose) in &mut self.bodies {
            let sample = (|| {
                let position = tree.center_in_solar(Center::Body(body.ephemeris_id))?.raw();
                let source_semantic = lunco_celestial::inherited_reference_frame(
                    entity,
                    &self.parents,
                    &self.frames,
                )?;
                let source_grid = self.index.resolve(source_semantic)?;
                let (_, source_axes) = grid_relative_pose(
                    entity,
                    source_grid,
                    &self.parents,
                    &self.grids,
                    &self.spatial,
                )?;
                let relative_rotation = if source_semantic == semantic {
                    // Identical semantic axes cancel analytically. Passing a
                    // local field through a celestial rotation and its inverse
                    // would introduce epoch-dependent roundoff into that field.
                    source_axes
                } else {
                    let (_, source_rotation) = semantic_pose(source_semantic)?;
                    axes.inverse() * source_rotation * source_axes
                };
                // Subtract celestial centres before applying the local offset:
                // a surface body's centre cancels exactly, without adding then
                // subtracting an astronomical translation around a small offset.
                Some(PrescribedSimulationPose::Sample {
                    frame,
                    tick: *self.tick,
                    position: GridPos(
                        inverse_local_axes * (axes.inverse() * (position - center) - offset),
                    ),
                    rotation: GridRot((inverse_local_axes * relative_rotation).normalize()),
                })
            })();
            pose.set_if_neq(sample.unwrap_or(PrescribedSimulationPose::Pending));
        }
    }
}

pub(crate) fn publish_inputs(mut sampling: CelestialSampling) {
    let _span = bevy::log::info_span!("celestial_physical_inputs").entered();
    let start = sampling
        .tick
        .0
        .checked_sub(1)
        .map(SimTick)
        .ok_or("an admitted simulation interval requires a preceding boundary");
    sampling.publish(start);
}

pub(crate) fn publish_endpoint(mut sampling: CelestialSampling) {
    let _span = bevy::log::info_span!("celestial_physical_endpoint").entered();
    let endpoint = *sampling.tick;
    sampling.publish(Ok(endpoint));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use lunco_celestial::{
        ephemeris_id::{MOON, SUN},
        frames::EclipticAu,
    };
    use lunco_time::{ClockRoot, Clocks, MissionClock, TimeDomain};
    use std::sync::{Arc, Mutex};

    struct RecordedEphemeris(Arc<Mutex<Vec<f64>>>);
    impl lunco_celestial::EphemerisProvider for RecordedEphemeris {
        fn position(&self, body: i32, epoch: f64) -> Option<EclipticAu> {
            self.0.lock().unwrap().push(epoch);
            match body {
                SUN => Some(EclipticAu::ZERO),
                MOON => Some(EclipticAu::new(DVec3::new(1.0, 0.002, 0.001))),
                _ => None,
            }
        }
        fn maximum_angular_rate_rad_per_day(&self) -> f64 {
            f64::INFINITY
        }
    }

    #[test]
    fn physical_samples_use_interval_boundaries_and_cancel_shared_frames() {
        let mut world = World::new();
        world.insert_resource(SimTick(2));
        world.insert_resource(MissionClock::anchored(2_451_545.0, 0));
        world.insert_resource(CelestialBodyRegistry::default_system());
        world.init_resource::<ReferenceFrameIndex>();
        world.init_resource::<lunco_environment::LunarSun>();
        world.init_resource::<lunco_environment::SunState>();
        world.init_resource::<lunco_core::RuntimeFaults>();
        let real = world.spawn((TimeDomain::default(), ClockRoot::Wall)).id();
        let sim = world.spawn((TimeDomain::default(), ClockRoot::Tick)).id();
        let celestial = world
            .spawn(TimeDomain::derived(Some(sim), 0.0, 100_000.0))
            .id();
        world.insert_resource(Clocks {
            real,
            sim,
            interaction: real,
            celestial,
        });
        let calls = Arc::new(Mutex::new(Vec::new()));
        world.insert_resource(EphemerisResource {
            provider: Arc::new(RecordedEphemeris(calls.clone())),
        });
        let make_grid = || lunco_spatial::WorldGridConfig::default().grid();
        let solar = world
            .spawn((
                make_grid(),
                ReferenceFrame::EclipticJ2000 { center: SUN },
                Transform::default(),
            ))
            .id();
        // Deliberately unrelated presentation state must never enter samples.
        let body_frame = world
            .spawn((
                make_grid(),
                ReferenceFrame::BodyFixed { body: MOON },
                Transform::from_xyz(30.0, -20.0, 10.0).with_rotation(Quat::from_rotation_y(0.7)),
                CellCoord::new(300, 200, 100),
                ChildOf(solar),
            ))
            .id();
        let (cell, translation) = make_grid().translation_to_grid(DVec3::Y * 1_737_400.0);
        let frame = world
            .spawn((
                make_grid(),
                Transform::from_translation(translation),
                cell,
                ChildOf(body_frame),
            ))
            .id();
        world.insert_resource(lunco_spatial::ActivePhysicsFrame(frame));
        world.spawn((
            lunco_celestial::geo::SiteAnchor,
            lunco_celestial::geo::GeodeticAnchor {
                body: MOON,
                geodetic: default(),
            },
        ));
        let source = |id| CelestialBody {
            ephemeris_id: id,
            ..default()
        };
        let moon = world
            .spawn((
                source(MOON),
                PrescribedSimulationPose::Pending,
                Transform::default(),
                ChildOf(body_frame),
            ))
            .id();
        let sun = world
            .spawn((
                source(SUN),
                PrescribedSimulationPose::Pending,
                Transform::default(),
                ChildOf(solar),
            ))
            .id();
        world
            .run_system_once(lunco_celestial_spatial_core::update_reference_frame_index)
            .unwrap();
        world.run_system_once(publish_inputs).unwrap();
        let expected_start =
            2_451_545.0 + SimTick(1).seconds() * 100_000.0 / lunco_time::SECS_PER_DAY;
        assert!(!calls.lock().unwrap().is_empty());
        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .all(|epoch| *epoch == expected_start)
        );
        let start_moon = *world.get::<PrescribedSimulationPose>(moon).unwrap();
        assert_eq!(
            start_moon,
            PrescribedSimulationPose::Sample {
                frame,
                tick: SimTick(2),
                position: GridPos(-DVec3::Y * 1_737_400.0),
                rotation: GridRot(DQuat::IDENTITY),
            }
        );
        let start_sun = *world.get::<PrescribedSimulationPose>(sun).unwrap();
        assert!(matches!(start_sun, PrescribedSimulationPose::Sample { .. }));
        calls.lock().unwrap().clear();
        world.run_system_once(publish_endpoint).unwrap();
        let expected_end =
            2_451_545.0 + SimTick(2).seconds() * 100_000.0 / lunco_time::SECS_PER_DAY;
        assert!(!calls.lock().unwrap().is_empty());
        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .all(|epoch| *epoch == expected_end)
        );
        assert_eq!(
            *world.get::<PrescribedSimulationPose>(moon).unwrap(),
            start_moon
        );
        assert_ne!(
            *world.get::<PrescribedSimulationPose>(sun).unwrap(),
            start_sun
        );
        world.clear_trackers();
        world.run_system_once(publish_endpoint).unwrap();
        assert!(
            !world
                .entity(moon)
                .get_ref::<PrescribedSimulationPose>()
                .unwrap()
                .is_changed()
        );
        world.get_mut::<TimeDomain>(celestial).unwrap().parent = Some(real);
        world.run_system_once(publish_endpoint).unwrap();
        assert_eq!(
            *world.get::<PrescribedSimulationPose>(moon).unwrap(),
            PrescribedSimulationPose::Pending
        );
        assert_eq!(
            *world.get::<PrescribedSimulationPose>(sun).unwrap(),
            PrescribedSimulationPose::Pending
        );
        assert!(
            world
                .resource::<lunco_environment::SunState>()
                .irradiance_w_m2
                .is_none()
        );
        assert!(world.resource::<lunco_core::RuntimeFaults>().active());
    }
}
