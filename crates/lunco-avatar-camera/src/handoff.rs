//! Scene-owned avatar camera handoff into the authored spatial frame.
//!
//! USD projection first creates the local avatar in the persistent world shell.
//! This module captures that authored pose and commits the migration after the
//! scene's site Grid is ready. BigSpace coordinate composition stays at the
//! spatial camera boundary; the avatar authority runtime does not perform this
//! scene-frame conversion.

use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_celestial::{GeodeticAnchor, SiteAnchor};
use lunco_core::CelestialBody as CoreCelestialBody;
use lunco_embodiment_core::roles::{Embodiment, LocalEmbodiment};
use lunco_environment::GravityBody;
use lunco_spatial::SceneSpatialHandoffSet;
use lunco_spatial::attach::migrate_to_grid_local_pose;

/// Pose captured while a local avatar is still attached to the loader's world
/// Grid. The value is local to the authored site frame, so it remains valid
/// when the site root becomes or is already a BigSpace Grid.
#[derive(Component, Clone, Copy, Debug)]
struct PendingSiteCameraPose {
    site_root: Entity,
    position: DVec3,
    rotation: DQuat,
}

/// Install the ordered scene-to-site handoff systems.
pub(crate) fn register(app: &mut App) {
    app.configure_sets(Update, SceneSpatialHandoffSet);
    app.add_systems(
        Update,
        (
            capture_site_camera_pose.run_if(site_camera_capture_changed),
            bind_local_avatar_to_site_grid.run_if(avatar_site_handoff_changed),
        )
            .chain()
            .in_set(SceneSpatialHandoffSet),
    );
}

fn site_camera_capture_changed(
    q_site: Query<(), With<SiteAnchor>>,
    q_avatar: Query<
        (),
        (
            With<Embodiment>,
            With<LocalEmbodiment>,
            Without<PendingSiteCameraPose>,
            Or<(
                Changed<Embodiment>,
                Changed<LocalEmbodiment>,
                Changed<ChildOf>,
                Changed<Transform>,
            )>,
        ),
    >,
) -> bool {
    !q_site.is_empty() && !q_avatar.is_empty()
}

/// Capture an authored local-camera pose before the binder migrates the avatar
/// into the site frame. The shared common-grid conversion also handles a USD
/// avatar projected after the site root has already moved beneath a celestial
/// surface Grid.
fn capture_site_camera_pose(
    q_site: Query<Entity, With<SiteAnchor>>,
    q_avatar: Query<
        (Entity, &ChildOf, Option<&PendingSiteCameraPose>),
        (With<Embodiment>, With<LocalEmbodiment>),
    >,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_world_grid: Query<(), With<lunco_spatial::WorldGrid>>,
    mut commands: Commands,
) {
    let Ok(site_root) = q_site.single() else {
        return;
    };

    for (avatar, child_of, pending) in &q_avatar {
        if pending.is_some() {
            continue;
        }
        let current_parent = child_of.parent();
        if q_grids.get(current_parent).is_ok() && q_world_grid.get(current_parent).is_err() {
            continue;
        }
        let pose = lunco_spatial::coords::common_grid_poses(
            avatar, site_root, &q_parents, &q_grids, &q_spatial,
        )
        .map(
            |(_, avatar_position, avatar_rotation, site_position, site_rotation)| {
                let inverse_site_rotation = site_rotation.inverse();
                (
                    inverse_site_rotation * (avatar_position - site_position),
                    (inverse_site_rotation * avatar_rotation).normalize(),
                )
            },
        );
        let Some((avatar_position, avatar_rotation)) = pose else {
            warn!(
                ?avatar,
                ?site_root,
                "local avatar cannot be composed with the authored site root"
            );
            continue;
        };
        commands.entity(avatar).try_insert(PendingSiteCameraPose {
            site_root,
            position: avatar_position,
            rotation: avatar_rotation.normalize(),
        });
        info!(
            ?avatar,
            ?site_root,
            "captured local avatar pose for site camera handoff"
        );
    }
}

/// Run the startup camera handoff when either side of the scene/camera
/// boundary changes. This keeps the steady-state path out of the frame loop;
/// scene replacement and a newly projected local avatar are the only events
/// that can require this binding.
fn avatar_site_handoff_changed(
    q_site: Query<(), (With<SiteAnchor>, Changed<Grid>)>,
    q_avatar: Query<
        (),
        (
            With<Embodiment>,
            With<LocalEmbodiment>,
            Or<(
                Added<LocalEmbodiment>,
                Changed<ChildOf>,
                Added<PendingSiteCameraPose>,
            )>,
        ),
    >,
) -> bool {
    !q_site.is_empty() || !q_avatar.is_empty()
}

