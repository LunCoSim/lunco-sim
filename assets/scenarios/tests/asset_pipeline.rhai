// Asset-pipeline public behavior. The production driver supplies only fixture
// identities and expected lifecycle outcomes; verdicts stay authored here.
fn pipeline_dataset(scope, key) {
    for dataset in query("ListDatasets", #{scope:scope}).datasets {
        if dataset.key == key { return dataset; }
    }
    throw "pipeline dataset is not declared: " + key;
}

fn verify_runtime_download_catalog() {
    let checks=[];
    let earth=false;let moon=false;let font=false;
    let font_state="missing";
    for dataset in query("ListDatasets",#{}).datasets {
        if dataset.scope!="engine" {continue;}
        if dataset.key=="earth" {earth=true;}
        if dataset.key=="moon" {moon=true;}
        if dataset.key=="dejavu_sans" {font=true;font_state=dataset.state.kind;}
    }
    checks.push(t_true(earth && moon,"planetary imagery remains in the downloadable catalog"));
    checks.push(t_true(font,"the application fallback font is listed in the downloadable catalog"));
    checks.push(t_true(font_state=="missing" || font_state=="installed",
        "a missing fallback font remains available for explicit download without startup work"));
    t_report(checks);
}

fn verify_scene_assets(scope,keys) {
    let selected=[];
    for dataset in query("ListDatasets",#{scope:scope}).datasets {
        if dataset.required_for_scene {selected.push(dataset.key);}
    }
    selected.sort();keys.sort();
    t_report([t_eq(selected,keys,"the composed scene requires its material inputs and excludes optional territories")]);
}

fn verify_terrain_visible() {
    let stream=query("TerrainLodStatus",#{}).stream;
    t_report([
        t_true(stream.wanted>0,"the terrain has a selected visual cover"),
        t_eq(stream.resident,stream.wanted,"every selected terrain tile crosses material readiness"),
        t_eq(stream.pending,0,"no missing source image leaves rendering pending"),
    ]);
}

fn verify_pipeline(scope, key, state, artifact_count, step_count) {
    let dataset = pipeline_dataset(scope,key);
    t_report([
        t_eq(dataset.state.kind,state,"recipe has the expected terminal lifecycle state"),
        t_true(dataset.processed,"recipe is owned by native processing"),
        t_eq(dataset.recipe.artifacts.len(),artifact_count,"declared artifacts are exposed through the native query"),
        t_eq(dataset.recipe.steps.len(),step_count,"declared processing stages retain their identity"),
        t_true(dataset.artifact_uri.starts_with("twin://"),"artifact retains logical Twin ownership"),
    ]);
}

fn verify_dataset_state(scope, key, state) {
    let dataset = pipeline_dataset(scope, key);
    t_report([
        t_eq(dataset.state.kind, state, "processed asset freshness has the expected lifecycle state"),
        t_true(dataset.processed, "freshness is checked for manifest-owned processing"),
    ]);
}

fn verify_texture_dimensions(width, height, expected_width, expected_height) {
    t_report([
        t_eq(width, expected_width, "rebaked texture has the current recipe width"),
        t_eq(height, expected_height, "rebaked texture has the current recipe height"),
    ]);
}

fn verify_pipeline_failure(scope, key, text) {
    let dataset = pipeline_dataset(scope,key);
    t_report([
        t_eq(dataset.state.kind,"failed","invalid processing is terminal"),
        t_true(dataset.state.error.contains(text),"failure identifies the owning stage or invalid input"),
    ]);
}

fn verify_pipeline_terrain(scope, key, path) {
    let dataset = pipeline_dataset(scope,key);
    let terrain = query("TerrainHeights", #{points:[[0.0,0.0],[100.0,100.0]]});
    t_report([
        t_eq(dataset.state.kind,"installed","the required terrain artifact is installed"),
        t_eq(dataset.artifact_uri,path,"terrain admission consumes the exact declared artifact"),
        t_true(terrain[0].found,"terrain owner publishes the prepared surface after artifact installation"),
        t_true(terrain[1].found && abs(terrain[1].height-terrain[0].height)>0.0001,"prepared terrain retains measured relief"),
    ]);
}

fn verify_generated_data(input, output, factor, times, values, gain, tolerance) {
    let checks = [
        t_eq(input.len(),output.len(),"shader output retains the declared pixel layout"),
        t_eq(times.len(),values.len(),"Modelica artifact has one value per declared sample time"),
        t_true(times.len()>1,"Modelica artifact contains an actual run"),
    ];
    for index in 0..input.len() {
        let expected = if index%4==3 {input[index]} else {(input[index]*factor).round()};
        checks.push(t_true(abs(output[index]-expected)<=1.0,"dynamic shader uses the authored processing factor"));
    }
    for index in 0..times.len() {
        checks.push(t_true(abs(values[index]-gain*times[index])<=tolerance,"dynamic Modelica produces the declared physical result"));
    }
    t_report(checks);
}

fn verify_preserved_artifact(before, after) {
    t_report([t_eq(before,after,"failed processing preserves the last valid published bundle")]);
}

fn verify_source_requests(paths) {
    let primary=0;
    let additional=0;
    for path in paths {
        if path=="/input.png" {primary+=1;}
        if path=="/reference.png" {additional+=1;}
    }
    t_report([
        t_eq(primary,1,"a cached primary source is reused"),
        t_eq(additional,2,"a missing additional source is fetched even with a cached primary"),
    ]);
}
fn verify_force_runs(paths,expected) {
    let runs=0;
    for path in paths {if path=="/processed" {runs+=1;}}
    t_report([t_eq(runs,expected,"forced rebuild executes stages while ordinary preparation reuses a valid bundle")]);
}

fn verify_preparation_view(scope,key,action,error) {
    let dataset=pipeline_dataset(scope,key);
    let view=query("ReadExposures",#{surface:"asset-consent"}).surfaces["asset-consent"];
    let row=();
    for candidate in view.properties.rows {if candidate.id==dataset.id {row=candidate;}}
    let checks=[t_true(view.visible && row!=(),"the current dataset has a visible preparation view")];
    if row!=() {
        checks.push(t_eq(row.source_url,dataset.inputs[0].url,"source link uses the declared download address"));
        checks.push(t_true(row.download_status.contains("Source files"),"download readiness is separate from preparation"));
        checks.push(t_true(row.preparation_status!="","preparation status remains explicit"));
        checks.push(t_eq(row.action,action,"the action matches source and artifact readiness"));
        if error!="" {checks.push(t_true(row.detail.contains(error),"the full preparation error remains visible"));}
    }
    t_report(checks);
}

fn certify_scene_dependencies(scope, keys, held) {
    let actual=[];
    for dataset in query("ListDatasets",#{scope:scope}).datasets {
        if dataset.required_for_scene {actual.push(dataset.key);}
    }
    actual.sort();keys.sort();
    let gate=false;
    for blocker in clock_snapshot().simulation_progress.blockers {if blocker.owner=="SceneDatasets" {gate=true;}}
    t_report([t_eq(actual,keys,"requirements follow the selected scene"),t_eq(gate,held,"scene completeness gate follows required artifacts")]);
}
