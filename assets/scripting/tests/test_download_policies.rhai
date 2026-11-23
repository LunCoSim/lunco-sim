// Production Rhai coverage for the generic download and consent seams.
// Rust supplies facts and low-level effects; Rhai owns the returned policy and
// presentation decisions.
fn provisioned_view(facts) {
    let result = invoke_hook("assets.provision", [facts]);
    if !result.ok { throw "assets.provision failed: " + result.error; }
    result.value
}

let checks = [];
let hooks = list_hooks();
let provisioning_hook = ();
let startup_selection_hook = ();
let download_prepare_hook = ();
for hook in hooks {
    if hook.id == "assets.provision" { provisioning_hook = hook; }
    if hook.id == "assets.startup.select" { startup_selection_hook = hook; }
    if hook.id == "assets.download.prepare" { download_prepare_hook = hook; }
}

checks.push(t_true(provisioning_hook != ()
        && provisioning_hook.declared == true
        && provisioning_hook.parameters.len() == 1
        && provisioning_hook.parameters[0].type == "map"
        && provisioning_hook.output == "map",
    "consent has a typed authored view-model contract"));
let provisioning = invoke_hook("assets.provision", [#{
    scope: "twin",
    scope_id: "twin:test",
    owner: "test",
    interactive: true,
    datasets: [#{
        id: "twin/test/texture",
        key: "texture",
        name: "Texture",
        required_for_scene: true, recommended: false,
        processed: false,
        artifact_storage_owner: "twin-cache", artifact_relative_path: "sources/texture.png",
        inputs_ready: false,inputs:[#{id:"source",name:"Texture",url:"https://example.invalid/texture.png",storage_owner:"twin-cache",relative_path:"sources/texture.png",state:"missing",error:()}],
        state: #{ kind: "missing", detail: () },
    }],
}]);
checks.push(t_true(provisioning.ok == true
        && provisioning.value.visible == true
        && provisioning.value.rows.len() == 1
        && provisioning.value.rows[0].action == "request"
        && provisioning.value.actions.len() == 1
        && provisioning.value.actions[0].action == "dismiss",
    "Rhai owns consent visibility, rows, and semantic actions"));
checks.push(t_eq(provisioning.value.rows[0].storage,
    "Downloaded asset · Twin .cache / sources/texture.png",
    "download-only details identify the delivered cache owner and relative path"));

