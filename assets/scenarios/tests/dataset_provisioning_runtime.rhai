// Public-runtime assertions used by the windowed dataset certification driver.
// Network pacing and archive installation fixtures belong to the driver; the
// authoritative state, UI publications, and clock verdicts stay here.
fn certification_dataset(scope, key) {
    let report = query("ListDatasets", #{scope: scope});
    for dataset in report.datasets {
        if dataset.key == key { return dataset; }
    }
    throw "Missing declared dataset: " + scope + "/" + key;
}

fn certify_dataset(scope, key, kind, visible, action, text) {
    let dataset = certification_dataset(scope, key);
    let consent = query("ReadExposures", #{surface: "asset-consent"}).surfaces["asset-consent"];
    let checks = [
        t_eq(dataset.scope, scope, "canonical scope survives the real query boundary"),
        t_eq(dataset.state.kind, kind, "real provisioning phase"),
        t_eq(consent.visible, visible, "consent visibility"),
    ];
    if visible && kind != "installed" {
        let row = ();
        for candidate in consent.properties.rows {
            if candidate.id == dataset.id { row = candidate; }
        }
        checks.push(t_true(row != (), "the live dataset has a retained UI row"));
        if row != () {
            checks.push(t_eq(row.scope, scope, "row action retains the real scope"));
            checks.push(t_eq(row.action, action, "row action matches its phase"));
            checks.push(t_true(row.status.contains(text), "status retains progress or failure detail"));
        }
    }
    if kind == "downloading" {
        checks.push(t_true(dataset.state.bytes_done > 0, "real received bytes are published"));
        checks.push(t_true(dataset.state.bytes_total > dataset.state.bytes_done,
            "the sample observes an unfinished transfer"));
    }
    if kind == "installed" && visible {
        checks.push(t_eq(consent.properties.title, "Data ready", "completion is explicit"));
    }
    t_report(checks);
}

fn certify_terrain_hold(dataset_key, tick_before) {
    let clock = clock_snapshot();
    let found = false;
    for blocker in clock.simulation_progress.blockers {
        if blocker.owner == "TerrainPreparation" && blocker.reason.contains(dataset_key) {
            found = true;
        }
    }
    t_report([
        t_true(clock.simulation_progress.held, "missing ground holds authoritative ticks"),
        t_true(found, "the terrain hold identifies the exact required dataset"),
        t_eq(clock.sim_tick, tick_before, "simulation cannot advance without ground"),
        t_true(!query("GetReadiness", #{}).ready, "readiness does not claim a usable world"),
    ]);
}

fn certify_resumed(tick_before, body_path, entity_before) {
    let clock = clock_snapshot();
    let terrain_held = false;
    for blocker in clock.simulation_progress.blockers {
        if blocker.owner == "TerrainPreparation" { terrain_held = true; }
    }
    t_report([
        t_true(!terrain_held, "the installed ground releases its owned preparation hold"),
        t_true(clock.sim_tick > tick_before, "simulation resumes through the existing terrain bridge"),
        t_true(query("GetReadiness", #{}).ready, "scene readiness clears after installation"),
        t_eq(find_path(body_path), entity_before, "dataset installation preserves the mounted scene entity"),
    ]);
}

fn certify_paused(tick_before) {
    let clock = clock_snapshot();
    t_report([
        t_true(!clock.transport_playing, "installation preserves an intentional pause"),
        t_eq(clock.sim_tick, tick_before, "a paused simulation does not advance on installation"),
    ]);
}

fn certify_retired(scope) {
    let present = false;
    for dataset in query("ListDatasets", #{}).datasets {
        if dataset.scope == scope { present = true; }
    }
    t_report([t_true(!present, "Twin replacement retires the outgoing dataset scope")]);
}

fn certify_modelica(paths) {
    let report = query("CosimStatus", #{include_values: false, include_entities: false});
    let checks = [
        t_eq(report.synchronization.active_participants, paths.len(), "all expected Modelica participants are active"),
        t_eq(report.synchronization.shared_clock_participants, paths.len(), "all participants share the master clock"),
        t_eq(report.modelica_step_profile.participants, paths.len(), "all participants committed a real solver step"),
    ];
    for path in paths {
        let entity = find_path(path);
        checks.push(t_true(entity >= 0, path + " has a projected identity"));
        let time = get(entity, "ModelicaModel.current_time");
        checks.push(t_true(time != () && time > 1.0, path + " publishes advancing state"));
        checks.push(t_eq(get(entity, "ModelicaModel.paused"), false, path + " is running"));
    }
    t_report(checks);
}