/// Mount a loader-created local avatar into the authored site's Grid.
///
/// USD projection initially has no celestial knowledge and therefore places
/// the avatar under the persistent world shell. Once celestial placement has
/// made the authored site root a Grid, this camera subsystem converts the
/// avatar pose through the shared BigSpace coordinate helpers and atomically
/// re-parents it. A camera already mounted in another valid Grid is left to
/// its owning camera mode (for example, orbital view).
fn bind_local_avatar_to_site_grid(
    q_site: Query<(Entity, &GeodeticAnchor), With<SiteAnchor>>,
    q_bodies: Query<(Entity, &CoreCelestialBody)>,
    q_avatar: Query<
        (Entity, &ChildOf, Option<&GravityBody>),
        (With<Embodiment>, With<LocalEmbodiment>),
    >,
    q_pending: Query<&PendingSiteCameraPose>,
    q_grids: Query<&Grid>,
    q_world_grid: Query<(), With<lunco_spatial::WorldGrid>>,
    mut commands: Commands,
) {
    let Ok((site_root, anchor)) = q_site.single() else {
        return;
    };
    let Some((body_entity, _)) = q_bodies
        .iter()
        .find(|(_, body)| body.ephemeris_id == anchor.body)
    else {
        return;
    };
    let Ok(site_grid) = q_grids.get(site_root) else {
        return;
    };

    for (avatar, child_of, gravity_body) in &q_avatar {
        let current_parent = child_of.parent();
        if current_parent == site_root {
            if gravity_body.is_none_or(|binding| binding.body_entity != body_entity) {
                commands
                    .entity(avatar)
                    .try_insert(GravityBody { body_entity });
            }
            commands
                .entity(avatar)
                .try_remove::<PendingSiteCameraPose>();
            continue;
        }

        // A valid non-world Grid is already owned by an explicit camera mode.
        // The startup binder must not reclaim orbital or target-relative views.
        if q_grids.get(current_parent).is_ok() && q_world_grid.get(current_parent).is_err() {
            continue;
        }

        let Ok(pending) = q_pending.get(avatar) else {
            warn!(
                ?avatar,
                ?site_root,
                "local avatar has no pre-mount pose for the authored site Grid"
            );
            continue;
        };
        if pending.site_root != site_root {
            warn!(
                ?avatar,
                ?site_root,
                pending_site = ?pending.site_root,
                "local avatar site-camera handoff targets a different scene"
            );
            continue;
        }
        migrate_to_grid_local_pose(
            &mut commands,
            avatar,
            site_root,
            site_grid,
            pending.position,
            pending.rotation,
        );
        commands
            .entity(avatar)
            .try_insert(GravityBody { body_entity })
            .try_remove::<PendingSiteCameraPose>();
        info!(
            ?avatar,
            ?site_root,
            "local avatar camera mounted in the authored site Grid"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_avatar_mounts_into_ready_site_grid() {
        let mut app = App::new();
        app.insert_resource(lunco_spatial::WorldGridConfig::default());

        let world_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                lunco_spatial::WorldGrid,
            ))
            .id();
        let site = app
            .world_mut()
            .spawn((
                SiteAnchor,
                GeodeticAnchor {
                    body: lunco_celestial::ephemeris_id::MOON,
                    geodetic: lunco_celestial::Geodetic::new(25.28, 307.60, 0.0),
                },
                CellCoord::ZERO,
                Transform::from_xyz(100.0, 2.0, -50.0),
                ChildOf(world_grid),
            ))
            .id();
        let body = app
            .world_mut()
            .spawn(CoreCelestialBody {
                name: "Moon".into(),
                ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                radius_m: lunco_celestial::MOON_MEAN_RADIUS_M,
            })
            .id();
        let avatar = app
            .world_mut()
            .spawn((
                Embodiment,
                LocalEmbodiment,
                CellCoord::ZERO,
                Transform::from_xyz(110.0, 4.0, -40.0),
                ChildOf(world_grid),
            ))
            .id();

        app.add_systems(PreUpdate, capture_site_camera_pose);
        app.add_systems(Update, bind_local_avatar_to_site_grid);
        app.update();

        assert!(app.world().get::<PendingSiteCameraPose>(avatar).is_some());
        app.world_mut()
            .entity_mut(site)
            .insert(lunco_spatial::WorldGridConfig::default().grid());
        app.update();

        assert_eq!(app.world().get::<ChildOf>(avatar).unwrap().parent(), site);
        assert_eq!(
            app.world().get::<GravityBody>(avatar).unwrap().body_entity,
            body
        );
        let avatar_transform = app.world().get::<Transform>(avatar).unwrap();
        assert!(
            avatar_transform
                .translation
                .abs_diff_eq(Vec3::new(10.0, 2.0, 10.0), 1e-5)
        );
    }

    #[test]
    fn late_avatar_projection_is_captured_after_site_grid_creation() {
        let mut app = App::new();
        let world_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                lunco_spatial::WorldGrid,
            ))
            .id();
        let site = app
            .world_mut()
            .spawn((
                SiteAnchor,
                GeodeticAnchor {
                    body: lunco_celestial::ephemeris_id::MOON,
                    geodetic: lunco_celestial::Geodetic::new(25.28, 307.60, 0.0),
                },
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(world_grid),
            ))
            .id();
        let avatar = app
            .world_mut()
            .spawn((
                Embodiment,
                LocalEmbodiment,
                CellCoord::ZERO,
                Transform::from_xyz(45.0, 22.0, 28.0),
                ChildOf(world_grid),
            ))
            .id();

        app.configure_sets(Update, SceneSpatialHandoffSet);
        app.add_systems(
            Update,
            capture_site_camera_pose
                .run_if(site_camera_capture_changed)
                .in_set(SceneSpatialHandoffSet),
        );
        app.update();

        let pending = app
            .world()
            .get::<PendingSiteCameraPose>(avatar)
            .expect("late local avatar must retain its loader-relative pose");
        assert_eq!(pending.site_root, site);
        assert_eq!(pending.position, DVec3::new(45.0, 22.0, 28.0));
        assert_eq!(pending.rotation, DQuat::IDENTITY);
    }

    #[test]
    fn late_avatar_projection_uses_the_mounted_site_frame() {
        let mut app = App::new();
        let world_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                lunco_spatial::WorldGrid,
            ))
            .id();
        let surface_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(world_grid),
            ))
            .id();
        let site = app
            .world_mut()
            .spawn((
                SiteAnchor,
                GeodeticAnchor {
                    body: lunco_celestial::ephemeris_id::MOON,
                    geodetic: lunco_celestial::Geodetic::new(25.28, 307.60, 0.0),
                },
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::from_xyz(100.0, 0.0, 0.0),
                ChildOf(surface_grid),
            ))
            .id();
        let avatar = app
            .world_mut()
            .spawn((
                Embodiment,
                LocalEmbodiment,
                CellCoord::ZERO,
                Transform::from_xyz(110.0, 0.0, 0.0),
                ChildOf(world_grid),
            ))
            .id();

        app.configure_sets(Update, SceneSpatialHandoffSet);
        app.add_systems(
            Update,
            capture_site_camera_pose
                .run_if(site_camera_capture_changed)
                .in_set(SceneSpatialHandoffSet),
        );
        app.update();

        let pending = app
            .world()
            .get::<PendingSiteCameraPose>(avatar)
            .expect("late local avatar must be projected in site coordinates");
        assert_eq!(pending.site_root, site);
        assert_eq!(pending.position, DVec3::new(10.0, 0.0, 0.0));
        assert_eq!(pending.rotation, DQuat::IDENTITY);
    }

    #[test]
    fn deferred_avatar_projection_is_captured_at_the_handoff_boundary() {
        fn project_avatar_once(
            mut commands: Commands,
            q_world_grid: Query<Entity, With<lunco_spatial::WorldGrid>>,
            mut projected: Local<bool>,
        ) {
            if *projected {
                return;
            }
            *projected = true;
            let world_grid = q_world_grid.single().unwrap();
            commands.spawn((
                Embodiment,
                LocalEmbodiment,
                CellCoord::ZERO,
                Transform::from_xyz(45.0, 22.0, 28.0),
                ChildOf(world_grid),
            ));
        }

        let mut app = App::new();
        app.configure_sets(Update, SceneSpatialHandoffSet);
        let world_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                lunco_spatial::WorldGrid,
            ))
            .id();
        app.world_mut().spawn((
            SiteAnchor,
            GeodeticAnchor {
                body: lunco_celestial::ephemeris_id::MOON,
                geodetic: lunco_celestial::Geodetic::new(25.28, 307.60, 0.0),
            },
            lunco_spatial::WorldGridConfig::default().grid(),
            CellCoord::ZERO,
            Transform::default(),
            ChildOf(world_grid),
        ));
        app.add_systems(
            Update,
            (
                project_avatar_once,
                capture_site_camera_pose.run_if(site_camera_capture_changed),
            )
                .chain()
                .in_set(SceneSpatialHandoffSet),
        );

        app.update();

        let position = {
            let world = app.world_mut();
            let mut query = world.query::<&PendingSiteCameraPose>();
            query.single(world).unwrap().position
        };
        assert_eq!(position, DVec3::new(45.0, 22.0, 28.0));
    }
}
