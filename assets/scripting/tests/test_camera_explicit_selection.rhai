// Run after SetUserCamera("VisualCamera") on `rocker_bogie_drive.usda`.
// An explicit request retains ownership over the no-track fallback.

let result = query("ReadExposures", #{ surface: "camera-status" });
let active_name = ();
if result != () && result.surfaces != ()
    && result.surfaces["camera-status"] != ()
    && result.surfaces["camera-status"].properties != () {
    active_name = result.surfaces["camera-status"].properties["active_name"];
}

let failures = [];
failures.push(t_eq(active_name, "/RockerBogieDrive/VisualCamera",
    "an explicit camera request retains viewport ownership"));
t_report(failures);
