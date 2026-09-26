// Production acceptance for the scheduled physics initialization hook.
// The USD fixture selects a policy that accepts one finite body. The verdict
// proves that the owner supplied the expected generation-qualified lifecycle
// context and consumed the authored decision before physics admission.

const BODY_PATH = "/SandboxScene/Box";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [find(BODY_PATH)], entity_writes: [],
       query_reads: ["RuntimeDiagnostics"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.body = find(BODY_PATH);
    this.elapsed = 0.0;
    this.done = false;
    let lifecycle = policy_status().lifecycle;
    let lifecycle_context = lifecycle.runtime_context;
    let lifecycle_phase_matches = (lifecycle.event == "startup"
            || lifecycle.event == "reload")
        && lifecycle_context.phase == "start"
        || lifecycle.event == "assets_mounted"
            && lifecycle_context.phase == "event"
        || lifecycle.event == "close"
            && lifecycle_context.phase == "stop";
    this.twin_lifecycle_context_valid = lifecycle.status == "ok"
        && lifecycle_context.scope == "twin"
        && lifecycle_context.cycle == "lifecycle"
        && lifecycle_context.generation.to_string() != "0"
        && lifecycle_context.clock == "none"
        && lifecycle_context.time_seconds == ()
        && lifecycle_context.delta_seconds == ()
        && lifecycle_context.sequence == ()
        && lifecycle_context.producer == ()
        && lifecycle_phase_matches;
    let off_cycle = invoke_hook("readiness.action", [#{}]);
    this.readiness_off_cycle_rejected = off_cycle.installed == true
        && off_cycle.status == "fault";
    let acausal_off_cycle = invoke_hook("synth.acausal-network", [#{}]);
    this.acausal_synthesis_off_cycle_rejected = acausal_off_cycle.installed == true
        && acausal_off_cycle.status == "fault"
        && acausal_off_cycle.error.contains("Twin lifecycle preparation context");
    let actuator_off_cycle = invoke_hook("synth.actuator-wrench", [#{}]);
    this.actuator_synthesis_off_cycle_rejected = actuator_off_cycle.installed == true
        && actuator_off_cycle.status == "fault"
        && actuator_off_cycle.error.contains("Twin lifecycle preparation context");
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 0.1 { return; }

    let state = query("QueryPhysicsState", #{ id: this.body });
    let diagnostics = query("RuntimeDiagnostics", #{});
    let policy_error = false;
    let wrong_context = false;
    if diagnostics.ok == true {
        for finding in diagnostics.findings {
            if finding.producer == "physics-initialization"
                && finding.subject == BODY_PATH {
                if finding.code == "physics-initialization-policy" {
                    policy_error = true;
                }
                if finding.message.contains("wrong runtime context")
                    || finding.message.contains("wrong physics initialization facts") {
                    wrong_context = true;
                }
            }
        }
    }

    this.done = true;
    report_verdict([
        t_true(this.body >= 0, "the authored physics body resolves to its live API id"),
        t_true(this.twin_lifecycle_context_valid,
            "Twin lifecycle hooks receive a generation-qualified discrete lifecycle context"),
        t_true(state != () && state.ok != false, "the admitted body remains queryable"),
        t_true(state.physics_initialization_pending == false,
            "the authored initialization decision was consumed"),
        t_true(state.physics_state_pending == false,
            "the admitted body is no longer held by physics readiness"),
        t_true(state.body_mode == "dynamic",
            "the accepted authored body entered dynamic physics"),
        t_true(!policy_error, "the scheduled Rhai policy admitted the body without a diagnostic"),
        t_true(!wrong_context, "physics initialization supplied the expected owner facts and cycle"),
        t_true(this.readiness_off_cycle_rejected,
            "readiness.action rejected a scenario-start invocation outside its fixed-simulation cycle"),
        t_true(this.acausal_synthesis_off_cycle_rejected,
            "synth.acausal-network rejected a scenario-start invocation outside lifecycle preparation"),
        t_true(this.actuator_synthesis_off_cycle_rejected,
            "synth.actuator-wrench rejected a scenario-start invocation outside lifecycle preparation"),
    ], "PHYSICS INITIALIZATION POLICY", "PHYSICS_INITIALIZATION_POLICY");
}

fn on_stop(me, ctx) {
    if !this.done {
        this.done = true;
        report_verdict(["physics initialization policy stopped before its admission result was observed"],
            "PHYSICS INITIALIZATION POLICY", "PHYSICS_INITIALIZATION_POLICY");
    }
}
