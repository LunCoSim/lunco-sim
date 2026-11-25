// Production/headless acceptance test for the complete pose command.
//
// This is authored Rhai because it exercises the public command/query surface
// and the same USD projection path used by the editor. The nested rotated
// parent is intentional: a root-only test cannot catch the parent-frame mistake
// that made a gizmo direction disappear or sent an object one cell away.

const TARGET_PATH = "/TransformCommandTest/Assembly/Target";
const TARGET_POSITION = [101.0, 202.0, 303.0];
const TARGET_ROTATION = [0.0, 0.1305261922, 0.0, 0.9914448614];
const PARENT_POSITION = [10.0, 20.0, -30.0];
const PARENT_INVERSE_ROTATION = [0.0, -0.3420201433, 0.0, 0.9396926208];
const MAX_POSITION_ERROR = 0.0001;
const MAX_ROTATION_ERROR = 0.000001;

fn simulation_dependencies(me, ctx) {
    let target = find_path(TARGET_PATH);
    #{ modelica_entities: [], entity_reads: [target], entity_writes: [target],
       query_reads: ["QueryUsdPrim"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
    this.sent = false;
    this.runtime_tested = false;
    this.rejection_tested = false;
    this.runtime_checks = [];
    this.target = find(TARGET_PATH);
    this.before = ();
    print("[transform-entity] -- active-frame nested-pose contract --");
}

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "TRANSFORM ENTITY", "TRANSFORM_ENTITY");
    }
}

fn vec_error(actual, expected) {
    if actual == () { return (); }
    vlen(vsub(actual, expected))
}

