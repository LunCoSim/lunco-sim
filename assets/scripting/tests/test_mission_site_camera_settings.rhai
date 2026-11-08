let original_engage = get_setting("CameraInputSettings.surface_mode_engage_altitude_m");
let original_disengage = get_setting("CameraInputSettings.surface_mode_disengage_altitude_m");
let original_duration = get_setting("CameraInputSettings.orbit_direction_animation_duration_s");
let original_surface_scale =
    get_setting("CameraInputSettings.orbit_surface_min_scale");
let original_altitude_exponent =
    get_setting("CameraInputSettings.orbit_look_altitude_curve_exponent");
let original_full_speed_altitude =
    get_setting("CameraInputSettings.orbit_look_full_speed_altitude_m");
let original_return_transition_duration =
    get_setting("CameraInputSettings.orbit_return_transition_duration_s");
let valid_settings = original_engage != () && original_disengage != () && original_duration != ()
    && original_surface_scale != () && original_altitude_exponent != ()
    && original_full_speed_altitude != () && original_return_transition_duration != ()
    && type_of(original_engage) == "f64" && type_of(original_disengage) == "f64"
    && type_of(original_duration) == "f64" && type_of(original_surface_scale) == "f64"
    && type_of(original_altitude_exponent) == "f64"
    && type_of(original_full_speed_altitude) == "f64"
    && type_of(original_return_transition_duration) == "f64";

let update = ();
let changed_engage = ();
let changed_disengage = ();
let changed_duration = ();
let changed_surface_scale = ();
let changed_altitude_exponent = ();
let changed_full_speed_altitude = ();
let changed_return_transition_duration = ();
let rejected_engage_unchanged = false;
if valid_settings {
    update = cmd("SetCameraInput", #{
        surface_mode_engage_altitude_m: 1250.0,
        surface_mode_disengage_altitude_m: 2500.0,
        orbit_direction_animation_duration_s: 0.6,
        orbit_surface_min_scale: 0.0002,
        orbit_look_altitude_curve_exponent: 1.5,
        orbit_look_full_speed_altitude_m: 200000.0,
        orbit_return_transition_duration_s: 1.75
    });
    changed_engage = get_setting("CameraInputSettings.surface_mode_engage_altitude_m");
    changed_disengage = get_setting("CameraInputSettings.surface_mode_disengage_altitude_m");
    changed_duration = mission_site_camera_animation_duration_s();
    changed_surface_scale = get_setting("CameraInputSettings.orbit_surface_min_scale");
    changed_altitude_exponent =
        get_setting("CameraInputSettings.orbit_look_altitude_curve_exponent");
    changed_full_speed_altitude =
        get_setting("CameraInputSettings.orbit_look_full_speed_altitude_m");
    changed_return_transition_duration =
        get_setting("CameraInputSettings.orbit_return_transition_duration_s");

    cmd("SetCameraInput", #{ surface_mode_engage_altitude_m: -1.0 });
    rejected_engage_unchanged = get_setting("CameraInputSettings.surface_mode_engage_altitude_m") == 1250.0;

    cmd("SetCameraInput", #{
        surface_mode_engage_altitude_m: original_engage,
        surface_mode_disengage_altitude_m: original_disengage,
        orbit_direction_animation_duration_s: original_duration,
        orbit_surface_min_scale: original_surface_scale,
        orbit_look_altitude_curve_exponent: original_altitude_exponent,
        orbit_look_full_speed_altitude_m: original_full_speed_altitude,
        orbit_return_transition_duration_s: original_return_transition_duration
    });
}

t_report([
    t_true(valid_settings, "camera settings expose persisted handoff and animation properties"),
    t_true(update != () && update.ok == true, "camera settings accept valid custom values"),
    t_near(changed_engage, 1250.0, 1e-9, "surface-mode engage altitude is configurable"),
    t_near(changed_disengage, 2500.0, 1e-9, "surface-mode release altitude is configurable"),
    t_near(changed_duration, 0.6, 1e-9, "mission-site animation uses its configurable duration"),
    t_near(changed_surface_scale, 0.0002, 1e-12,
        "near-surface orbital response is configurable"),
    t_near(changed_altitude_exponent, 1.5, 1e-12,
        "orbital altitude curve is configurable"),
    t_near(changed_full_speed_altitude, 200000.0, 1e-9,
        "orbital full-speed altitude is configurable"),
    t_near(changed_return_transition_duration, 1.75, 1e-12,
        "orbital return transition duration is configurable"),
    t_true(rejected_engage_unchanged, "invalid engage altitude leaves the current setting unchanged"),
]);
