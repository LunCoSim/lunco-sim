// Production acceptance for the Twin runtime-layer write and restore boundary.
const TEST_KIND = "headless";
const ROUTE = "/RouteRuntimePersistence/Route";
const POINT = ROUTE + "/P0";

fn runtime_point(doc) {
    query("ResolveUsdTarget", #{
        doc_id: doc,
        path: POINT,
        edit_target: "@runtime@",
    })
}

fn finish(me, passed, message) {
    me.done = true;
    report_verdict([t_true(passed, message)],
        "ROUTE RUNTIME PERSISTENCE", "ROUTE_RUNTIME_PERSISTENCE");
}

fn on_start(me, ctx) {
    this.phase = 0;
    this.elapsed = 0.0;
    this.done = false;
    this.doc = -1;
    this.plan = ();
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 20.0 {
        finish(this, false, "runtime route edit or restored point did not become observable");
        return;
    }

    if this.phase == 0 {
        let route = query("QueryUsdPrim", #{ path: ROUTE });
        if route == () || route.ok == false || route.doc_id == () { return; }
        this.doc = route.doc_id;
        let point = runtime_point(this.doc);
        if point != () && point.ok != false && point.authored_here == true {
            print("ROUTE_RUNTIME_GATE: RESTORED_AT_START");
            finish(this, true,
                "the runtime route point was restored before the initial scene projection");
            return;
        }

        print("ROUTE_RUNTIME_GATE: ABSENT_AT_START");
        this.plan = waypoint_editor::marker_ops(this.doc, ROUTE, [0.0, 0.0, 0.0]);
        if this.plan == () || this.plan.ok != true {
            finish(this, false, "route editor could not build a runtime-layer point edit");
            return;
        }
        let applied = waypoint_editor::apply_ops(
            this.doc, "Persist route waypoint", this.plan.ops,
        );
        if applied == () || applied.ok != true {
            finish(this, false, "route editor could not author the runtime-layer point");
            return;
        }
        this.phase = 1;
        return;
    }

    let point = runtime_point(this.doc);
    if point != () && point.ok != false && point.authored_here == true {
        finish(this, true,
            "the route editor authored its waypoint in the document runtime layer");
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        finish(this, false, "scene stopped before route runtime persistence verdict");
    }
}
