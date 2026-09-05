// Dynamic diagnostic tools. The caller supplies an explicit entity api_id;
// there is no first-camera/first-collider fallback and no per-frame polling.
// The command returns an opaque lease id through the API command result.

fn show_camera(target) {
    cmd("AcquireDiagnosticVisual", #{target: target, kind: "camera", policy: "default"});
}

fn show_collider(target) {
    cmd("AcquireDiagnosticVisual", #{target: target, kind: "collider", policy: "default"});
}

fn update_visual(lease, target, kind) {
    cmd("UpdateDiagnosticVisual", #{lease: lease, target: target, kind: kind});
}

fn close_visual(lease) {
    cmd("ReleaseDiagnosticVisual", #{lease: lease});
}
