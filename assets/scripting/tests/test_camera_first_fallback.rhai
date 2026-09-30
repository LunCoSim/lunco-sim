// Run against `lunco://scenes/tests/rocker_bogie_drive.usda` in the windowed app.
// With no authored CameraTrack, explicit request, or LocalEmbodiment camera,
// the first active-root camera in USD-path order wins.

let result = query("ReadExposures", #{ surface: "camera-status" });
let active_name = ();
if result != () && result.surfaces != ()
    && result.surfaces["camera-status"] != ()
    && result.surfaces["camera-status"].properties != () {
    active_name = result.surfaces["camera-status"].properties["active_name"];
}

let failures = [];
failures.push(t_eq(active_name, "/RockerBogieDrive/Rover/FrontCamera",
    "the first active-root authored camera is the fallback presentation"));
t_report(failures);
