use bevy::prelude::*;
use big_space::prelude::*;
use bevy::ecs::query::QueryFilter;

/// Converts J2000 Ecliptic coordinates (AU) to Bevy Meters (approx 1.5e11 scale),
/// applying the obliquity rotation (23.44°) to reach J2000 Equatorial J-up frame.
pub fn ecliptic_to_bevy(pos: bevy::math::DVec3) -> bevy::math::DVec3 {
    let au_to_m = 1.495_978_707e11;
    let pos_m = pos * au_to_m;
    
    // Obliquity of the Ecliptic (J2000)
    let epsilon = (23.439281f64).to_radians();
    let (sin_e, cos_e) = epsilon.sin_cos();
    
    // Rotate around X axis: Ecliptic (x, y, z) -> Equatorial (x', y', z')
    let x = pos_m.x;
    let y = pos_m.y * cos_e - pos_m.z * sin_e;
    let z = pos_m.y * sin_e + pos_m.z * cos_e;
    
    // Map to Bevy Y-up axes: 
    // Bevy X = Eq X
    // Bevy Y = Eq Z (North Pole)
    // Bevy Z = -Eq Y 
    // (This is a standard right-handed mapping where Y is Up)
    bevy::math::DVec3::new(x, z, -y)
}

/// Helper: Resolve any entity's absolute position relative to the solar system root.
/// We use Generics for F to allow passing queries with different Filters (like Without<Camera>).
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
        
        // Loop: Resolve current_grid's position in its parent grid, and so on...
        let mut current_grid_ent = parent_grid_ent;
        let mut depth = 0;
        while let Ok(child_of) = q_parents.get(current_grid_ent) {
            if depth > 5 { break; } depth += 1;
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
