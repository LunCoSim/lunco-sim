// Hook reflection, runtime-context forwarding, and feature-gated policy loading
// are observable through the production Rhai bridge, so their acceptance checks
// stay here rather than in a Rust-only assertion.

let checks = [];
let hooks = list_hooks();

fn contains_string(entries, value) {
    for entry in entries {
        if entry == value { return true; }
    }
    false
}

fn contains_hook(entries, id) {
    for entry in entries {
        if entry.id == id { return true; }
    }
    false
}

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

let escape_policy_hook = ();
for hook in hooks {
    if hook.id == "physics.body_escape" { escape_policy_hook = hook; }
}
checks.push(t_true(escape_policy_hook != ()
        && escape_policy_hook.declared == true
        && escape_policy_hook.deterministic == true
        && escape_policy_hook.required == true
        && escape_policy_hook.installable == true
        && escape_policy_hook.parameters.len() == 1
        && escape_policy_hook.parameters[0].name == "ctx"
        && escape_policy_hook.parameters[0].type == "map"
        && escape_policy_hook.output == "string",
    "physics escape exposes a required deterministic Rhai action policy"));
let physics_initialization_hook = ();
for hook in hooks {
    if hook.id == "physics.initialization" { physics_initialization_hook = hook; }
}
checks.push(t_true(physics_initialization_hook != ()
        && physics_initialization_hook.declared == true
        && physics_initialization_hook.deterministic == true
        && physics_initialization_hook.required == false
        && physics_initialization_hook.installable == true
        && physics_initialization_hook.parameters.len() == 1
        && physics_initialization_hook.parameters[0].name == "facts"
        && physics_initialization_hook.parameters[0].type == "map"
        && physics_initialization_hook.output == "string",
    "physics initialization exposes one declared deterministic owner policy seam"));
let default_escape_policy = invoke_hook("physics.body_escape", [#{
    kind: "finite_world_exit",
    path: "/EscapeContainmentTest/Escapee",
    global_id: (),
    position_m: [1510.0, 0.0, 0.0],
    velocity_mps: [0.0, -0.16, 0.0],
    world_min_m: [-200.0, -200.0, -200.0],
    world_max_m: [200.0, 200.0, 200.0],
}]);
checks.push(t_true(default_escape_policy.ok == false
        && default_escape_policy.status == "fault"
        && default_escape_policy.error.contains("classified fixed simulation context"),
    "the physics escape policy rejects invocation outside its fixed simulation cycle"));
let merge = ();
for hook in hooks {
    if hook.id == "journal.merge.order" { merge = hook; }
}
checks.push(t_true(merge != () && merge.declared == true
        && merge.deterministic == true
        && merge.required == false
        && merge.installable == true
        && merge.parameters.len() == 2
        && merge.parameters[0].type == "map"
        && merge.parameters[1].type == "map"
        && merge.output == "int",
    "journal merge policy exposes its deterministic typed contract"));

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

let sysml_lifecycle_call = invoke_hook("twin.lifecycle", [
    "assets_mounted",
    #{
        twin_id: 42,
        name: "analysis-fixture",
        root: "twins/analysis-fixture",
        active: true,
        manifest: #{sysml: #{root: "models/root.sysml", paths: ["models"]}},
        files: ["models/root.sysml", "models/base.kerml", "scenes/main.usda"],
    },
]);
let load_count = 0;
let prepare_found = false;
for action in sysml_lifecycle_call.value.actions {
    if action.command == "LoadTwinSysmlSource" { load_count += 1; }
    if action.command == "PrepareTwinSysmlAnalysis" {
        prepare_found = action.params.twin_id == 42
            && action.params.name == "analysis-fixture"
            && action.params.relative_paths.len() == 2
            && action.params.relative_paths[0] == "models/root.sysml"
            && action.params.relative_paths[1] == "models/base.kerml";
    }
}
checks.push(t_true(sysml_lifecycle_call.ok == true && load_count == 2
        && prepare_found,
    "Twin policy loads the complete authored SysML set then requests one async analysis snapshot"));

let asset_lifecycle = ();
for hook in hooks {
    if hook.id == "application.asset.lifecycle" { asset_lifecycle = hook; }
}
checks.push(t_true(asset_lifecycle != ()
        && asset_lifecycle.declared == true
        && asset_lifecycle.parameters.len() == 2
        && asset_lifecycle.parameters[0].type == "string"
        && asset_lifecycle.parameters[1].type == "map"
        && asset_lifecycle.output == "map",
    "application asset selection has a reflected typed lifecycle contract"));

let scene_time_hook = ();
for hook in hooks {
    if hook.id == "scene.time.select" { scene_time_hook = hook; }
}
checks.push(t_true(scene_time_hook != ()
        && scene_time_hook.declared == true
        && scene_time_hook.deterministic == true
        && scene_time_hook.required == true
        && scene_time_hook.installable == true
        && scene_time_hook.parameters.len() == 1
        && scene_time_hook.parameters[0].name == "facts"
        && scene_time_hook.parameters[0].type == "map"
        && scene_time_hook.output == "map",
    "scene time has one required typed application policy"));
let current_time_epoch = 2461400.125;
let current_time_selection = invoke_hook("scene.time.select", [#{
    epoch_api: false,
    epoch_status: "not_applied",
    authored_epoch_jd: (),
    has_celestial_source: true,
    computer_time_tdb_jd: current_time_epoch,
}]);
checks.push(t_true(current_time_selection.ok == true
        && current_time_selection.value.source == "computer_time"
        && current_time_selection.value.epoch_jd == current_time_epoch
        && current_time_selection.value.warning.contains("computer UTC"),
    "scene time policy selects the supplied current computer time for an untimed celestial scene"));
