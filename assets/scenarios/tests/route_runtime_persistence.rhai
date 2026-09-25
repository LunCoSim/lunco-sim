// Production acceptance for the Twin runtime-layer write and restore boundary.
const TEST_KIND = "headless";
const ROUTE = "/RouteRuntimePersistence/Route";
const POINT = ROUTE + "/P0";

fn runtime_point(doc, path) {
    query("ResolveUsdTarget", #{
        doc_id: doc,
        path: path,
        edit_target: "@runtime@",
    })
}

fn trigger_zone(doc, path) {
    let record = query("QueryUsdPrim", #{
        doc_id: doc,
        path: path + "/Trigger",
        attrs: ["lunco:triggerZone"],
    });
    if record == () || record.ok == false || record.attrs == () { return (); }
    record.attrs["lunco:triggerZone"]
}

fn finish(me, passed, message) {
    me.done = true;
    report_verdict([t_true(passed, message)],
        "ROUTE RUNTIME PERSISTENCE", "ROUTE_RUNTIME_PERSISTENCE");
}

fn route_document_ready(me) {
    let route = query("QueryUsdPrim", #{ path: ROUTE });
    route != () && route.ok != false && route.doc_id != ()
}

fn discover_runtime_state(me) {
    let route = query("QueryUsdPrim", #{ path: ROUTE });
    if route == () || route.ok == false || route.doc_id == () {
        this.failure = "route test document became unavailable";
        return;
    }
    this.doc = route.doc_id;
    let point = runtime_point(this.doc, POINT);
    this.restored = point != () && point.ok != false && point.authored_here == true;
    this.waiting_for = if this.restored { "restored route trigger" }
        else { "new runtime waypoint" };
}

fn test_state_is_valid(me) { this.failure == "" }
fn route_point_was_restored(me) { this.restored == true }
fn route_point_is_new(me) { this.restored != true }

fn create_runtime_point(me) {
    print("ROUTE_RUNTIME_GATE: ABSENT_AT_START");
    let plan = waypoint_editor::marker_ops(this.doc, ROUTE, [0.0, 0.0, 0.0]);
    if plan == () || plan.ok != true {
        this.failure = "route editor could not build a runtime-layer point edit";
        return;
    }
    let applied = waypoint_editor::apply_ops(
        this.doc, "Create route waypoint", plan.ops,
    );
    if applied == () || applied.ok != true {
        this.failure = "route editor could not author the runtime-layer point";
        return;
    }
    this.waiting_for = "runtime waypoint authoring";
}

fn verify_restored_trigger(me) {
    print("ROUTE_RUNTIME_GATE: RESTORED_AT_START");
    if trigger_zone(this.doc, POINT) != "route_P0" {
        this.failure = "restored runtime waypoint lost its authored trigger override";
    }
}

fn report_new_waypoint_pass(me) {
    finish(this, true, "route editor authored a runtime waypoint in one document transaction");
}

fn report_restored_waypoint_pass(me) {
    finish(this, true, "restored runtime route point retains its authored trigger override");
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
    this.doc = -1;
    this.restored = false;
    this.failure = "";
    this.waiting_for = "route test document";
    subscribe("TASK_FAILED");
}

fn task(me, ctx) {
    reactive_seq([
        check(Fn("test_state_is_valid")),
        seq([
            wait_until(Fn("route_document_ready")),
            once(Fn("discover_runtime_state")),
            check(Fn("test_state_is_valid")),
            sel([
                seq([
                    check(Fn("route_point_was_restored")),
                    once(Fn("verify_restored_trigger")),
                    check(Fn("test_state_is_valid")),
                    once(Fn("report_restored_waypoint_pass")),
                ]),
                seq([
                    check(Fn("route_point_is_new")),
                    once(Fn("create_runtime_point")),
                    check(Fn("test_state_is_valid")),
                    once(Fn("report_new_waypoint_pass")),
                ]),
            ]),
        ]),
    ])
}

fn on_event(me, evt, ctx) {
    if evt.name == "TASK_FAILED" && !this.done {
        let message = if this.failure == "" {
            "route runtime task sequence failed"
        } else { this.failure };
        finish(this, false, message);
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 20.0 {
        finish(this, false,
            "route runtime test timed out waiting for " + this.waiting_for);
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        finish(this, false,
            "scene stopped before route runtime persistence verdict while waiting for "
                + this.waiting_for);
    }
}
