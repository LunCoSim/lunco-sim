//! # Sphere of Influence (SOI) & Local Grid Management
//!
//! Floating-point precision degrades with distance from origin. To keep
//! rovers operating near (0,0,0), each celestial body owns a local
//! `Grid`, and entities tagged `SoiMigrant` are re-parented to the Grid
//! of the body whose `SOI` they currently occupy.
//!
//! All re-parenting goes through `lunco_core::attach::migrate_to_grid`
//! so the `(ChildOf, CellCoord, Transform)` triple lands atomically and
//! observers / propagation never see a half-migrated entity.

use bevy::prelude::*;
use big_space::prelude::*;
use lunco_core::attach::migrate_to_grid;
use lunco_core::markers::SoiMigrant;

/// Gravitational and spatial dominance radius of a celestial body, in meters.
///
/// `SoiMigrant` entities entering this zone get re-parented to this body's
/// local `Grid`.
#[derive(Component)]
pub struct SOI {
    pub radius_m: f64,
}

/// Re-parents `SoiMigrant` entities to the dominant body's local `Grid`.
///
/// Only entities marked `SoiMigrant` participate — static terrain and
/// scene decoration stay where they were spawned. Migration uses
/// `migrate_to_grid` to write `ChildOf`/`CellCoord`/`Transform` atomically.
pub fn soi_transition_system(
    mut commands: Commands,
    q_migrants: Query<
        (Entity, &CellCoord, &Transform, &ChildOf),
        (With<SoiMigrant>, Without<crate::registry::CelestialBody>, Without<Grid>),
    >,
    q_bodies: Query<(Entity, &CellCoord, &Transform, &SOI, &crate::registry::CelestialBody)>,
    q_all_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
) {
    for (entity, cell, tf, child_of) in q_migrants.iter() {
        let current_pos = lunco_core::coords::world_position_seeded(
            entity, cell, tf, &q_parents, &q_all_grids, &q_spatial,
        );
        let current_grid = child_of.parent();

        let mut best = None;
        let mut min_dist = f64::MAX;

        for (body_ent, b_cell, b_tf, soi, _) in q_bodies.iter() {
            let body_pos = lunco_core::coords::world_position_seeded(
                body_ent, b_cell, b_tf, &q_parents, &q_all_grids, &q_spatial,
            );
            let dist = (current_pos - body_pos).length();
            if dist < soi.radius_m && dist < min_dist {
                min_dist = dist;
                best = Some((body_ent, body_pos));
            }
        }

        let Some((new_grid_ent, new_grid_pos)) = best else { continue };
        if new_grid_ent == current_grid { continue };
        let Ok(new_grid) = q_all_grids.get(new_grid_ent) else { continue };

        let (new_cell, local_translation) = new_grid.translation_to_grid(current_pos - new_grid_pos);
        let local_tf = Transform::from_translation(local_translation).with_rotation(tf.rotation);
        migrate_to_grid(&mut commands, entity, new_grid_ent, new_cell, local_tf);
    }
}
