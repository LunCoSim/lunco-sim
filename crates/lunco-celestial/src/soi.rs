use bevy::prelude::*;
use bevy::math::DVec3;
use big_space::prelude::*;

#[derive(Component)]
pub struct SOI {
    pub radius_m: f64,
}

pub fn soi_transition_system(
    mut commands: Commands,
    q_entities: Query<(Entity, &GlobalTransform, &ChildOf), (Without<crate::registry::CelestialBody>, Without<Grid>)>,
    q_bodies: Query<(Entity, &GlobalTransform, &SOI, &crate::registry::CelestialBody)>,
    q_all_grids: Query<&Grid>,
) {
    for (entity, gtf, child_of) in q_entities.iter() {
        // Use compute_matrix to get an unambiguous translation Vec3 in Bevy 0.18
        let c_trans = gtf.to_matrix().transform_point3(Vec3::ZERO);
        let current_pos = DVec3::new(c_trans.x as f64, c_trans.y as f64, c_trans.z as f64);
        let current_p_grid = child_of.parent();
        
        let mut best_body = None;
        let mut min_dist = f64::MAX;

        for (body_ent, body_gtf, soi, _body) in q_bodies.iter() {
            let b_trans = body_gtf.to_matrix().transform_point3(Vec3::ZERO);
            let body_pos = DVec3::new(b_trans.x as f64, b_trans.y as f64, b_trans.z as f64);
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
                    let target_gtf = q_bodies.get(new_parent_grid_ent).unwrap().1;
                    let t_trans = target_gtf.to_matrix().transform_point3(Vec3::ZERO);
                    let target_pos = DVec3::new(t_trans.x as f64, t_trans.y as f64, t_trans.z as f64);
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
