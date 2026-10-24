const TEST_KIND = "headless";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [],
       entity_reads: [find_path("/World/A"), find_path("/World/B"), find_path("/World/Independent")],
       entity_writes: [find_path("/World/A"), find_path("/World/B")],
       query_reads: ["GetReadiness"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
    this.sent = false;
    this.repeated = false;
    this.events = 0;
    this.event_detail = false;
    this.event_warning = false;
    this.a = find_path("/World/A");
    this.b = find_path("/World/B");
    this.independent = find_path("/World/Independent");
    this.independent_y = world_pos(this.independent)[1];
}

fn on_event(me, evt, ctx) {
    if evt.name == "simulation-object-paused" && evt.source == this.a {
        this.events += 1;
        this.event_warning = evt.severity == "Warning";
        this.event_detail = evt.value.contains("test:energy-capacity-exceeded")
            && evt.value.contains("12 J exceeds the 10 J test capacity");
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if !this.sent && this.elapsed >= 0.1 {
        let invalid = cmd("PauseSimulationObject", #{ target: this.a, condition: "", detail: "missing condition" });
        this.invalid_rejected = invalid.ok == false;
        this.before_a = world_pos(this.a);
        this.before_b = world_pos(this.b);
        let paused = cmd("PauseSimulationObject", #{ target: this.a,
            condition: "test:energy-capacity-exceeded",
            detail: "12 J exceeds the 10 J test capacity" });
        this.accepted = paused.ok == true;
        this.sent = true;
    }
    if this.sent && !this.repeated && this.elapsed >= 0.2 {
        let repeated = cmd("PauseSimulationObject", #{ target: this.a,
            condition: "test:duplicate", detail: "must retain the first explanation" });
        this.repeat_accepted = repeated.ok == true;
        this.repeated = true;
    }
    if this.elapsed < 0.5 { return; }
    let a = query("QueryPhysicsState", #{ id: this.a });
    let b = query("QueryPhysicsState", #{ id: this.b });
    let independent = query("QueryPhysicsState", #{ id: this.independent });
    let ready = query("GetReadiness", #{});
    this.done = true;
    report_verdict([
        t_true(this.invalid_rejected, "missing condition rejects the pause before mutation"),
        t_true(this.accepted, "authored policy can pause its object"),
        t_true(a.physics_object_paused && a.rigid_body_disabled, "target physics is disabled"),
        t_true(b.physics_object_paused && b.rigid_body_disabled, "joint-connected physics is disabled"),
        t_true(vlen(vsub(world_pos(this.a), this.before_a)) < 1.0e-9, "target retains its accepted pose"),
        t_true(vlen(vsub(world_pos(this.b), this.before_b)) < 1.0e-9, "connected body retains its accepted pose"),
        t_true(!independent.physics_object_paused
            && world_pos(this.independent)[1] < this.independent_y - 0.05,
            "unrelated physics continues integrating"),
        t_true(a.simulation_object_pause_reason.condition == "test:energy-capacity-exceeded"
            && b.simulation_object_pause_reason == a.simulation_object_pause_reason,
            "all connected bodies expose the retained condition and explanation"),
        t_true(this.events == 1 && this.event_detail,
            "one sourced Recent Events observation explains the measured limit"),
        t_true(this.event_warning, "the accepted object pause is a Recent Events warning"),
        t_true(ready.ready && !ready.world_hold && !ready.faulted, "object pause preserves world readiness"),
        t_true(this.repeated && this.repeat_accepted,
            "a repeated request is idempotent and preserves the original reason"),
    ], "PHYSICS OBJECT PAUSE", "PHYSICS_OBJECT_PAUSE");
}
