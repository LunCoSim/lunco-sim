//! Concrete camera realizations for rendered LunCoSim application surfaces.

use bevy::prelude::App;

/// Install camera adapters needed by a rendered presentation surface.
pub(crate) fn install_camera_realizations(app: &mut App) {
    app.add_plugins((
        lunco_camera_runtime::CameraRuntimePlugin,
        lunco_camera_celestial::CelestialSurfaceCameraPlugin,
        lunco_avatar_camera::AvatarCelestialCameraPlugin,
    ));
}

/// Install the interactive avatar input projection at the application edge.
pub(crate) fn install_interactive_camera(app: &mut App) {
    install_camera_realizations(app);
    app.add_plugins(lunco_avatar_input::AvatarInputPlugin);
}
