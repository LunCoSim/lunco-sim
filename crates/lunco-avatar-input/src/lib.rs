//! Avatar-specific semantic input projection.
//!
//! This package owns the edge between the shared semantic-control substrate and
//! avatar camera behavior: pointer look, wheel zoom, camera-mode look updates,
//! and the pause/cancel hotkeys. Possession and vessel authority remain in
//! [`lunco-avatar`], while the input vocabulary and bindings remain in
//! [`lunco-control-core`] and [`lunco-input-core`].

use bevy::prelude::*;

mod input;

use input::{
    avatar_behavior_input_system, avatar_global_hotkeys, capture_avatar_intent,
    collect_camera_zoom, scene_keyboard_active,
};

/// Installs the avatar-specific semantic input projection.
pub struct AvatarInputPlugin;

impl Plugin for AvatarInputPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_camera_runtime::CameraRuntimePlugin>() {
            app.add_plugins(lunco_camera_runtime::CameraRuntimePlugin);
        }
        lunco_control_core::ensure_control_plugin(app);
        app.add_systems(Update, collect_camera_zoom);
        app.add_systems(
            Update,
            (capture_avatar_intent, avatar_behavior_input_system).chain(),
        );
        app.add_systems(
            Update,
            (avatar_global_hotkeys, input::avatar_escape_possession).run_if(scene_keyboard_active),
        );
    }
}
