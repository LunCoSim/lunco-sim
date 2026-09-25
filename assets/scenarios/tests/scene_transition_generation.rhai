// Production lifecycle acceptance: each same-scene restart receives a fresh
// committed generation, and Rhai re-enters only after that generation commits.
const TEST_KIND = "headless";

const VERDICT_CHANNEL = "SCENE_TRANSITION_GENERATION";

fn on_start(me, ctx) {
    let runtime = execution_context();
    let generation = runtime.generation.to_string();
    if generation == "1" || generation == "2" {
        let restart = cmd("RestartScene", #{ reset_document: false });
        if restart == () || restart.ok != true {
            report_verdict([t_true(false,
                "same-scene RestartScene was rejected at generation " + generation + ": " + restart.error,
            )], "SCENE TRANSITION GENERATION", VERDICT_CHANNEL);
        }
        return;
    }

    if generation == "3" {
        report_verdict([
            t_true(generation == "3",
                "Rhai did not receive the committed generation after two same-scene restarts"),
        ], "SCENE TRANSITION GENERATION", VERDICT_CHANNEL);
        return;
    }

    report_verdict([
        t_true(false, "expected committed scene generations 1, 2, and 3; got " + generation),
    ], "SCENE TRANSITION GENERATION", VERDICT_CHANNEL);
}
