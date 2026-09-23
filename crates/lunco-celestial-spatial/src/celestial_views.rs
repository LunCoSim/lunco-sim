//! Scene-authored trajectory views and ephemeris-driven prim placement.

use bevy::prelude::*;
use big_space::prelude::CellCoord;
use lunco_celestial::EphemerisPosition;
use lunco_celestial_spatial_core::{
    ReferenceFrameIndex, TrajectoryPath, TrajectoryView, TrajectoryViewDecl,
};

/// Stamps a declaring prim after creating its derived trajectory view.
#[derive(Component, Debug, Clone, Copy)]
struct TrajectoryViewSpawned;

#[derive(Component, Debug, Clone, Copy, Default)]
struct EphemerisPositionStatus {
    available: bool,
}

pub(super) struct CelestialViewsPlugin;

impl Plugin for CelestialViewsPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(initialize_ephemeris_position_status)
            .add_systems(Update, spawn_declared_trajectory_views)
            .add_systems(
                Update,
                (
                    ephemeris_position_alignment_system,
                    update_ephemeris_position_system.run_if(
                        crate::cadence::tracked_needs_solve()
                            .or_else(ephemeris_frame_assignment_changed),
                    ),
                    ephemeris_position_visibility_system,
                )
                    .chain()
                    .after(spawn_declared_trajectory_views),
            );
    }
}

fn initialize_ephemeris_position_status(add: On<Add, EphemerisPosition>, mut commands: Commands) {
    commands
        .entity(add.entity)
        .try_insert(EphemerisPositionStatus::default());
}

/// Derived trajectory lines are created from scene-authored sampling parameters.
fn spawn_declared_trajectory_views(
    mut commands: Commands,
    declarations: Query<(Entity, &TrajectoryViewDecl), Without<TrajectoryViewSpawned>>,
) {
    for (declaration, view) in declarations.iter() {
        commands.spawn((
            Name::new(view.name.clone()),
            crate::big_space_setup::CelestialDerived,
            TrajectoryView {
                tracked_id: view.tracked_id,
                reference_id: view.reference_id,
                frame: view.frame,
                color: LinearRgba::from(Color::srgba(
                    view.color[0],
                    view.color[1],
                    view.color[2],
                    view.color[3],
                )),
                is_visible: true,
                user_visible: view.user_visible.unwrap_or(true),
                sampling_days: view.sampling_days,
                sampling_step: view.sampling_step,
                start_epoch: view.start_epoch_jd,
                end_epoch: view.end_epoch_jd,
            },
            TrajectoryPath::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
        ));
        commands
            .entity(declaration)
            .try_insert(TrajectoryViewSpawned);
    }
}

fn ephemeris_frame_assignment_changed(
    positions: Query<
        (),
        (
            With<EphemerisPosition>,
            Or<(Changed<EphemerisPosition>, Changed<ChildOf>)>,
        ),
    >,
) -> bool {
    !positions.is_empty()
}

fn ephemeris_position_alignment_system(
    mut commands: Commands,
    frame_index: Res<ReferenceFrameIndex>,
    positions: Query<(Entity, &EphemerisPosition, &Transform, Option<&ChildOf>)>,
    children: Query<&Children>,
) {
    for (entity, position, transform, current_parent) in &positions {
        let Some(frame) = frame_index.resolve(crate::ReferenceFrame::EclipticJ2000 {
            center: position.reference_id,
        }) else {
            continue;
        };
        if current_parent.is_some_and(|parent| parent.parent() == frame) {
            continue;
        }

        let initial_transform = Transform {
            translation: Vec3::ZERO,
            rotation: transform.rotation,
            scale: transform.scale,
        };
        lunco_spatial::attach::migrate_to_grid(
            &mut commands,
            entity,
            frame,
            CellCoord::default(),
            initial_transform,
        );
        if let Ok(children) = children.get(entity) {
            for child in children.iter() {
                commands
                    .entity(child)
                    .try_insert(big_space::grid::propagation::LowPrecisionRoot);
            }
        }
    }
}

fn update_ephemeris_position_system(
    world: Res<lunco_time::WorldTime>,
    ephemeris: Option<Res<lunco_celestial::ephemeris::EphemerisResource>>,
    grids: Query<&big_space::prelude::Grid>,
    mut positions: Query<(
        &EphemerisPosition,
        &mut Transform,
        Option<&mut CellCoord>,
        Option<&ChildOf>,
        Option<&mut EphemerisPositionStatus>,
    )>,
) {
    let jd = world.epoch_jd;
    for (position, mut transform, cell, parent, status) in &mut positions {
        let Some(ephemeris) = ephemeris.as_deref() else {
            set_position_status(status, false);
            continue;
        };
        let (Some(target), Some(reference)) = (
            ephemeris.provider.global_position(position.target_id, jd),
            ephemeris
                .provider
                .global_position(position.reference_id, jd),
        ) else {
            set_position_status(status, false);
            continue;
        };
        let (Some(mut cell), Some(parent)) = (cell, parent) else {
            set_position_status(status, false);
            continue;
        };
        let Ok(grid) = grids.get(parent.parent()) else {
            set_position_status(status, false);
            continue;
        };

        let relative_position = lunco_celestial::coords::ecliptic_to_bevy(target - reference).raw();
        let (next_cell, next_translation) = grid.translation_to_grid(relative_position);
        if transform.translation != next_translation {
            transform.translation = next_translation;
        }
        if *cell != next_cell {
            *cell = next_cell;
        }
        set_position_status(status, true);
    }
}

fn set_position_status(status: Option<Mut<EphemerisPositionStatus>>, available: bool) {
    let Some(mut status) = status else {
        return;
    };
    if status.as_ref().available != available {
        status.available = available;
    }
}

fn ephemeris_position_visibility_system(
    mut positions: Query<
        (&EphemerisPositionStatus, &mut Visibility),
        Changed<EphemerisPositionStatus>,
    >,
) {
    for (status, mut visibility) in &mut positions {
        let target = if status.available {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != target {
            *visibility = target;
        }
    }
}
