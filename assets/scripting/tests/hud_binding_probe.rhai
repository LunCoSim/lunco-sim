// Register this library in an isolated test Twin; its public hooks publish and
// exercise the same named card without keeping state outside the HUD owner.
fn configure(label) {
    hint("hud-binding-probe", "Ready: " + label);
    guided_hud_actions("hud-binding-probe", "hud_binding_probe", "on_action", [
        #{id: label, label: "Invoke " + label, enabled: true}
    ])
}

fn on_action(action) {
    let text = "Invoked: " + action;
    for card in cmd_data("InspectGuidedHud", #{}).cards {
        if card.hud == "hud-binding-probe" && card.hint.starts_with("Invoked:") {
            text = card.hint + "\n" + text;
        }
    }
    hint("hud-binding-probe", text)
}

// API drivers evaluate these assertions through RunRhai after pointer input
// and lifecycle operations. Rust does not enumerate or assert on test assets.
fn verify_state(label, owner, invoked) {
    let checks = [];
    let found = false;
    for card in cmd_data("InspectGuidedHud", #{}).cards {
        if card.hud == "hud-binding-probe" {
            found = true;
            checks.push(t_true(card.twin.to_string() == owner.to_string(), "callback retains its explicit Twin"));
            let expected = if invoked { "Invoked: " + label } else { "Ready: " + label };
            checks.push(t_true(card.hint == expected, "card retains the expected callback state"));
            checks.push(t_true(card.actions.len() == 1 && card.actions[0].id == label,
                "callback preserves its current action set"));
        }
    }
    checks.push(t_true(found, "callback card exists"));
    report_verdict(checks, "HUD callback", "HUD_CALLBACK")
}

fn verify_retired() {
    let found = false;
    for card in cmd_data("InspectGuidedHud", #{}).cards {
        if card.hud == "hud-binding-probe" { found = true; }
    }
    report_verdict([t_true(!found, "retired card is absent")], "HUD retirement", "HUD_RETIREMENT")
}

fn verify_teaching(author, expected) {
    let checks = [];
    let found = false;
    for card in cmd_data("InspectGuidedHud", #{}).cards {
        if card.kind == "script" && card.author.to_string() == author.to_string() {
            found = true;
            checks.push(t_true(card.perspectives == [] && card.visible && card.hint != "",
                "script-owned teaching guidance declares its cross-perspective scope"));
        }
    }
    checks.push(t_true(found == expected, "teaching card follows its script lifetime"));
    report_verdict(checks, "HUD teaching lifecycle", "HUD_TEACHING")
}
