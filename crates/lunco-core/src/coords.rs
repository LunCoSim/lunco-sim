use bevy::prelude::*;
use big_space::prelude::*;
use bevy::ecs::query::QueryFilter;

/// Helper: Resolve any entity's absolute position relative to the solar system root.
/// We use Generics for F to allow passing queries with different Filters.
pub fn get_absolute_pos_in_root_double_ghost_aware<F: QueryFilter>(
    entity: Entity,
    initial_cell: &CellCoord,
    initial_tf: &Transform,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(&CellCoord, &Transform), F>,
) -> bevy::math::DVec3 {
    let mut total_pos = bevy::math::DVec3::ZERO;
    
    let mut current_tf = *initial_tf;
    let mut current_cell = *initial_cell;
    let mut current_entity = entity;

    let mut depth = 0;
    while depth < 20 {
        depth += 1;
        if let Ok(child_of) = q_parents.get(current_entity) {
            let parent = child_of.parent();
            if let Ok(grid) = q_grids.get(parent) {
                // Cross a grid boundary: convert current local state to parent coordinate space
                total_pos += grid.grid_position_double(&current_cell, &current_tf);
                
                // Now continue recursion from the grid entity itself
                if let Ok((p_cell, p_tf)) = q_spatial.get(parent) {
                    current_entity = parent;
                    current_cell = *p_cell;
                    current_tf = *p_tf;
                } else {
                    break;
                }
            } else {
                // Intermediate parent (not a grid): accumulate local transform
                if let Ok((_p_cell, p_tf)) = q_spatial.get(parent) {
                    current_tf.translation = p_tf.translation + p_tf.rotation * current_tf.translation;
                    current_tf.rotation = p_tf.rotation * current_tf.rotation;
                    current_cell = *_p_cell;
                    current_entity = parent;
                } else { 
                    total_pos += current_tf.translation.as_dvec3();
                    break;
                }
            }
        } else {
            total_pos += current_tf.translation.as_dvec3();
            break;
        }
    }
    total_pos
}
