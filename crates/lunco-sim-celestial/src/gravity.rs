use bevy::prelude::*;
use bevy::math::DVec3;
use bevy_hierarchy::DespawnRecursiveExt;

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
    q_bodies: Query<(&GlobalTransform, &GravityProvider)>,
    q_origin: Query<&GlobalTransform, With<big_space::prelude::FloatingOrigin>>,
    mut avian_gravity: ResMut<avian3d::prelude::Gravity>,
) {
    let Some(origin_gtf) = q_origin.iter().next() else { return; };
    
    let origin_pos = origin_gtf.translation().as_dvec3();
    let mut nearest_accel = DVec3::ZERO;
    let mut min_dist = f64::MAX;
    
    for (body_gtf, provider) in q_bodies.iter() {
        let body_pos = body_gtf.translation().as_dvec3();
        let rel_to_body = origin_pos - body_pos;
        let dist = rel_to_body.length();
        
        if dist < min_dist {
            min_dist = dist;
            nearest_accel = provider.model.acceleration(rel_to_body);
        }
    }
    
    avian_gravity.0 = nearest_accel;
}
