// Observable task failure and explicit task-replacement contract.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: [], required_inputs: []}
}

fn on_start(me, ctx) {
    this.phase = 0;
    this.ticks = 0;
    this.calls = 0;
    this.failed = 0;
    this.completed = 0;
    this.recovered = 0;
    this.fallback_calls = 0;
    this.reason = "";
    this.checks = [];
    this.done = false;
    subscribe("TASK_FAILED");
    subscribe("TASK_COMPLETE");
    this.task = once(|m| { this.calls += 1; throw("action boundary rejected"); });
}

fn on_event(me, event, ctx) {
    if event.name == "TASK_FAILED" {
        this.failed += 1;
        this.reason = event.value;
    }
    if event.name == "TASK_COMPLETE" { this.completed += 1; }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks < 5 { return; }
    this.ticks = 0;
    if this.phase == 0 {
        this.checks += [
            t_true(this.calls == 1 && this.failed == 1 && this.completed == 0,
                "a throwing action fails once without retry or completion"),
            t_true(string_is(this.reason) && this.reason.contains("action boundary rejected"),
                "the failure event retains the action diagnostic")
        ];
        this.phase = 1;
        this.calls = 0;
        this.failed = 0;
        this.reason = "";
        this.task = wait_until(|m| { this.calls += 1; throw("predicate boundary rejected"); });
        return;
    }
    if this.phase == 1 {
        this.checks += [
            t_true(this.calls == 1 && this.failed == 1 && this.completed == 0,
                "a throwing predicate fails once without retry or completion"),
            t_true(string_is(this.reason) && this.reason.contains("predicate boundary rejected"),
                "the failure event retains the predicate diagnostic")
        ];
        this.phase = 2;
        this.calls = 0;
        this.failed = 0;
        this.task = sel([
            once(|m| { this.calls += 1; throw("selector boundary rejected"); }),
            once(|m| { this.fallback_calls += 1; })
        ]);
        return;
    }
    if this.phase == 2 {
        this.checks.push(t_true(this.calls == 1 && this.failed == 1
            && this.fallback_calls == 0 && this.completed == 0,
            "a selector cannot mask an exception or run another callback after it"));
        this.phase = 3;
        this.calls = 0;
        this.failed = 0;
        this.task = retry(3, once(|m| {
            this.calls += 1;
            throw("retry boundary rejected");
        }));
        return;
    }
    if this.phase == 3 {
        this.checks.push(t_true(this.calls == 1 && this.failed == 1 && this.completed == 0,
            "a retry decorator cannot repeat a callback exception"));
        this.phase = 4;
        this.task = once(|m| { this.recovered += 1; });
        return;
    }
    this.done = true;
    this.checks += [
        t_true(this.recovered == 1 && this.completed == 1,
            "an explicit replacement task completes once"),
        t_true(this.failed == 1,
            "replacement does not repeat the retired task failure")
    ];
    report_verdict(this.checks, "TASK CALLBACK FAILURE", "TASK_CALLBACK_FAILURE");
}