let authored_time_epoch = 2461132.5824799724;
let authored_time_selection = invoke_hook("scene.time.select", [#{
    epoch_api: true,
    epoch_status: "valid",
    authored_epoch_jd: authored_time_epoch,
    has_celestial_source: true,
    computer_time_tdb_jd: current_time_epoch,
}]);
checks.push(t_true(authored_time_selection.ok == true
        && authored_time_selection.value.source == "authored"
        && authored_time_selection.value.epoch_jd == authored_time_epoch
        && authored_time_selection.value.warning == (),
    "scene time policy selects an authored root epoch when it is valid"));
let camera_call = invoke_hook("camera.default_presentation", [#{
    local_avatar_camera_count: 1,
    camera_track_count: 0,
    host_requests_generated: false,
}]);
checks.push(t_true(camera_call.ok == true && camera_call.value == "avatar",
    "the authored policy is installed and returns its declared type"));

let quality_profile_hook = ();
let default_quality_hook = ();
for hook in hooks {
    if hook.id == "render.quality_profile" { quality_profile_hook = hook; }
    if hook.id == "render.default_quality_profile" { default_quality_hook = hook; }
}
checks.push(t_true(quality_profile_hook != ()
        && quality_profile_hook.declared == true
        && quality_profile_hook.deterministic == true
        && quality_profile_hook.required == true
        && quality_profile_hook.parameters.len() == 1
        && quality_profile_hook.parameters[0].name == "id"
        && quality_profile_hook.parameters[0].type == "string"
        && quality_profile_hook.output == "map",
    "render presets expose a required deterministic typed profile policy"));

checks.push(t_true(default_quality_hook != ()
        && default_quality_hook.declared == true
        && default_quality_hook.deterministic == true
        && default_quality_hook.required == true
        && default_quality_hook.parameters.len() == 0
        && default_quality_hook.output == "string",
    "fresh settings use a required deterministic authored default-profile policy"));
let default_quality = invoke_hook("render.default_quality_profile", []);
checks.push(t_true(default_quality.ok == true && default_quality.value == "high",
    "the application Rhai policy selects High for fresh settings"));

let low_profile = invoke_hook("render.quality_profile", ["low"]);
let balanced_profile = invoke_hook("render.quality_profile", ["balanced"]);
let high_profile = invoke_hook("render.quality_profile", ["high"]);
checks.push(t_true(low_profile.ok == true
        && low_profile.value.len() == 69
        && low_profile.value.directional_shadow_map_size == 512
        && low_profile.value.directional_cascades == 1,
    "Low profile values and complete field coverage come from Rhai"));
checks.push(t_true(balanced_profile.ok == true
        && balanced_profile.value.len() == 69
        && balanced_profile.value.directional_shadow_map_size == 2048
        && balanced_profile.value.directional_cascades == 3
        && balanced_profile.value.shadow_maximum_distance == 1500.0,
    "Balanced shadow quality uses the authored map, cascade, and range values"));
checks.push(t_true(high_profile.ok == true
        && high_profile.value.len() == 69
        && high_profile.value.directional_shadow_map_size == 4096
        && high_profile.value.directional_cascades == 4
        && high_profile.value.shadow_first_cascade_far_bound == 30.0
        && high_profile.value.shadow_maximum_distance == 3000.0
        && high_profile.value.terrain_lod_max_depth == 9
        && high_profile.value.terrain_lod_tile_budget == 256
        && high_profile.value.horizon_march_steps == 96,
    "High shadow quality uses the authored high-detail values"));
let missing_profile = invoke_hook("render.quality_profile", ["turbo"]);
checks.push(t_true(missing_profile.ok == false && missing_profile.status == "fault",
    "unknown profile ids fail visibly instead of selecting another profile"));

let wrong_shape = invoke_hook("camera.default_presentation", [41]);
checks.push(t_true(wrong_shape.ok == false && wrong_shape.status == "fault"
        && wrong_shape.error.contains("expects map"),
    "the reflected signature rejects an invalid argument kind"));

let bound = bind_policy(
    "camera.default_presentation",
    "decide",
    "fn decide(ctx) { runtime_context.scope + \"/\" + runtime_context.cycle + \"/\" + runtime_context.phase + \"/\" + runtime_context.clock }",
);
checks.push(t_true(bound.ok == true && bound.status == "installed",
    "Rhai can bind a policy dynamically"));

let invoked = invoke_hook("camera.default_presentation", [#{}]);
checks.push(t_true(invoked.ok == true && invoked.installed == true
        && invoked.value == "application/repl/evaluation/application",
    "nested hook invocation exposes the caller's typed REPL context to Rhai"));

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
        && status.failed != () && status.unavailable != ()
        && status.required_failures != (),
    "Rhai can inspect the application policy load report"));
checks.push(t_true(status.failed.len() == 0,
    "feature-omitted owners do not become policy installation failures"));
for hook_id in [
    "assets.download.prepare",
    "render.shadow_quality",
    "runtime.ui.recording",
] {
    let hook_declared = contains_hook(hooks, hook_id);
    let reported_unavailable = contains_string(status.unavailable, hook_id);
    checks.push(t_true(hook_declared != reported_unavailable,
        "optional policy owner availability is explicit for " + hook_id));
}

t_report(checks);
