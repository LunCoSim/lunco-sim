use bevy::prelude::*;
use big_space::prelude::*;
use crate::registry::CelestialBody;

#[derive(Component)]
pub struct SOI {
    pub radius_m: f64,
}

pub fn soi_transition_system(
    mut commands: Commands,
    q_entities: Query<(Entity, &GlobalTransform, &ChildOf), (Without<CelestialBody>, Without<Grid>)>,
    q_bodies: Query<(Entity, &GlobalTransform, &SOI, &CelestialBody)>,
    q_all_grids: Query<&Grid>,
) {
    for (entity, gtf, child_of) in q_entities.iter() {
        let current_pos = gtf.translation().as_dvec3();
        let current_p_grid = child_of.parent();
        
        let mut best_body = None;
        let mut min_dist = f64::MAX;

        for (body_ent, body_gtf, soi, _body) in q_bodies.iter() {
            let body_pos = body_gtf.translation().as_dvec3();
            let dist = (current_pos - body_pos).length();
            
            if dist < soi.radius_m {
                if dist < min_dist {
                    min_dist = dist;
                    best_body = Some(body_ent);
                }
            }
        }

        if let Some(new_parent_grid_ent) = best_body {
            if new_parent_grid_ent != current_p_grid {
                if let Ok(new_grid) = q_all_grids.get(new_parent_grid_ent) {
                    // Re-parent in big_space
                    let (new_cell, new_transform) = new_grid.translation_to_grid(current_pos - q_bodies.get(new_parent_grid_ent).unwrap().1.translation().as_dvec3());
                    // Wait, GlobalTransform in big_space is relative to origin.
                    // If we re-parent, we want the same global position.
                    // big_space 0.12 has some helpers but let's do it manually for now if needed.
                    
                    // Actually, commands.entity(entity).set_parent_in_place(new_parent_grid_ent) might NOT work correctly with big_space coordinates.
                    // We need to update CellCoord and Transform.
                    
                    commands.entity(entity).insert((
                        new_cell,
                        Transform::from_translation(new_transform),
                    )).set_parent_in_place(new_parent_grid_ent);
                }
            }
        }
    }
}
