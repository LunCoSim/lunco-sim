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

fn on_menu_action(args) { on_action(args.label) }

// The driver submits this after a rendered input frame. FIFO completion places
// subsequent verdicts after that frame's deferred interaction callbacks.
fn input_fence(context) { true }

fn publish_menu(label) {
    open_context_menu([500.0, 200.0], [#{
        label: "Invoke " + label, tool: "hud_binding_probe", hook: "on_menu_action",
        args: #{label: label}
    }])
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
            checks.push(t_true(card.hint == expected, "expected " + expected + ", observed " + card.hint));
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

// Called through both deferred queues by an on_stop hook. Scheduling must keep
// the stopping caller's phase rather than turning it into a new UI/REPL author.
fn verify_stopping_call(label) {
    let checks = [
        t_true(!hint("stopped-" + label, "Rejected").ok, "deferred stop cannot create content"),
        t_true(!hud_perspectives("stopped-" + label, []).ok, "deferred stop cannot create scope"),
        t_true(!guided_hud_actions("stopped-" + label, "hud_binding_probe", "on_action", [
            #{id: "probe", label: "Rejected", enabled: true}
        ]).ok, "deferred stop cannot bind callbacks"),
        t_true(!open_context_menu([500.0, 200.0], []), "deferred stop cannot open menus")
    ];
    if !report_verdict(checks, "Deferred stop " + label, "HUD_DEFERRED_STOP") {
        throw "deferred stop escaped its lifecycle context";
    }
}

// The isolated API driver binds this generic asset hook before changing the
// fixture's watched JSON file. No product policy or Rust test names fixtures.
fn menu_policy_source() {
    `fn contribution(label) {
        let arguments = #{label: label};
        let action = #{label: "Invoke " + label, tool: "hud_menu_binding_probe",
            hook: "on_menu_action", args: arguments};
        let menu = #{label: "Callbacks", items: [action]};
        #{menus: [menu]}
    }
    fn menus(event, ctx) {
        if event != "json_scope_changed" || ctx.scope != "application" { return #{ menus: [] }; }
        for asset in ctx.payload {
            if asset.asset_uri.ends_with("/callback-menu.json") {
                let entry = parse_json(asset.text);
                print("CALLBACK_MENU_PUBLISHED " + entry.label);
                return contribution(entry.label);
            }
        }
        #{ menus: [] }
    }`
}

fn verify_stopped_requests(ids) {
    let checks = [];
    let negative_rejected = false;
    try { command_result(-1); } catch (error) {
        negative_rejected = error.contains("non-negative command id");
    }
    checks.push(t_true(negative_rejected, "negative command ids fail visibly without a crash"));
    for id in ids {
        let result = command_result(id);
        checks.push(t_true(result.ok && result.status == "applied"
            && result.data.stdout.contains("TESTS_OK 4"), "stopping request completed with its own verdict: " + result));
    }
    for card in cmd_data("InspectGuidedHud", #{}).cards {
        checks.push(t_true(!card.hud.starts_with("stopped-"), "deferred stop left no presentation"));
    }
    report_verdict(checks, "Deferred lifecycle completion", "HUD_DEFERRED_COMPLETION")
}
