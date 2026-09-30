// Run against `lunco://scenes/tests/sun_tracker.usda` in the windowed app.
// With no authored CameraTrack or explicit request, LocalEmbodiment wins.

let result = query("ReadExposures", #{ surface: "camera-status" });
let active_name = ();
if result != () && result.surfaces != ()
    && result.surfaces["camera-status"] != ()
    && result.surfaces["camera-status"].properties != () {
    active_name = result.surfaces["camera-status"].properties["active_name"];
}

let failures = [];
failures.push(t_eq(active_name, "/SunTrackerTest/Avatar",
    "the active-root LocalEmbodiment camera is the default presentation"));
t_report(failures);
