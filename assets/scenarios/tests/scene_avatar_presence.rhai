// Windowed production acceptance, attached through RunScenario to an owned
// scene. Parameters: doc_id and authored_avatar_path (USD path or unit).
// Run on both an avatar-free scene and a scene with an authored avatar.
// SaveAs writes an ignored test artifact and leaves the input scene untouched.

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["SceneCameraAudit", "SyncUsdDocument"], required_inputs: [] }
}

fn on_start(me, params) {
    this.done = false;
    this.ticks = 0;
    this.saved = false;
    this.doc = params.doc_id;
    this.expected_path = params.authored_avatar_path;
    this.before = query("SyncUsdDocument", #{ doc_id: this.doc });
}

fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    let audit = query("SceneCameraAudit", #{});
    let avatars = [];
    if audit != () && audit.candidates != () {
        for camera in audit.candidates {
            if camera.local_avatar && camera.avatar { avatars.push(camera); }
        }
    }
    if avatars.len() != 1 && this.ticks < 600 { return; }
    let checks = [t_true(avatars.len() == 1, "the viewed scene has exactly one local avatar")];
    if avatars.len() == 1 {
        checks.push(t_true(avatars[0].usd_path == this.expected_path,
            "avatar provisioning preserves authored identity or supplies a transient rig"));
    }
    if this.before == () || this.before.layers == () {
        checks.push(t_true(false, "the exact scene document snapshot is available"));
    } else {
        if !this.saved {
            let saved = cmd("SaveAsDocument", #{ doc_id: this.doc,
                path: "target/scene-tests/scene_avatar_saved.usda" });
            if saved.ok {
                this.saved = true;
                return;
            }
            checks.push(t_true(false, "the exact scene document can be saved"));
        }
        let after = query("SyncUsdDocument", #{ doc_id: this.doc });
        checks.push(t_true(after != () && after.layers != ()
            && after.layers.root.source == this.before.layers.root.source
            && after.layers.runtime.source == this.before.layers.runtime.source
            && after.layers.view.source == this.before.layers.view.source,
            "avatar provisioning and saving preserve all authored document layers"));
    }
    this.done = true;
    report_verdict(checks, "SCENE AVATAR PRESENCE", "SCENE_AVATAR_PRESENCE");
}
