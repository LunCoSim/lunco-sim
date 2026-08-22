//! Sphere-of-influence reference-frame transitions.
//!
//! `SoiMigrant` is a semantic moving object. The system chooses a new frame by
//! catalog SOI data, resolves that frame through [`ReferenceFrameIndex`],
//! converts the complete pose in f64, and only then writes the BigSpace
//! `(CellCoord, local Transform)` representation atomically.
//!
//! Crossing an SOI changes the frame *centre*, not surface/orbit mode. An
//! object already in Earth's body-fixed frame remains there while Earth is
//! dominant; an incoming object enters Earth's ecliptic-inertial frame. A
//! landing/contact controller may then explicitly request Earth-fixed without
//! exposing a grid entity to user code.

use bevy::prelude::*;
use big_space::prelude::*;
use lunco_core::attach::migrate_to_grid;
use lunco_core::markers::SoiMigrant;

use crate::{CelestialBodyRegistry, ReferenceFrame, ReferenceFrameIndex};

/// Automatically re-encode moving objects when their dominant celestial
/// centre changes.
#[allow(clippy::type_complexity)]
pub fn soi_transition_system(
    mut commands: Commands,
    registry: Res<CelestialBodyRegistry>,
    frame_index: Res<ReferenceFrameIndex>,
    q_migrants: Query<
        (Entity, &CellCoord, &Transform, &ChildOf),
        (With<SoiMigrant>, Without<Grid>),
    >,
    q_frames: Query<&ReferenceFrame>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<SoiMigrant>>,
) {
    for (entity, cell, transform, child_of) in &q_migrants {
        let current_frame =
            crate::registry::inherited_reference_frame(child_of.parent(), &q_parents, &q_frames);

        // Outside every catalog SOI, use the canonical heliocentric ecliptic
        // frame. This is an explicit semantic target, not a raw-root fallback.
        let mut target_center = crate::ephemeris_id::SUN;
        let mut nearest_distance = f64::INFINITY;

        for body in &registry.bodies {
            let Some(soi_radius) = body.soi_radius_m else {
                continue;
            };
            let candidate_frame = ReferenceFrame::EclipticJ2000 {
                center: body.ephemeris_id,
            };
            let Some(candidate_grid) = frame_index.resolve(candidate_frame) else {
                continue;
            };
            let Some((local_position, _)) = lunco_core::coords::pose_in_grid_seeded(
                entity,
                candidate_grid,
                Some(cell),
                transform,
                &q_parents,
                &q_grids,
                &q_spatial,
            ) else {
                continue;
            };
            let distance = local_position.length();
            if distance < soi_radius && distance < nearest_distance {
                nearest_distance = distance;
                target_center = body.ephemeris_id;
            }
        }

        // A fixed and an inertial frame at the same centre are different
        // orientations but the same SOI. SOI logic must not choose between
        // surface/contact and free-flight modes.
        if current_frame.is_some_and(|frame| frame.center() == Some(target_center)) {
            continue;
        }

        let target_frame = ReferenceFrame::EclipticJ2000 {
            center: target_center,
        };
        let Some(target_grid_entity) = frame_index.resolve(target_frame) else {
            error_once!(
                "[celestial] SOI transition needs {:?}, but that semantic frame is missing or ambiguous",
                target_frame
            );
            continue;
        };
        let Ok(target_grid) = q_grids.get(target_grid_entity) else {
            error_once!(
                "[celestial] resolved SOI frame {:?} is not a BigSpace Grid",
                target_frame
            );
            continue;
        };
        let Some((target_position, target_rotation)) = lunco_core::coords::pose_in_grid_seeded(
            entity,
            target_grid_entity,
            Some(cell),
            transform,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            error_once!(
                "[celestial] cannot transform SOI migrant {:?} into {:?}; hierarchy is disconnected",
                entity,
                target_frame
            );
            continue;
        };

        let (new_cell, local_translation) = target_grid.translation_to_grid(target_position);
        let local_transform = Transform {
            translation: local_translation,
            rotation: target_rotation.as_quat(),
            scale: transform.scale,
        };
        migrate_to_grid(
            &mut commands,
            entity,
            target_grid_entity,
            new_cell,
            local_transform,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::update_reference_frame_index;
    use bevy::math::DVec3;

    const EDGE_M: f32 = 10_000.0;

    fn grid_at(
        world: &mut World,
        parent: Entity,
        frame: ReferenceFrame,
        position: DVec3,
    ) -> Entity {
        let parent_grid = world.get::<Grid>(parent).expect("parent grid");
        let (cell, translation) = parent_grid.translation_to_grid(position);
        world
            .spawn((
                Grid::new(EDGE_M, 1_000.0),
                frame,
                cell,
                Transform::from_translation(translation),
                ChildOf(parent),
            ))
            .id()
    }

    fn app_with_soi_system() -> App {
        let mut app = App::new();
        app.init_resource::<ReferenceFrameIndex>()
            .insert_resource(CelestialBodyRegistry::default_system())
            .add_systems(First, update_reference_frame_index)
            .add_systems(Update, soi_transition_system);
        app
    }

    #[test]
    fn entering_an_soi_migrates_to_its_inertial_frame_without_changing_pose() {
        let mut app = app_with_soi_system();
        let root = app
            .world_mut()
            .spawn((
                Grid::new(EDGE_M, 1_000.0),
                CellCoord::ZERO,
                Transform::IDENTITY,
            ))
            .id();
        let sun = grid_at(
            app.world_mut(),
            root,
            ReferenceFrame::EclipticJ2000 {
                center: crate::ephemeris_id::SUN,
            },
            DVec3::ZERO,
        );
        let earth_offset = DVec3::new(2.0e9, 0.0, 0.0);
        let earth = grid_at(
            app.world_mut(),
            sun,
            ReferenceFrame::EclipticJ2000 {
                center: crate::ephemeris_id::EARTH,
            },
            earth_offset,
        );
        let sun_grid = app.world().get::<Grid>(sun).expect("Sun grid");
        let before_sun = earth_offset + DVec3::new(1_250.0, 25.0, -80.0);
        let (cell, translation) = sun_grid.translation_to_grid(before_sun);
        let migrant = app
            .world_mut()
            .spawn((
                SoiMigrant,
                cell,
                Transform::from_translation(translation),
                ChildOf(sun),
            ))
            .id();

        app.update();

        assert_eq!(app.world().get::<ChildOf>(migrant).unwrap().parent(), earth);
        let target_grid = app.world().get::<Grid>(earth).unwrap();
        let target_cell = app.world().get::<CellCoord>(migrant).unwrap();
        let target_transform = app.world().get::<Transform>(migrant).unwrap();
        let after_earth = target_grid.grid_position_double(target_cell, target_transform);
        assert!((after_earth - (before_sun - earth_offset)).length() < 1.0e-4);
    }

    #[test]
    fn soi_selection_never_switches_fixed_and_inertial_modes_at_same_center() {
        let mut app = app_with_soi_system();
        let root = app
            .world_mut()
            .spawn((
                Grid::new(EDGE_M, 1_000.0),
                CellCoord::ZERO,
                Transform::IDENTITY,
            ))
            .id();
        let sun = grid_at(
            app.world_mut(),
            root,
            ReferenceFrame::EclipticJ2000 {
                center: crate::ephemeris_id::SUN,
            },
            DVec3::ZERO,
        );
        let earth_offset = DVec3::new(2.0e9, 0.0, 0.0);
        let _earth_inertial = grid_at(
            app.world_mut(),
            sun,
            ReferenceFrame::EclipticJ2000 {
                center: crate::ephemeris_id::EARTH,
            },
            earth_offset,
        );
        let earth_fixed = grid_at(
            app.world_mut(),
            sun,
            ReferenceFrame::BodyFixed {
                body: crate::ephemeris_id::EARTH,
            },
            earth_offset,
        );
        // A precision sub-grid inherits Moon-fixed semantics. Repeating the
        // tag here would incorrectly create two semantic owners.
        let surface = app
            .world_mut()
            .spawn((
                Grid::new(EDGE_M, 1_000.0),
                CellCoord::ZERO,
                Transform::IDENTITY,
                ChildOf(earth_fixed),
            ))
            .id();
        let surface_grid = app.world().get::<Grid>(surface).unwrap();
        let (cell, translation) = surface_grid.translation_to_grid(DVec3::new(10_000.0, 0.0, 0.0));
        let migrant = app
            .world_mut()
            .spawn((
                SoiMigrant,
                cell,
                Transform::from_translation(translation),
                ChildOf(surface),
            ))
            .id();

        app.update();

        assert_eq!(
            app.world().get::<ChildOf>(migrant).unwrap().parent(),
            surface,
            "SOI ownership chooses a centre; contact/orbit policy chooses orientation"
        );
    }
}
