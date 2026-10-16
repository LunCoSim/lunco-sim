// Register hud_binding_probe in an isolated Twin, then run/stop this host.
// HUD_TEARDOWN and each deferred command report their own typed verdict.
hud_perspectives([]);

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: [], required_inputs: []}
}

fn on_start(me, ctx) {
    hint("Teaching lifecycle probe");
    hint("hud-teardown", "Stop this scenario");
    objectives_hud("hud-teardown", [#{text: "Retire the controls", state: "active"}]);
    let result = guided_hud_actions("hud-teardown", "runtime_ui", "on_action", [
        #{id: "probe", label: "Teardown probe", enabled: true}
    ]);
    if !result.ok { throw "HUD teardown probe could not bind its endpoint"; }
}

fn on_stop(me, ctx) {
    // Both dispatch paths must inherit Stop after this hook returns.
    let tool = cmd("RunRhaiToolHook", #{tool: "hud_binding_probe", hook: "verify_stopping_call", args: #{String: "tool"}});
    let snippet = cmd("RunRhai", #{code: "hud_binding_probe::verify_stopping_call(\"snippet\")"});
    print("HUD_STOP_REQUESTS " + tool.id + " " + snippet.id);
    let checks = [
        t_true(clear_hint("hud-teardown").ok, "stop clears existing hint"),
        t_true(clear_objectives("hud-teardown").ok, "stop clears existing objectives"),
        t_true(clear_guided_hud_actions("hud-teardown").ok, "stop retires callback lease"),
        t_true(clear_hint("hud-teardown-missing").ok, "clear of missing card is idempotent"),
        t_true(!hint("hud-teardown-missing", "Rejected").ok, "stop cannot create content"),
        t_true(!hud_perspectives("hud-teardown-missing", []).ok, "stop cannot create scope"),
        t_true(!guided_hud_actions("hud-teardown-missing", "runtime_ui", "on_action", [
            #{id: "probe", label: "Rejected", enabled: true}
        ]).ok, "stop cannot bind callbacks"),
        t_true(!open_context_menu([20.0, 80.0], []), "stop cannot open persistent menus")
    ];
    for card in cmd_data("InspectGuidedHud", #{}).cards {
        checks.push(t_true(card.hud != "hud-teardown-missing", "clears cannot recreate a missing card"));
        if card.hud == "hud-teardown" {
            checks.push(t_true(card.hint == "" && card.objectives == "" && card.actions.is_empty(),
                "stopped scene card has no content or input"));
        }
    }
    report_verdict(checks, "HUD teardown", "HUD_TEARDOWN");
}
