use bevy::prelude::*;
use bevy::math::curve::{Curve, EaseFunction};
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
    pub easing: EaseFunction,
}

impl Default for CameraTransition {
    fn default() -> Self {
        Self {
            start: ViewPoint::default(),
            target: ViewPoint::default(),
            duration_secs: 1.0,
            elapsed_secs: 0.0,
            easing: EaseFunction::QuadraticInOut,
        }
    }
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
        let raw_t = (trans.elapsed_secs / trans.duration_secs).clamp(0.0, 1.0);
        
        // Use Bevy's built-in easing sampling
        let t = trans.easing.sample(raw_t).unwrap_or(raw_t);
        
        // Interpolation with Easing
        vp.offset = trans.start.offset.lerp(trans.target.offset, t);
        vp.yaw = trans.start.yaw + (trans.target.yaw - trans.start.yaw) * t;
        vp.pitch = trans.start.pitch + (trans.target.pitch - trans.start.pitch) * t;
        vp.fov = trans.start.fov + (trans.target.fov - trans.start.fov) * t;
        
        action.progress = raw_t; 

        if raw_t >= 1.0 {
            action.status = ActionStatus::Completed;
            commands.entity(entity).remove::<CameraTransition>();
            info!("Camera transition completed for entity {:?}", entity);
        }
    }
}
