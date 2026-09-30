let original_engage = get_setting("CameraInputSettings.surface_mode_engage_altitude_m");
let original_disengage = get_setting("CameraInputSettings.surface_mode_disengage_altitude_m");
let original_duration = get_setting("CameraInputSettings.orbit_direction_animation_duration_s");
let valid_settings = original_engage != () && original_disengage != () && original_duration != ()
    && type_of(original_engage) == "f64" && type_of(original_disengage) == "f64"
    && type_of(original_duration) == "f64";

let update = ();
let changed_engage = ();
let changed_disengage = ();
let changed_duration = ();
let rejected_engage_unchanged = false;
if valid_settings {
    update = cmd("SetCameraInput", #{
        surface_mode_engage_altitude_m: 1250.0,
        surface_mode_disengage_altitude_m: 2500.0,
        orbit_direction_animation_duration_s: 0.6
    });
    changed_engage = get_setting("CameraInputSettings.surface_mode_engage_altitude_m");
    changed_disengage = get_setting("CameraInputSettings.surface_mode_disengage_altitude_m");
    changed_duration = mission_site_camera_animation_duration_s();

    cmd("SetCameraInput", #{ surface_mode_engage_altitude_m: -1.0 });
    rejected_engage_unchanged = get_setting("CameraInputSettings.surface_mode_engage_altitude_m") == 1250.0;

    cmd("SetCameraInput", #{
        surface_mode_engage_altitude_m: original_engage,
        surface_mode_disengage_altitude_m: original_disengage,
        orbit_direction_animation_duration_s: original_duration
    });
}

t_report([
    t_true(valid_settings, "camera settings expose persisted handoff and animation properties"),
    t_true(update != () && update.ok == true, "camera settings accept valid custom values"),
    t_near(changed_engage, 1250.0, 1e-9, "surface-mode engage altitude is configurable"),
    t_near(changed_disengage, 2500.0, 1e-9, "surface-mode release altitude is configurable"),
    t_near(changed_duration, 0.6, 1e-9, "mission-site animation uses its configurable duration"),
    t_true(rejected_engage_unchanged, "invalid engage altitude leaves the current setting unchanged"),
]);
