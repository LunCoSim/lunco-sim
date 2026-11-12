// Production lifecycle acceptance: runtime reset and source reload each receive
// fresh committed generations, and Rhai re-enters only after commit.
const TEST_KIND = "headless";

const VERDICT_CHANNEL = "SCENE_TRANSITION_GENERATION";

fn on_start(me, ctx) {
    let rejected = [];
    for invalid in ["scene.usda", "C:\\Twins\\scene.usda", "file:///scene.usda"] {
        let response = cmd("LoadScene", #{ path: invalid, root_prim: "" });
        rejected.push(t_true(response != () && response.ok == false,
            "LoadScene must terminally reject a native or unqualified address: " + invalid));
    }
    let runtime = execution_context();
    let generation = runtime.generation.to_string();
    if generation == "1" || generation == "3" {
        let reset = cmd("ResetSceneRuntime", #{});
        if reset == () || reset.ok != true {
            report_verdict([t_true(false,
                "same-source ResetSceneRuntime was rejected at generation " + generation + ": " + reset.error,
            )], "SCENE TRANSITION GENERATION", VERDICT_CHANNEL);
        }
        return;
    }

    if generation == "2" {
        let restart = cmd("RestartScene", #{ reset_document: false });
        if restart == () || restart.ok != true {
            report_verdict([t_true(false,
                "same-scene RestartScene was rejected at generation " + generation + ": " + restart.error,
            )], "SCENE TRANSITION GENERATION", VERDICT_CHANNEL);
        }
        return;
    }

    if generation == "4" {
        let scene = "lunco://scenes/tests/scene_transition_generation.usda";
        expect_scene_load_failure(scene, "not an existing absolute USD prim");
        rejected.push(t_true(generation == "4",
            "Rhai did not receive committed generations after runtime resets and source reload"));
        report_verdict(rejected, "SCENE TRANSITION GENERATION", VERDICT_CHANNEL);
        let accepted = cmd("LoadScene", #{ path: scene, root_prim: "/DoesNotExist" });
        if accepted == () || accepted.ok != true {
            report_verdict([t_true(false, "valid scene address must admit the explicit-root projection test")],
                "SCENE TRANSITION GENERATION", VERDICT_CHANNEL);
        }
        return;
    }

    report_verdict([
        t_true(false, "expected committed scene generations 1 through 4; got " + generation),
    ], "SCENE TRANSITION GENERATION", VERDICT_CHANNEL);
}
