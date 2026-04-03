use bevy::prelude::*;
use lunco_core::architecture::{ActiveAction, ActionStatus};
use crate::{ViewPoint, ObserverCamera};

/// Component defining a smooth camera transition between two viewpoints.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct CameraTransition {
    pub start: ViewPoint,
    pub target: ViewPoint,
    pub duration_secs: f32,
    pub elapsed_secs: f32,
}

/// System that processes active camera transitions, interpolating the camera's ViewPoint.
pub fn camera_transition_system(
    time: Res<Time>,
    mut q_transitions: Query<(Entity, &mut ViewPoint, &mut CameraTransition, &mut ActiveAction), With<ObserverCamera>>,
    mut commands: Commands,
) {
    for (entity, mut vp, mut trans, mut action) in q_transitions.iter_mut() {
        if action.status != ActionStatus::Running { continue; }

        trans.elapsed_secs += time.delta_secs();
        let t = (trans.elapsed_secs / trans.duration_secs).clamp(0.0, 1.0);
        
        // Linear Interpolation
        vp.offset = trans.start.offset.lerp(trans.target.offset, t);
        vp.yaw = trans.start.yaw + (trans.target.yaw - trans.start.yaw) * t;
        vp.pitch = trans.start.pitch + (trans.target.pitch - trans.start.pitch) * t;
        vp.fov = trans.start.fov + (trans.target.fov - trans.start.fov) * t;
        
        action.progress = t;

        if t >= 1.0 {
            action.status = ActionStatus::Completed;
            commands.entity(entity).remove::<CameraTransition>();
            info!("Camera transition completed for entity {:?}", entity);
        }
    }
}