let facts = #{
    scope: "twin", scope_id: "twin:test", owner: "test", interactive: true,
    datasets: [],
};
for state in [
    #{kind: "queued", detail: ()},
    #{kind: "downloading", detail: (), bytes_done: 1048576, bytes_total: 4194304},
    #{kind: "processing", detail: "dem"},
    #{kind: "cancelling", detail: ()},
    #{kind: "failed", detail: "checksum mismatch"},
    #{kind: "cancelled", detail: ()},
] {
    facts.datasets = [#{id: "twin/test/data", key: "data", name: "Terrain",
        required_for_scene: true, recommended: false, selected_for_startup: false,
        artifact_storage_owner: "twin", artifact_relative_path: "terrain/site",
        processed: true, inputs_ready:false,inputs:[#{id:"source",name:"Terrain",url:"https://example.invalid/terrain.tif",storage_owner:"twin-cache",relative_path:"sources/terrain.tif",state:"missing",error:()}], state: state}];
    let view = provisioned_view(facts);
    checks.push(t_true(view.visible && view.rows.len() == 1,
        "consent retains " + state.kind + " work without another missing row"));
    checks.push(t_eq(view.rows[0].scope, "twin:test", "row actions retain their exact scope"));
    if state.kind == "downloading" {
        checks.push(t_true(view.rows[0].status.contains("25%")
            && view.rows[0].action == "cancel", "download shows real progress and cancellation"));
        checks.push(t_true(view.phase_color=="var(--caution-color)"
            && view.rows[0].status_color=="var(--caution-color)" && view.activity_display=="flex",
            "active downloads are amber and show activity"));
    }
    if state.kind == "failed" {
        checks.push(t_true(view.rows[0].detail.contains("checksum mismatch")
            && view.rows[0].action_label == "Retry download", "failure retains its cause and retry action"));
        checks.push(t_true(view.phase_color=="var(--danger-color)"
            && view.rows[0].status_color=="var(--danger-color)" && view.activity_display=="none",
            "failed downloads are red and stop showing activity"));
    }
}
facts.datasets[0].inputs_ready=true;
facts.datasets[0].inputs[0].state="ready";
facts.datasets[0].state = #{kind: "missing", detail: ()};
let pending=provisioned_view(facts).rows[0];
checks.push(t_true(pending.status.contains("Downloaded") && pending.action=="process" && pending.action_label=="Prepare" && pending.rebuild_action=="", "stale or missing output offers preparation without a duplicate rebuild action"));
checks.push(t_eq(pending.source_url,"https://example.invalid/terrain.tif","the source link preserves the authored address"));
checks.push(t_true(pending.storage.contains("Terrain · Twin .cache / sources/terrain.tif")
    && pending.storage.contains("Twin folder / terrain/site"),
    "details show the download cache and explicit shared Twin output separately"));
facts.datasets[0].state = #{kind: "installed", detail: ()};
let complete = provisioned_view(facts);
checks.push(t_true(complete.visible && complete.rows.len() == 1
    && complete.title == "Data ready" && complete.actions[0].action == "dismiss",
    "completed downloads retain an explicit completion view"));
checks.push(t_true(complete.rows[0].stages_display=="none" && complete.rows[0].min_height=="80px","ready rows use compact summaries"));
checks.push(t_eq(complete.rows[0].rebuild_action,"rebuild","only an installed processed artifact offers forced rebuild"));
checks.push(t_true(complete.phase_color=="var(--ok-color)"
    && complete.rows[0].status_color=="var(--ok-color)" && complete.activity_display=="none",
    "completed preparation is green and stops showing activity"));
facts.simulation=#{pending:[],world_hold:false,blockers:[],playing:false,fault:(),
    terrain_maps_pending:false,terrain_views_pending:false};
let paused=provisioned_view(facts);
checks.push(t_true(paused.simulation_status=="Simulation paused" && paused.actions[0].action=="dismiss","completion preserves intentional pause and never starts transport"));
facts.simulation.terrain_maps_pending=true;
facts.simulation.terrain_views_pending=true;
let maps=provisioned_view(facts);
checks.push(t_true(maps.title=="Building terrain maps" && maps.actions[0].label=="Continue in background",
    "installed data retains a terrain map stage after physics is ready"));
checks.push(t_true(maps.description.contains("Terrain will appear once")
    && maps.scope_label.contains("1 / 1 datasets installed") && maps.rows[0].status=="Installed",
    "the map stage says which work is still outstanding and labels disk installation explicitly"));
checks.push(t_true(maps.phase_color=="var(--caution-color)"
    && maps.rows[0].status_color=="var(--ok-color)" && maps.activity_display=="flex",
    "the scene stays amber and active while installed data rows turn green"));
facts.simulation.terrain_maps_pending=false;
let views=provisioned_view(facts);
checks.push(t_true(views.title=="Preparing terrain views" && views.description.contains("Terrain is still loading")
    && views.simulation_status=="Simulation paused",
    "terrain rendering progress leaves the transport pause visible"));
facts.simulation.terrain_views_pending=false;
checks.push(t_eq(provisioned_view(facts).title,"Data ready",
    "the completion title returns when terrain presentation settles"));
facts.datasets[0].state=#{kind:"missing",detail:()};
checks.push(t_true(provisioned_view(facts).simulation_detail.contains("Source files are downloaded"),"downloaded files wait for preparation without claiming work is running"));
facts.datasets[0].inputs_ready=false;
let missing=provisioned_view(facts);
checks.push(t_true(missing.phase_color=="var(--danger-color)"
    && missing.rows[0].status_color=="var(--danger-color)" && missing.activity_display=="none",
    "missing data is red and waits without showing activity"));
checks.push(t_true(provisioned_view(facts).simulation_detail.contains("Download the required files"),"missing files explain the required action"));
facts.datasets[0].state=#{kind:"queued",detail:()};
checks.push(t_true(provisioned_view(facts).simulation_detail.contains("continue in the background"),"active work leaves the application available"));
facts.datasets[0].state=#{kind:"failed",detail:"checksum mismatch"};
checks.push(t_true(provisioned_view(facts).simulation_detail.contains("then retry"),"failed required data retains an explicit recovery step"));
facts.datasets[0].inputs_ready=true;
facts.datasets[0].state=#{kind:"installed",detail:()};
facts.simulation.pending=["collision support"];
facts.simulation.world_hold=true;
let held=provisioned_view(facts);
checks.push(t_true(held.title=="Preparing simulation" && held.description.contains("still in progress")
    && held.simulation_status.contains("held"),"the window names ongoing simulation preparation after installation"));
facts.simulation.fault=#{subject:"Physics",detail:"Initialization failed"};
checks.push(t_true(provisioned_view(facts).title=="Simulation needs attention","a simulation failure is not reported ready merely because data is ready"));
facts.simulation=();
checks.push(t_true(provisioned_view(facts).visible,
    "nonrecommended data remains mandatory when referenced by the scene"));
facts.datasets[0].required_for_scene=false;
facts.datasets[0].recommended=true;
checks.push(t_true(!provisioned_view(facts).visible && provisioned_view(facts).rows.len()==0,
    "recommendations for other scenes do not open consent"));
facts.include_optional=true;
let optional=provisioned_view(facts);
checks.push(t_true(optional.rows.len()==1 && optional.rows[0].label.starts_with("Recommended"), "optional Twin data remains available in Settings"));
facts.datasets[0].name="DejaVu Sans Regular";
facts.datasets[0].recommended=false;
let optional_font=provisioned_view(facts);
checks.push(t_true(optional_font.rows.len()==1
    && optional_font.rows[0].label.starts_with("Optional · DejaVu Sans Regular"),
    "the fallback font uses the ordinary explicit-download label"));
facts.include_optional=false;
facts.datasets[0].required_for_scene=true;
facts.datasets[0].state = #{kind: "downloading", detail: (), bytes_done: 1048576, bytes_total: 0};
let unknown_total = provisioned_view(facts).rows[0].status;
checks.push(t_true(unknown_total.contains("MiB received") && !unknown_total.contains("%"),
    "unknown download length does not invent a percentage"));
facts.datasets[0].state = #{kind: "invalid", detail: ()};
checks.push(t_true(invoke_hook("assets.provision", [facts]).ok == false,
    "an invalid dataset phase is rejected visibly"));

facts.datasets[0].state=#{kind:"missing",detail:()};
facts.startup_working=true;
checks.push(t_true(provisioned_view(facts).description.contains("Built-in downloads continue in the background."),
    "scene consent explains continuing startup work"));
facts.startup_working=false;

if startup_selection_hook != () {
    let selected = invoke_hook("assets.startup.select", [[
        #{id:"engine/celestial/earth", scope:"engine", group:"celestial", key:"earth", state:"installed"},
        #{id:"twin/mission/moon", scope:"twin", group:"celestial", key:"moon", state:"missing"},
        #{id:"engine/terrain/moon", scope:"engine", group:"terrain", key:"moon", state:"missing"},
        #{id:"engine/celestial/moon", scope:"engine", group:"celestial", key:"moon", state:"missing"},
        #{id:"engine/celestial/mars", scope:"engine", group:"celestial", key:"mars", state:"missing"},
    ]]);
    checks.push(t_true(selected.ok && selected.value == ["engine/celestial/earth", "engine/celestial/moon"],
        "startup selects the authored engine datasets and excludes matching Twin keys and other engine datasets"));
}

facts.startup=true;
facts.scope="engine";
facts.scope_id="engine";
facts.datasets[0].required_for_scene=false;
facts.datasets[0].state=#{kind:"processing",detail:"texture"};
let startup=provisioned_view(facts);
checks.push(t_true(startup.visible && startup.rows[0].status=="Preparing for the simulator"
    && startup.actions[0].scope=="startup-assets" && startup.simulation_status=="",
    "startup progress is independent of scene consent"));
facts.scene_notice_visible=true;
let background_startup=provisioned_view(facts);
checks.push(t_true(!background_startup.visible && background_startup.rows[0].status==startup.rows[0].status,
    "the scene-data notice takes the foreground while startup retains its progress"));
facts.scene_notice_visible=false;
facts.datasets[0].state=#{kind:"failed",detail:"processing failed: test failure"};
let failed_startup=provisioned_view(facts);
checks.push(t_true(failed_startup.rows[0].action_label=="Retry preparation"
    && failed_startup.description.contains("Data & libraries") && failed_startup.actions[0].label=="Close",
    "failed startup assets keep an actionable recovery path"));
facts.datasets[0].state=#{kind:"installed",detail:()};
let ready_startup=provisioned_view(facts);
checks.push(t_true(ready_startup.title=="Built-in assets ready" && ready_startup.actions[0].label=="Done",
    "startup reports completed cache installation"));

checks.push(t_true(download_prepare_hook != ()
        && download_prepare_hook.declared == true
        && download_prepare_hook.parameters.len() == 1
        && download_prepare_hook.parameters[0].type == "map"
        && download_prepare_hook.output == "map",
    "download preparation has a typed generic policy contract"));
let download_policy = invoke_hook("assets.download.prepare", [#{
    key: "texture",
    name: "Texture",
    url: "https://example.invalid/texture.zip",
    destination: "textures/texture.zip",
    twin_scoped: true,
    shared_cache: false,
    destination_exists: false,
    version: (),
    expected_sha256: (),
    has_process: false,
}]);
checks.push(t_true(download_policy.ok == true
        && download_policy.value.allow_network == true
        && download_policy.value.cache == "use_existing"
        && download_policy.value.install_mode == "keep_existing"
        && download_policy.value.backup == "discard_after_commit"
        && download_policy.value.retain_max_count == 0,
    "Rhai owns the default download mode"));

t_report(checks);