fn quat_abs_dot(a, b) {
    if a == () || b == () { return (); }
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    dot.abs()
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();

    if this.target < 0 {
        if this.elapsed > 5.0 {
            fail(this, "nested transform target did not compose: " + TARGET_PATH);
        }
        return;
    }

    if !this.sent {
        if this.before == () { this.before = world_pos(this.target); }
        if this.before == () { return; }

        if !this.runtime_tested {
            let authored = query("QueryUsdPrim", #{path:TARGET_PATH, attrs:["xformOp:translate"]});
            if authored == () || authored.attrs == () { return; }
            this.initial_authored = authored.attrs["xformOp:translate"];
            this.generation = authored.document_generation;
            this.visual = find_path(TARGET_PATH + "/Visual");
            if this.visual < 0 { return; }
            let result = cmd("TransformEntity", #{
                entity_id: this.target,
                translation: TARGET_POSITION,
                rotation: TARGET_ROTATION
            });
            if !result.ok { fail(this, "Interactive pose was rejected: " + result.error); return; }
            this.runtime_tested = true;
            return;
        }
        if !this.rejection_tested {
            let live = world_pos(this.target);
            let authored = query("QueryUsdPrim", #{path:TARGET_PATH, attrs:["xformOp:translate"]});
            this.runtime_checks.push(t_max(vec_error(live, TARGET_POSITION), MAX_POSITION_ERROR,
                "interactive pose changes the live active-frame position"));
            this.runtime_checks.push(t_true(authored.document_generation == this.generation &&
                authored.attrs["xformOp:translate"] == this.initial_authored &&
                find_path(TARGET_PATH + "/Visual") == this.visual,
                "interactive pose retains authored USD generation and visual identity"));
            let invalid = cmd("TransformEntity", #{
                entity_id: this.target, translation: TARGET_POSITION,
                rotation: [0.0, 0.0, 0.0, 0.0], intent: "Interactive"
            });
            this.runtime_checks.push(t_true(!invalid.ok &&
                query("QueryUsdPrim", #{path:TARGET_PATH}).document_generation == this.generation,
                "a degenerate runtime pose is rejected without editing USD"));
            let rejected = cmd("TransformEntity", #{
                entity_id: this.target, translation: [1.0e60, 0.0, 0.0],
                rotation: TARGET_ROTATION, intent: "Persistent"
            });
            this.runtime_checks.push(t_true(!rejected.ok, "unrepresentable persistent pose is rejected"));
            let moved = cmd("MoveEntity", #{entity_id:this.target, translation:[102.0,202.0,303.0]});
            let rotated = cmd("RotateEntity", #{entity_id:this.target, rotation:[0.0,0.0,0.0,1.0]});
            this.runtime_checks.push(t_true(moved.ok && rotated.ok && vec_error(world_pos(this.target), [102.0,202.0,303.0]) < MAX_POSITION_ERROR,
                "partial pose commands also default to live state"));
            let bad_move = cmd("MoveEntity", #{entity_id:this.target, translation:[1.0e60,0.0,0.0], intent:"Persistent"});
            let bad_rotation = cmd("RotateEntity", #{entity_id:this.target, rotation:[0.0,0.0,0.0,0.0], intent:"Persistent"});
            this.runtime_checks.push(t_true(!bad_move.ok && !bad_rotation.ok, "invalid persistent partial pose commands reject before journaling"));
            let restored = cmd("TransformEntity", #{entity_id:this.target, translation:TARGET_POSITION, rotation:TARGET_ROTATION});
            if !restored.ok { fail(this, "live pose restoration failed"); return; }
            this.rejection_tested = true;
            return;
        }
        let unchanged = query("QueryUsdPrim", #{path:TARGET_PATH, attrs:["xformOp:translate"]});
        this.runtime_checks.push(t_true(unchanged.document_generation == this.generation &&
            unchanged.attrs["xformOp:translate"] == this.initial_authored &&
            vec_error(world_pos(this.target), TARGET_POSITION) < MAX_POSITION_ERROR,
            "rejected persistent transform neither authors nor changes the live pose"));
        let result = cmd("TransformEntity", #{
            entity_id: this.target,
            translation: TARGET_POSITION,
            rotation: TARGET_ROTATION, intent:"Persistent",
        });
        if !result.ok {
            fail(this, "TransformEntity was rejected: " + result.error);
            return;
        }
        this.sent = true;
        return;
    }

    // The live command is synchronous, while the USD operation is deliberately
    // deferred through the normal command observer. Give both projections a
    // tick before measuring them.
    if this.elapsed < 1.0 { return; }

    let position = world_pos(this.target);
    let rotation = world_rotation(this.target);
    let entity = query("QueryEntity", #{ id: this.target });
    let authored = query("QueryUsdPrim", #{
        path: TARGET_PATH,
        attrs: ["xformOp:translate", "xformOp:rotateXYZ"],
    });
    if position == () || rotation == () || entity == () || authored == () {
        if this.elapsed > 8.0 {
            fail(this, "TransformEntity did not expose live and authored pose reads");
        }
        return;
    }

    let attrs = authored.attrs;
    if attrs == () || attrs["xformOp:translate"] == () {
        if this.elapsed > 8.0 {
            fail(this, "TransformEntity did not reach the composed USD runtime layer");
        }
        return;
    }

    let expected_local = qrot(
        PARENT_INVERSE_ROTATION,
        vsub(TARGET_POSITION, PARENT_POSITION),
    );
    let position_error = vec_error(position, TARGET_POSITION);
    let authored_error = vec_error(attrs["xformOp:translate"], expected_local);
    let rotation_dot = quat_abs_dot(rotation, TARGET_ROTATION);
    let checks = this.runtime_checks;
    checks.push(t_moved(distance_from(this.before, position), 1.0,
        "TransformEntity did not move the target"));
    checks.push(t_max(position_error, MAX_POSITION_ERROR,
        "live active-frame position changed during conversion"));
    checks.push(t_max(authored_error, MAX_POSITION_ERROR,
        "USD stored the wrong parent-local translation"));
    checks.push(t_true(rotation_dot != () && rotation_dot >= 1.0 - MAX_ROTATION_ERROR,
        "live active-frame rotation changed during conversion"));
    checks.push(t_true(entity.position_frame == "active_physics",
        "QueryEntity did not identify the active physics frame"));
    checks.push(t_present(attrs["xformOp:rotateXYZ"],
        "USD runtime layer has no authored rotation"));
    report_verdict(checks, "TRANSFORM ENTITY", "TRANSFORM_ENTITY");
    this.done = true;
}

fn distance_from(a, b) {
    if a == () || b == () { return (); }
    vlen(vsub(a, b))
}
