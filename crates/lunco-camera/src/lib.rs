use bevy::prelude::*;
use lunco_core::Avatar;

pub struct LunCoCameraPlugin;

impl Plugin for LunCoCameraPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ViewPoint>();
        app.add_systems(Update, (
            viewpoint_blender_system,
        ));
    }
}

/// A target-relative viewport configuration.
/// Used for smooth transitions between different camera perspectives.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct ViewPoint {
    /// The entity to follow/look at
    pub target: Option<Entity>,
    /// High-precision offset in the target's coordinate frame
    pub offset: Vec3,
    /// Desired Field of View
    pub fov: f32,
    /// Blending speed multiplier
    pub speed: f32,
    /// Whether this viewpoint is currently active
    pub active: bool,
}

/// System that smoothly interpolates the camera's transform and projection 
/// toward the desired ViewPoint.
fn viewpoint_blender_system(
    time: Res<Time>,
    mut q_camera: Query<(&mut Transform, &mut Projection, &ViewPoint, &Avatar), With<Camera>>,
    q_targets: Query<&GlobalTransform>,
) {
    let dt = time.delta_secs();
    
    for (mut tf, mut projection, viewpoint, _) in q_camera.iter_mut() {
        if !viewpoint.active { continue; }
        
        if let Some(target_ent) = viewpoint.target {
            if let Ok(target_gtf) = q_targets.get(target_ent) {
                let lerp_factor = (viewpoint.speed * dt).min(1.0);
                
                // Interpolate Transform
                let desired_pos = target_gtf.translation() + target_gtf.back() * viewpoint.offset.z + target_gtf.right() * viewpoint.offset.x + target_gtf.up() * viewpoint.offset.y;
                tf.translation = tf.translation.lerp(desired_pos, lerp_factor);
                
                // Interpolate Rotation toward target
                let target_rot = target_gtf.compute_transform().rotation;
                tf.rotation = tf.rotation.slerp(target_rot, lerp_factor);

                // Interpolate FOV
                if let Projection::Perspective(ref mut p) = *projection {
                    p.fov = p.fov + (viewpoint.fov.to_radians() - p.fov) * lerp_factor;
                }
            }
        }
    }
}
