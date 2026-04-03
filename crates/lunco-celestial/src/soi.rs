use bevy::prelude::*;
use big_space::prelude::*;

#[derive(Component)]
pub struct SOI {
    pub radius_m: f64,
}

pub fn soi_transition_system(
    mut commands: Commands,
    q_entities: Query<(Entity, &CellCoord, &Transform, &ChildOf), (Without<crate::registry::CelestialBody>, Without<Grid>)>,
    q_bodies: Query<(Entity, &CellCoord, &Transform, &SOI, &crate::registry::CelestialBody)>,
    q_all_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(&CellCoord, &Transform)>,
) {
    for (entity, cell, tf, child_of) in q_entities.iter() {
        let current_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(entity, cell, tf, &q_parents, &q_all_grids, &q_spatial);
        let current_p_grid = child_of.parent();
        
        let mut best_body = None;
        let mut min_dist = f64::MAX;

        for (body_ent, b_cell, b_tf, soi, _body) in q_bodies.iter() {
            let body_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(body_ent, b_cell, b_tf, &q_parents, &q_all_grids, &q_spatial);
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
                    let (_, b_cell, b_tf, _, _) = q_bodies.get(new_parent_grid_ent).unwrap();
                    let target_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(new_parent_grid_ent, b_cell, b_tf, &q_parents, &q_all_grids, &q_spatial);
                    let (new_cell, new_transform) = new_grid.translation_to_grid(current_pos - target_pos);
                    
                    commands.entity(entity).insert((
                        new_cell,
                        Transform::from_translation(new_transform),
                    ));
                    commands.entity(new_parent_grid_ent).add_child(entity);
                }
            }
        }
    }
}
