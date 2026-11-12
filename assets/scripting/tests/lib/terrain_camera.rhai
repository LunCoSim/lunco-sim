// Terrain rows carry raw ECS output/source identities, independently of API ids.
fn main_cover(status, camera, source) {
    if camera == () || status.viewport_camera != camera { return (); }
    let selected = ();
    for row in status.camera_cover {
        if row.camera == camera && (source == () || row.source == source) {
            if selected != () { throw "main viewport terrain cover is ambiguous"; }
            selected = row;
        }
    }
    selected
}

fn above_ground(camera, tolerance) {
    camera != () && camera.collision_radius_m != () && camera.collision_skin_m != ()
        && camera.clearance_m >= max(0.0, camera.collision_radius_m + camera.collision_skin_m - tolerance)
}
