// @timing presentation
// The navigation body owns the touchdown reference. Its visual footprint is
// a disposable surface annotation, prepared by the shared curve-view owner.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [me,
        find_path(usd_path(me) + "/SurfaceMark/Geometry")], entity_writes: [],
      query_reads: ["QueryUsdPrim", "TerrainHeight", "GroundHeight", "InspectUsdCurveView", "SceneCameraAudit"],
      required_inputs: []}
}

fn footprint_source(path) {
    let source = query("QueryUsdPrim", #{path: path + "/Dome", attrs: ["radius"]});
    if source == () || source.doc_id == () || source.attrs.radius == ()
        || !source.attrs.radius.is_finite() || source.attrs.radius <= 0.0 {
        throw "Landing annotation requires a finite positive authored footprint radius";
    }
    source
}

fn on_start(me, ctx) {
    subscribe("usd.document.projected");
    this.path = usd_path(me);
    this.rendered = false;
    let audit = query("SceneCameraAudit", #{});
    if audit == () || audit.candidates == () {throw "Landing annotation host audit unavailable";}
    for camera in audit.candidates {
        if camera.render_camera == true {this.rendered = true;}
    }
    let source = footprint_source(this.path);
    this.doc = source.doc_id;
    this.radius = source.attrs.radius;
    this.root = this.path + "/SurfaceMark";
    this.curve = this.root + "/Geometry";
    this.refresh = true;
}

fn on_event(me, evt, ctx) {
    if evt.name != "usd.document.projected" || evt.value == () || evt.value.doc_id != this.doc {return;}
    for path in evt.value.changed_prim_paths {
        if path == this.path {this.refresh = true;}
    }
}

fn submit(me, state) {
    if !state.rendered {return true;}
    let root = find_path_in_document(state.doc, state.root);
    let curve = find_path_in_document(state.doc, state.curve);
    if root < 0 || curve < 0 {return false;}
    // USD identity precedes asynchronous mesh and material projection.
    if get(curve, "PbrLook") == () {return false;}
    let pose = world_pos3(me);
    let rotation = world_rotation_quat(me);
    if pose == () || rotation == () {throw "Landing annotation frame is unavailable";}
    let terrain = query("TerrainHeight", #{x: pose.x, z: pose.z});
    let ground = if terrain != () && terrain.found == true {terrain.height} else {
        let hit = query("GroundHeight", #{x: pose.x, z: pose.z, from: pose.y});
        if hit == () || hit.hit != true {return false;}
        hit.height
    };
    if ground == () || !ground.is_finite() {throw "Landing annotation support is invalid";}
    let points = [];
    // A closed stroke of width radius around a half-radius circle fills the
    // footprint. Terrain fragments supply its elevation and occlusion.
    for i in 0..65 {
        let angle = to_float(i) * 6.283185307179586 / 64.0;
        let world = vec3(pose.x + state.radius * 0.5 * cos(angle), ground,
            pose.z + state.radius * 0.5 * sin(angle));
        let local = qrot(quat_inverse(rotation), world - pose);
        points.push([local.x, local.y, local.z]);
    }
    let result = cmd("UpdateUsdCurveView", #{doc_id: state.doc,
        root_entity_id: root, curve_entity_id: curve, points: points, width_m: state.radius});
    if result == () || result.ok != true {throw "Landing annotation projection failed: " + result;}
    true
}

fn published(state) {
    if !state.rendered {return true;}
    let curve = find_path_in_document(state.doc, state.curve);
    if curve < 0 {return false;}
    let view = query("InspectUsdCurveView", #{doc_id: state.doc, entity_id: curve});
    if view == () || view.state == "pending" {return false;}
    if view.state != "ready" {throw "Landing annotation publication failed: " + view;}
    true
}

fn publication_task() {
    par_race([
        seq([wait_until(|m| submit(m, this)),
            wait_until(|m| published(this)),
            once(|m| {if this.rendered {emit("landing_annotation_ready", #{path: this.path});} true})]),
        seq([wait(30.0), once(|m| {throw "Landing annotation support/publication timed out";})]),
    ])
}

fn task(me, ctx) {
    forever(seq([
        wait_until(|m| this.refresh),
        once(|m| {this.refresh=false;this.radius=footprint_source(this.path).attrs.radius;true}),
        publication_task(),
    ]))
}
