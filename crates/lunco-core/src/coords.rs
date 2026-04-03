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
    
    // First step: Resolve entity's own position in its immediate parent grid
    if let Ok(child_of) = q_parents.get(entity) {
        let parent_grid_ent = child_of.parent();
        if let Ok(parent_grid) = q_grids.get(parent_grid_ent) {
             total_pos += parent_grid.grid_position_double(initial_cell, initial_tf);
        }
        
        let mut current_grid_ent = parent_grid_ent;
        let mut depth = 0;
        while let Ok(child_of) = q_parents.get(current_grid_ent) {
            if depth > 10 { break; } depth += 1;
            let parent_of_grid = child_of.parent();
            if let Ok(parent_grid) = q_grids.get(parent_of_grid) {
                if let Ok((cell, tf)) = q_spatial.get(current_grid_ent) {
                    total_pos += parent_grid.grid_position_double(cell, tf);
                }
            }
            current_grid_ent = parent_of_grid;
        }
    }
    
    total_pos
}
