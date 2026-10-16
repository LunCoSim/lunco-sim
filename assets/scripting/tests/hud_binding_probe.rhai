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

fn verify_pointer_subscription(doc) {
    let request = #{doc_id: doc, interaction_id: "callback-probe",
        tool: "runtime_ui", hook: "on_action", context: #{probe: "initial"}};
    let registered = cmd("SetScenePointerMoveHook", request);
    let competing_request = request;
    competing_request.interaction_id = "other-probe";
    let competing = cmd("SetScenePointerMoveHook", competing_request);
    let stale_clear = cmd("ClearScenePointerMoveHook", #{doc_id: doc, interaction_id: "other-probe"});
    request.context = #{probe: "refreshed"};
    let refreshed = cmd("SetScenePointerMoveHook", request);
    let invalid_request = request;
    invalid_request.hook = "missing_hook";
    let invalid = cmd("SetScenePointerMoveHook", invalid_request);
    let cleared = cmd("ClearScenePointerMoveHook", #{doc_id: doc, interaction_id: "callback-probe"});
    let next = cmd("SetScenePointerMoveHook", competing_request);
    let next_clear = cmd("ClearScenePointerMoveHook", #{doc_id: doc, interaction_id: "other-probe"});
    let repeated_clear = cmd("ClearScenePointerMoveHook", #{doc_id: doc, interaction_id: "other-probe"});
    report_verdict([
        t_true(registered.ok, "a registered endpoint binds to an open document"),
        t_true(!competing.ok, "a competing interaction cannot replace its owner"),
        t_true(!stale_clear.ok, "stale cleanup cannot retire another interaction"),
        t_true(refreshed.ok, "the owning interaction can refresh its bound context"),
        t_true(!invalid.ok, "invalid endpoint replacement rejects"),
        t_true(cleared.ok, "owner cleanup succeeds after a rejected replacement"),
        t_true(next.ok, "cleanup admits a fresh interaction"),
        t_true(next_clear.ok && repeated_clear.ok, "cleanup is idempotent"),
    ], "Pointer callback lifecycle", "POINTER_CALLBACK")
}

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
    let pointer = cmd("SetScenePointerMoveHook", #{
        doc_id: 0, interaction_id: "stopped-" + label,
        tool: "hud_binding_probe", hook: "on_menu_action", context: #{},
    });
    let checks = [
        t_true(!pointer.ok && pointer.error.contains("scenario teardown"),
            "deferred stop rejects pointer subscriptions at the lifecycle boundary"),
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
    let negative_diagnostic = "";
    try { command_result(-1); } catch (error) {
        negative_diagnostic = error.to_string();
        negative_rejected = negative_diagnostic.starts_with("command_result:");
    }
    checks.push(t_true(negative_rejected, "negative command ids fail visibly without a crash: " + negative_diagnostic));
    for id in ids {
        let result = command_result(id);
        checks.push(t_true(result.ok && result.status == "applied"
            && result.data.stdout.contains("TESTS_OK 5"), "stopping request completed with its own verdict: " + result));
    }
    for card in cmd_data("InspectGuidedHud", #{}).cards {
        checks.push(t_true(!card.hud.starts_with("stopped-"), "deferred stop left no presentation"));
    }
    report_verdict(checks, "Deferred lifecycle completion", "HUD_DEFERRED_COMPLETION")
}
