//! Concrete camera realizations for rendered LunCoSim application surfaces.

use bevy::prelude::App;

/// Install camera adapters needed by a rendered presentation surface.
pub(crate) fn install_camera_realizations(app: &mut App) {
    if !app.is_plugin_added::<lunco_camera_runtime::CameraRuntimePlugin>() {
        app.add_plugins(lunco_camera_runtime::CameraRuntimePlugin);
    }
    if !app.is_plugin_added::<lunco_avatar_camera::AvatarCelestialCameraPlugin>() {
        app.add_plugins(lunco_avatar_camera::AvatarCelestialCameraPlugin);
    }
    app.add_plugins(lunco_camera_celestial::CelestialSurfaceCameraPlugin);
}

/// Install the interactive avatar input projection at the application edge.
pub(crate) fn install_interactive_camera(app: &mut App) {
    install_camera_realizations(app);
    app.add_plugins(lunco_avatar_input::AvatarInputPlugin);
}
