// Hook reflection and dynamic policy binding stay at the Rhai production-test
// boundary. The source is intentionally short: the test exercises the native
// HookValue ABI rather than embedding a USD fixture or a Rust-only assertion.

let checks = [];
let hooks = list_hooks();
let camera = ();
for hook in hooks {
    if hook.id == "camera.default_presentation" { camera = hook; }
}
checks.push(t_true(camera != (), "hook contracts are collected from owner macros"));
checks.push(t_true(camera.declared == true, "camera hook is declared"));
checks.push(t_true(camera.parameters.len() == 1
        && camera.parameters[0].name == "ctx"
        && camera.parameters[0].type == "map",
    "hook parameter signature is reflected"));
checks.push(t_true(camera.output == "string",
    "hook output type is reflected"));

let lifecycle = ();
for hook in hooks {
    if hook.id == "twin.lifecycle" { lifecycle = hook; }
}
checks.push(t_true(lifecycle != () && lifecycle.declared == true
        && lifecycle.parameters.len() == 2
        && lifecycle.parameters[0].type == "string"
        && lifecycle.parameters[1].type == "map",
    "application startup installs the reflected Twin lifecycle seam"));
let lifecycle_call = invoke_hook("twin.lifecycle", [
    "startup",
    #{
        twin_id: "test",
        root: "assets/scenes/tests",
        scope: "test",
        installed: [],
        failed: [],
    },
]);
checks.push(t_true(lifecycle_call.ok == true
        && lifecycle_call.value.event == "startup"
        && lifecycle_call.value.twin_id == "test",
    "the authored lifecycle policy returns its typed event facts"));

let camera_call = invoke_hook("camera.default_presentation", [#{
    local_avatar_camera_count: 1,
    camera_track_count: 0,
    host_requests_generated: false,
}]);
checks.push(t_true(camera_call.ok == true && camera_call.value == "avatar",
    "the authored policy is installed and returns its declared type"));

let wrong_shape = invoke_hook("camera.default_presentation", [41]);
checks.push(t_true(wrong_shape.ok == false && wrong_shape.status == "fault"
        && wrong_shape.error.contains("expects map"),
    "the reflected signature rejects an invalid argument kind"));

let bound = bind_policy(
    "camera.default_presentation",
    "decide",
    "fn decide(ctx) { \"none\" }",
);
checks.push(t_true(bound.ok == true && bound.status == "installed",
    "Rhai can bind a policy dynamically"));

let invoked = invoke_hook("camera.default_presentation", [#{}]);
checks.push(t_true(invoked.ok == true && invoked.installed == true
        && invoked.value == "none",
    "Rhai invokes a bound policy through the typed HookValue ABI"));

let missing = invoke_hook("test.rhai.missing", []);
checks.push(t_true(missing.ok == false && missing.status == "unavailable"
        && missing.installed == false,
    "missing policy implementations are visible to Rhai"));

let rejected = bind_policy(
    "lint.usd",
    "wrong",
    "fn wrong(value) { true }",
);
checks.push(t_true(rejected.ok == false && rejected.status == "rejected",
    "deterministic contracts reject an unqualified inline binding"));

let removed = unbind_policy("camera.default_presentation");
checks.push(t_true(removed.ok == true && removed.status == "removed",
    "Rhai can remove exactly the implementation it installed"));

let status = policy_status();
checks.push(t_true(status.scope != () && status.installed != ()
        && status.failed != () && status.required_failures != (),
    "Rhai can inspect the application policy load report"));
checks.push(t_true(status.failed.len() == 0,
    "the application manifest installs only declared hook policies"));

t_report(checks);
