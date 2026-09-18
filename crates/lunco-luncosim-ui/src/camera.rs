//! Concrete camera realizations for rendered LunCoSim application surfaces.

use bevy::prelude::App;

/// Install the camera adapters that need a rendered presentation surface.
pub(crate) fn install_camera_realizations(app: &mut App) {
    app.add_plugins((
        lunco_camera_celestial::CelestialSurfaceCameraPlugin,
        lunco_avatar_camera::AvatarCelestialCameraPlugin,
    ));
}
