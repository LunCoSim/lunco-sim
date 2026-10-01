// Run against `lunco://scenes/tests/rocker_bogie_drive.usda` in the windowed app.
// The scene has authored cameras but no avatar. The application supplies
// a transient local avatar without adding a prim to the document.

let result = query("ReadExposures", #{ surface: "camera-status" });
let active_name = ();
if result != () && result.surfaces != ()
    && result.surfaces["camera-status"] != ()
    && result.surfaces["camera-status"].properties != () {
    active_name = result.surfaces["camera-status"].properties["active_name"];
}

let failures = [];
failures.push(t_eq(active_name, "Avatar",
    "a transient avatar is the default presentation in an avatar-free scene"));
t_report(failures);
