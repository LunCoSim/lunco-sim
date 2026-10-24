const TEST_KIND = "headless";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [find_path("/World/A/ContactValidity"),
        find_path("/World/Independent/ContactValidity")],
      entity_reads: [find_path("/World/A"), find_path("/World/B"), find_path("/World/Independent")],
      entity_writes: [], query_reads: ["QueryPhysicsState", "GetReadiness"], required_inputs: []}
}
fn on_start(me, ctx) {
    this.started = sim_tick(); this.done = false; this.paused_at = ();
    this.a = find_path("/World/A"); this.b = find_path("/World/B");
    this.model = find_path("/World/A/ContactValidity");
    this.control = find_path("/World/Independent");
    this.control_model = find_path("/World/Independent/ContactValidity");
    this.events = 0; this.warned = false; this.impact = false;
    subscribe("simulation-object-paused"); subscribe("test.contact-speed-limit");
}
fn on_event(me, evt, ctx) {
    if evt.name == "test.contact-speed-limit" && evt.source == this.model {
        this.impact = evt.value > 1.0 && evt.sim_tick <= sim_tick();
        emit("CONTACT_IMPACT_EVENT_EVIDENCE", evt);
    }
    if evt.name == "simulation-object-paused" && evt.source == this.a {
        this.events += 1;
        this.warned = evt.severity == "Warning" && evt.value.contains("Impact normal approach speed=")
            && evt.value.contains("validity limit=5") && evt.value.contains("not meaningful");
    }
}
fn on_tick(me, ctx) {
    if this.done { return; }
    let a = query("QueryPhysicsState", #{id: this.a});
    if a.simulation_object_paused && this.paused_at == () {
        this.paused_at = sim_tick(); this.pose = world_pos(this.a);
        this.model_time = get(this.model, "ModelicaModel.current_time");
        this.body_model_time = get(this.a, "ModelicaModel.current_time");
    }
    if sim_tick() - this.started < 150 { return; }
    this.done = true;
    let b = query("QueryPhysicsState", #{id: this.b});
    let control = query("QueryPhysicsState", #{id: this.control});
    let ready = query("GetReadiness", #{});
    let held = this.paused_at != () && sim_tick() - this.paused_at > 30;
    report_verdict([
        t_true(this.impact, "native contact produces a retained Modelica validity event"),
        t_true(held && a.rigid_body_disabled && b.rigid_body_disabled && b.simulation_object_paused,
            "an excessive impact pauses the whole joint-connected object"),
        t_true(held && get(this.model, "ModelicaModel.paused") == true,
            "the impact observer's continuous model pauses with the object"),
        t_true(held && get(this.a, "ModelicaModel.paused") == true,
            "a continuous model sharing the impacted rigid body pauses too"),
        t_true(held && world_pos(this.a) == this.pose
            && get(this.model, "ModelicaModel.current_time") == this.model_time,
            "pose and Modelica clock remain fixed after the accepted pause"),
        t_true(held && this.body_model_time > 0.0
            && get(this.a, "ModelicaModel.current_time") == this.body_model_time,
            "native contact initializes beside a real body model and both clocks stop"),
        t_true(this.events == 1 && this.warned, "one Recent Events warning explains speed, limit and model validity"),
        t_true(!control.simulation_object_paused && contact_debug(this.control).touching_pair_count > 0,
            "the lower-speed control reaches contact without a false failure"),
        t_true(get(this.control_model, "ModelicaModel.current_time") > 1.0
            && get(this.control_model, "ModelicaModel.paused") == false,
            "the independent model keeps integrating within its envelope"),
        t_true(ready.ready && !ready.world_hold && !ready.faulted,
            "containment preserves scene readiness and inspection")
    ], "CONTACT IMPACT PAUSE", "CONTACT_IMPACT_PAUSE");
}
