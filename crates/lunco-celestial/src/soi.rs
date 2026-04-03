//! # Sphere of Influence (SOI) & Local Grid Management
//!
//! This module implements the "Spatial Handover" logic required for stable 
//! across-the-solar-system travel. 
//!
//! ## The "Why": Numeric Stability & Jitter Prevention
//! Floating-point numbers (`f32` and even `f64`) lose precision as they move 
//! further from the coordinate origin. In a solar system simulation, being 
//! 1 AU (150M km) away from the origin results in "jitter" that makes fine-grained 
//! physics (like rover driving) impossible.
//!
//! ## The Solution: Grid Handovers
//! We use [big_space] to manage localized grids. The SOI system acts as a 
//! "Spatial Router":
//! 1. It monitors the distance between dynamic entities (rovers, ships) and 
//!    celestial bodies.
//! 2. When an entity enters a body's [SOI] radius, it triggers a **Grid Migration**.
//! 3. The entity is re-parented to that body's local `Grid`, effectively 
//!    making the body the new "Origin" for that entity.
//!
//! This ensures that rovers always operate near `(0,0,0)` relative to the 
//! ground they are driving on, maintaining millimeter-level precision.

use bevy::prelude::*;
use big_space::prelude::*;

/// Defines the gravitational and spatial dominance radius of a body.
///
/// Within this radius, entities should be parented to this body's local 
/// coordinate grid to ensure numeric stability.
#[derive(Component)]
pub struct SOI {
    /// Radius in meters. Transitions are triggered when entering or leaving this zone.
    pub radius_m: f64,
}

/// System that manages the migration of entities between planetary coordinate grids.
///
/// It performs absolute spatial lookups across all cells and grids to determine 
/// if an entity has crossed a spatial boundary that requires a re-parenting 
/// operation for stability.
pub fn soi_transition_system(
    mut commands: Commands,
    q_entities: Query<(Entity, &CellCoord, &Transform, &ChildOf), (Without<crate::registry::CelestialBody>, Without<Grid>)>,
    q_bodies: Query<(Entity, &CellCoord, &Transform, &SOI, &crate::registry::CelestialBody)>,
    q_all_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(&CellCoord, &Transform)>,
) {
    for (entity, cell, tf, child_of) in q_entities.iter() {
        // Compute the "Universal" position relative to the simulation root (Solar System Center).
        let current_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(entity, cell, tf, &q_parents, &q_all_grids, &q_spatial);
        let current_p_grid = child_of.parent();
        
        let mut best_body = None;
        let mut min_dist = f64::MAX;

        for (body_ent, b_cell, b_tf, soi, _body) in q_bodies.iter() {
            let body_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(body_ent, b_cell, b_tf, &q_parents, &q_all_grids, &q_spatial);
            let dist = (current_pos - body_pos).length();
            
            // Check if we have entered a new body's dominance zone.
            if dist < soi.radius_m {
                if dist < min_dist {
                    min_dist = dist;
                    best_body = Some(body_ent);
                }
            }
        }

        if let Some(new_parent_grid_ent) = best_body {
            // Only perform a handover if the entity is not already parented to the dominant body.
            if new_parent_grid_ent != current_p_grid {
                if let Ok(new_grid) = q_all_grids.get(new_parent_grid_ent) {
                    // Re-parent in big_space domain:
                    // 1. Calculate the new local offset relative to the target body.
                    // 2. Convert that offset into a Grid-local (Cell, Transform) pair.
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

