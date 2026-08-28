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

fn on_start(me) {
    this.elapsed = 0.0;
    this.done = false;
    this.sent = false;
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

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    if this.target < 0 {
        if this.elapsed > 5.0 {
            fail(this, "nested transform target did not compose: " + TARGET_PATH);
        }
        return;
    }

    if !this.sent {
        this.before = world_pos(this.target);
        if this.before == () { return; }

        let result = cmd("TransformEntity", #{
            entity_id: this.target,
            translation: TARGET_POSITION,
            rotation: TARGET_ROTATION,
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
    let checks = [];
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
