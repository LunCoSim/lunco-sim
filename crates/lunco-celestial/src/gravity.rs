use bevy::prelude::*;
use bevy::math::DVec3;
use big_space::prelude::CellCoord;

pub trait GravityModel: Send + Sync + 'static {
    fn acceleration(&self, relative_pos: DVec3) -> DVec3;
}

#[derive(Component)]
pub struct GravityProvider {
    pub model: Box<dyn GravityModel>,
}

pub struct PointMassGravity {
    pub gm: f64,
}

impl GravityModel for PointMassGravity {
    fn acceleration(&self, relative_pos: DVec3) -> DVec3 {
        let r2 = relative_pos.length_squared();
        if r2 < 1.0 { // Avoid singularity
            return DVec3::ZERO;
        }
        let r = r2.sqrt();
        -relative_pos * (self.gm / (r * r2))
    }
}

pub fn update_global_gravity_system(
    q_bodies: Query<(Entity, &CellCoord, &Transform, &GravityProvider)>,
    q_origin: Query<(Entity, &CellCoord, &Transform), With<big_space::prelude::FloatingOrigin>>,
    q_all_grids: Query<&big_space::prelude::Grid>,
    q_parents: Query<&bevy::prelude::ChildOf>,
    q_spatial: Query<(&CellCoord, &Transform)>,
    mut avian_gravity: ResMut<avian3d::prelude::Gravity>,
) {
    let Some((origin_ent, o_cell, o_tf)) = q_origin.iter().next() else { return; };
    
    let origin_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(origin_ent, o_cell, o_tf, &q_parents, &q_all_grids, &q_spatial);
    let mut nearest_accel = DVec3::ZERO;
    let mut min_dist = f64::MAX;
    
    for (body_ent, b_cell, b_tf, provider) in q_bodies.iter() {
        let body_pos = lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware(body_ent, b_cell, b_tf, &q_parents, &q_all_grids, &q_spatial);
        let rel_to_body = origin_pos - body_pos;
        let dist = rel_to_body.length();
        
        if dist < min_dist {
            min_dist = dist;
            nearest_accel = provider.model.acceleration(rel_to_body);
        }
    }
    
    avian_gravity.0 = nearest_accel;
}
