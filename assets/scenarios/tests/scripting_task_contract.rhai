// Production scene-test coverage for the authored Rhai task and mission
// contract. The test observes the same public event/task surface used by
// authored programs; it does not recreate the task cursor in Rust.

fn on_start(me) {
    this.elapsed = 0.0;
    this.go_sent = false;
    this.go2_sent = false;
    this.seq_a = 0;
    this.seq_b = 0;
    this.branch_a = 0;
    this.branch_b1 = 0;
    this.branch_b2 = 0;
    this.race = 0;
    this.repeated = 0;
    this.forever = 0;
    this.task_complete = false;
    this.mission_complete = false;
    this.done = false;
    subscribe("TASK_SEQ_A");
    subscribe("TASK_SEQ_B");
    subscribe("TASK_BRANCH_A");
    subscribe("TASK_BRANCH_B1");
    subscribe("TASK_BRANCH_B2");
    subscribe("TASK_RACE");
    subscribe("TASK_REPEAT");
    subscribe("TASK_FOREVER");
    subscribe("TASK_COMPLETE");
    subscribe("MISSION_COMPLETE");
}

fn task(me) {
    seq([
        once(|m| emit("TASK_SEQ_A", 1)),
        wait_for("TASK_GO"),
        once(|m| emit("TASK_SEQ_B", 1)),
        par_all([
            once(|m| emit("TASK_BRANCH_A", 1)),
            seq([
                once(|m| emit("TASK_BRANCH_B1", 1)),
                wait_for("TASK_GO_2"),
                once(|m| emit("TASK_BRANCH_B2", 1)),
            ]),
        ]),
        wait(0.01),
        par_race([
            wait_until(|m| false),
            once(|m| emit("TASK_RACE", 1)),
        ]),
        repeat(3, seq([
            wait(0.01),
            once(|m| emit("TASK_REPEAT", 1)),
        ])),
        forever(once(|m| emit("TASK_FOREVER", 1))),
    ])
}

fn mission(me) {
    [
        objective("sequence", #{ requires_event: "TASK_SEQ_B" }),
        objective("branches", #{
            requires: ["sequence"],
            requires_event: "TASK_BRANCH_B2",
        }),
        objective("complete", #{
            requires: ["branches"],
            done: |m| true,
        }),
    ]
}

fn on_event(me, evt) {
    if evt.name == "TASK_SEQ_A" { this.seq_a += 1; }
    if evt.name == "TASK_SEQ_B" { this.seq_b += 1; }
    if evt.name == "TASK_BRANCH_A" { this.branch_a += 1; }
    if evt.name == "TASK_BRANCH_B1" { this.branch_b1 += 1; }
    if evt.name == "TASK_BRANCH_B2" { this.branch_b2 += 1; }
    if evt.name == "TASK_RACE" { this.race += 1; }
    if evt.name == "TASK_REPEAT" { this.repeated += 1; }
    if evt.name == "TASK_FOREVER" { this.forever += 1; }
    if evt.name == "TASK_COMPLETE" { this.task_complete = true; }
    if evt.name == "MISSION_COMPLETE" { this.mission_complete = true; }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    if !this.go_sent {
        emit("TASK_GO", 1);
        this.go_sent = true;
    }
    if this.seq_b > 0 && !this.go2_sent {
        emit("TASK_GO_2", 1);
        this.go2_sent = true;
    }

    if this.mission_complete && this.forever >= 3 {
        this.done = true;
        report_verdict([
            t_true(this.seq_a == 1, "sequence action ran once"),
            t_true(this.seq_b == 1, "sequence advanced after wait_for"),
            t_true(this.branch_a == 1, "par_all first branch ran once"),
            t_true(this.branch_b1 == 1, "par_all second branch started"),
            t_true(this.branch_b2 == 1, "par_all waited for its second branch"),
            t_true(this.race == 1, "par_race completed on its winning branch"),
            t_true(this.repeated == 3, "repeat ran exactly three times"),
            t_true(this.forever >= 3, "forever kept running"),
            t_true(!this.task_complete, "forever did not report task completion"),
            t_true(this.mission_complete, "gated mission completed"),
        ], "SCRIPTING TASK CONTRACT", "SCRIPTING_TASK_CONTRACT");
        return;
    }

    if this.elapsed > 15.0 {
        this.done = true;
        report_verdict(["task/mission contract did not reach its public observations"],
            "SCRIPTING TASK CONTRACT", "SCRIPTING_TASK_CONTRACT");
    }
}

fn on_stop(me) {
    if !this.done {
        this.done = true;
        report_verdict(["task/mission contract stopped before its public observations"],
            "SCRIPTING TASK CONTRACT", "SCRIPTING_TASK_CONTRACT");
    }
}
