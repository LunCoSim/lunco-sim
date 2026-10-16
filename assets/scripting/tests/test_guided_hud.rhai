// Windowed production gate: run with scripts/api/run_rhai_test.sh in an owned session.
fn hud_card(id) {
    let state = cmd_data("InspectGuidedHud", #{});
    for card in state.cards { if card.hud == id { return card; } }
    throw("HUD card missing: " + id);
}

let initial = cmd_data("InspectGuidedHud", #{});
let checks = [];
hint("hud-gate-mission", "Mission controls");
objectives_hud("hud-gate-mission", [#{text: "Mission objective", state: "active"}]);
let bound = guided_hud_actions("hud-gate-mission", "runtime_ui", "on_action", [
    #{id: "test", label: "Test action", enabled: true}
]);
checks.push(t_true(bound.ok, "a registered hook binds the card action"));
hint("hud-gate-teaching", "Teaching guidance");
checks.push(t_true(hud_perspectives("hud-gate-teaching", []).ok, "teaching card explicitly allows all perspectives"));

for perspective in ["sandbox_view", "rover_build", "editor"] {
    cmd("ActivatePerspective", #{id: perspective});
    let mission = hud_card("hud-gate-mission");
    let teaching = hud_card("hud-gate-teaching");
    checks.push(t_true(mission.visible == (perspective == "sandbox_view"), "whole mission card is scoped in " + perspective));
    checks.push(t_true(mission.interactive == mission.visible, "hidden mission actions own no input in " + perspective));
    checks.push(t_true(teaching.visible, "teaching scope is independent in " + perspective));
    checks.push(t_true(mission.hint == "Mission controls" && mission.objectives != "" && mission.actions.len() == 1,
        "perspective changes preserve card content and bindings"));
}

checks.push(t_true(hud_perspectives("hud-gate-teaching", ["editor"]).ok, "explicit registered subset is admitted"));
checks.push(t_true(!hud_perspectives("hud-gate-teaching", ["missing-perspective"]).ok, "unknown perspective rejects"));
checks.push(t_true(hud_card("hud-gate-teaching").perspectives == ["editor"], "invalid scope preserves the admitted scope"));
checks.push(t_true(!guided_hud_actions("hud-gate-mission", "missing_tool", "apply", [
    #{id: "test", label: "Test action", enabled: true}
]).ok, "missing callback endpoint rejects"));
checks.push(t_true(hud_card("hud-gate-mission").actions.len() == 1, "invalid rebind preserves the admitted binding"));
cmd("ActivatePerspective", #{id: "rover_build"});
checks.push(t_true(!hud_card("hud-gate-teaching").visible, "explicit teaching subset hides outside its scope"));
cmd("ActivatePerspective", #{id: "sandbox_view"});
checks.push(t_true(hud_card("hud-gate-mission").visible, "mission card restores in View"));

hint("Caller-owned guidance");
cmd("ClearGuidedOverlay", #{});
checks.push(t_true(hud_card("hud-gate-mission").actions.len() == 1
    && hud_card("hud-gate-teaching").hint == "Teaching guidance",
    "clearing the caller's card preserves independently owned cards"));
let implicit_found = false;
for card in cmd_data("InspectGuidedHud", #{}).cards {
    if card.hint == "Caller-owned guidance" { implicit_found = true; }
}
checks.push(t_true(!implicit_found, "clearing retires the caller's implicit card"));

for id in ["hud-gate-mission", "hud-gate-teaching"] {
    clear_hint(id);
    clear_objectives(id);
    clear_guided_hud_actions(id);
}
if initial.active_perspective != () { cmd("ActivatePerspective", #{id: initial.active_perspective}); }
t_report(checks);
