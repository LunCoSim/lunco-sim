// Read the camera selected by the scheduled Application/Presentation owner.
// The application policy rejects off-cycle calls, so an active camera here
// proves the owner supplied its Presentation context and consumed the result.

let result = query("ReadExposures", #{ surface: "camera-status" });
let active_name = ();
if result != () && result.surfaces != ()
    && result.surfaces["camera-status"] != ()
    && result.surfaces["camera-status"].properties != () {
    active_name = result.surfaces["camera-status"].properties["active_name"];
}

t_report([t_true(active_name != () && active_name != "",
    "the presentation owner selected a camera through its scheduled policy")]);
