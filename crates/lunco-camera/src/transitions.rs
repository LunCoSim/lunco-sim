//! Logic for smooth camera transitions between viewpoints.
//!
//! This module provides the [CameraTransition] component and a corresponding 
//! system to interpolate camera properties using various easing functions, 
//! creating a cinematic experience when switching perspectives.

use bevy::prelude::*;
use bevy::math::curve::{Curve, EaseFunction};
use lunco_core::architecture::{ActiveAction, ActionStatus};
use crate::{ViewPoint, ObserverCamera};

/// Component defining a smooth camera transition between two viewpoints.
///
/// When added to an entity with an [ObserverCamera], this component will
/// drive the interpolation of its [ViewPoint] properties over time.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct CameraTransition {
    /// Initial viewpoint state at the start of the transition.
    pub start: ViewPoint,
    /// Final viewpoint state to reach.
    pub target: ViewPoint,
    /// Total time the transition should take (seconds).
    pub duration_secs: f32,
    /// Time elapsed since the transition started (seconds).
    pub elapsed_secs: f32,
    /// Easing curve to apply for non-linear movement (e.g., EaseInOut).
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
///
/// It updates the [ViewPoint] of entities with a [CameraTransition] component, 
/// handling slerp for rotations and lerp for positions.
pub fn camera_transition_system(
    time: Res<Time>,
    mut q_transitions: Query<(Entity, &mut ViewPoint, &mut CameraTransition, &mut ActiveAction), With<ObserverCamera>>,
    mut commands: Commands,
) {
    for (entity, mut vp, mut trans, mut action) in q_transitions.iter_mut() {
        if action.status != ActionStatus::Running { continue; }

        trans.elapsed_secs += time.delta_secs();
        let raw_t = (trans.elapsed_secs / trans.duration_secs).clamp(0.0, 1.0);
        
        // Use Bevy's built-in easing sampling to make the movement feel more natural.
        let t = trans.easing.sample(raw_t).unwrap_or(raw_t);
        
        // Interpolate properties based on the eased time factor.
        vp.offset = trans.start.offset.lerp(trans.target.offset, t as f64);
        vp.rotation = trans.start.rotation.slerp(trans.target.rotation, t);
        vp.fov = trans.start.fov + (trans.target.fov - trans.start.fov) * t;
        
        action.progress = raw_t; 

        if raw_t >= 1.0 {
            action.status = ActionStatus::Completed;
            commands.entity(entity).remove::<CameraTransition>();
            info!("Camera transition completed for entity {:?}", entity);
        }
    }
}

