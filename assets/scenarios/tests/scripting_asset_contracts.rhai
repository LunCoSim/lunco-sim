// The scripting surface is authored content. This scene checks it through the
// same public catalog, query, and validation boundaries used by the editor and
// API; Rust keeps only the generic Rhai bridge and reflection mechanisms.

const TEST_KIND = "headless";

fn contains_name(entries, name) {
    for entry in entries {
        if entry.name == name { return true; }
    }
    false
}

fn contains_hook(entries, name) {
    for entry in entries {
        if entry.id == name { return true; }
    }
    false
}

fn contains_function(entries, name) {
    for entry in entries {
        if entry.name == name { return true; }
    }
    false
}

fn tool_has_function(tools, tool_name, function_name) {
    for tool in tools {
        if tool.name != tool_name { continue; }
        for function in tool.functions {
            if function.starts_with(function_name + "/") {
                return true;
            }
        }
    }
    false
}

fn validate_rhai(path, failures) {
    let report = query("ValidateAsset", #{ path: path });
    failures.push(t_true(report != (), path + " returned no validation report"));
    failures.push(t_true(report != () && report.kind == "rhai",
        path + " was not classified as Rhai"));
    failures.push(t_true(report != () && report.ok == true,
        path + " failed authored Rhai validation"));
}

fn on_start(me, ctx) {
    this.done = false;
    this.elapsed = 0.0;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 0.1 { return; }

    let failures = [];
    let catalog = query("ScriptingCatalog", #{});
    failures.push(t_true(catalog != (),
        "ScriptingCatalog did not publish the authored scripting surface"));

    for sample in [
        #{
            document: "twin://moonbase/scenes/main.usda",
            relative: "textures/albedo.png",
            expected: "twin://moonbase/textures/albedo.png",
        },
        #{
            document: "scenes/main.usda",
            relative: "textures/albedo.png",
            expected: "lunco://textures/albedo.png",
        },
    ] {
        let resolved = asset_source_relative_uri(sample.document, sample.relative);
        failures.push(t_true(resolved == sample.expected,
            "asset URI resolution changed source authority: " + sample.document));
    }

    if catalog != () {
        failures.push(t_true(catalog.verbs != ()
                && contains_name(catalog.verbs, "asset_source_relative_uri"),
            "ScriptingCatalog is missing asset_source_relative_uri"));
        let prelude = catalog.prelude;
        failures.push(t_true(prelude != () && prelude.len() > 0,
            "the active Rhai prelude is empty or failed to compile"));
        for function_name in [
            "compile_timeline", "point3", "report_verdict", "set_property", "links",
            "world_point"
        ] {
            failures.push(t_true(prelude != () && contains_function(prelude, function_name),
                "active prelude is missing " + function_name));
        }
        for function in prelude {
            failures.push(t_true(!function.name.starts_with("attach_")
                    && !function.name.starts_with("program_")
                    && function.name != "detach_component",
                "prelude leaked a global assembly helper: " + function.name));
        }

        let tools = catalog.tools;
        failures.push(t_true(contains_hook(catalog.policy_hooks, "scripting.source.classify"),
            "source classification policy hook is not reflected"));
        for tool_name in [
            "assembly_audit", "assembly_builder", "assembly_edit", "assembly_ui",
            "authoring_inspection", "model_authoring", "program_editor"
        ] {
            failures.push(t_true(contains_name(tools, tool_name),
                "tool library is not discoverable: " + tool_name));
        }
        failures.push(t_true(tool_has_function(tools, "assembly_builder",
                "lunar_albedo_material_plan"),
            "assembly_builder does not expose lunar_albedo_material_plan"));
        failures.push(t_true(tool_has_function(tools, "assembly_ui", "panel_templates"),
            "assembly_ui does not expose panel_templates"));

        let clock = clock_snapshot();
        failures.push(t_true(clock != () && clock.fixed_dt_s > 0.0 &&
                clock.deterministic_master == "sim_tick",
            "clock_snapshot did not expose the deterministic runtime spine"));

        failures.push(t_true(input_binding("forward") != () &&
                input_binding("look_button") != (),
            "the application input defaults were not loaded through the runtime asset pipeline"));

        let xs = [#{id: 1, v: 5}, #{id: 2, v: 2}, #{id: 3, v: 9}];
        let lo = min_by(xs, |e| e.v);
        let hi = max_by(xs, |e| e.v);
        let selected = count_where(xs, |e| e.v > 3);
        failures.push(t_true(lo.id == 2 && hi.id == 3 && selected == 2,
            "selection helpers did not preserve their authored closure contract"));

        let collision = #{name: "COLLISION_START", value: "10:20"};
        failures.push(t_true(collision_other(collision, 10) == 20 &&
                collision_other(collision, 20) == 10 && entered(collision, 10) &&
                !exited(collision, 10) && collision_other(collision, 99) == (),
            "collision helpers did not resolve the event participants"));

        let zone = #{name: "enter:pad_2", value: 42};
        failures.push(t_true(zone_of(zone) == "pad_2" && entered_zone(zone, "pad_2") &&
                !entered_zone(zone, "bay") && exited_zone(#{name: "exit:bay"}, "bay"),
            "zone helpers did not preserve event-name semantics"));

        let entities = list_entities();
        failures.push(t_true(entities != () && entities.len() > 0,
            "list_entities did not expose the authored scene"));

        let timeline = compile_timeline([#{ wait: 1.0 }]);
        failures.push(t_true(timeline != () && timeline.len() == 1,
            "the authored timeline policy did not lower one explicit operation"));

        let albedo = assembly_builder::lunar_albedo_material_plan(
            "runtime", "/Looks/Terrain/Surface",
            "twin://terrain/materials/textures/albedo.png",
            "twin://terrain/materials/textures/normal.png", 1.0, 0.5);
        failures.push(t_true(albedo != () && albedo.ok == true,
            "assembly_builder albedo policy did not evaluate"));
        let panels = assembly_ui::panel_templates(7, 3, "@root@");
        failures.push(t_true(panels != () && panels.len() == 9,
            "assembly_ui panel_templates did not evaluate"));
    }

    for path in [
        "lunco://scripting/prelude/_intro.rhai",
        "lunco://scripting/prelude/authoring.rhai",
        "lunco://scripting/prelude/auto_tests.rhai",
        "lunco://scripting/prelude/control.rhai",
        "lunco://scripting/prelude/coordinates.rhai",
        "lunco://scripting/prelude/links.rhai",
        "lunco://scripting/prelude/math.rhai",
        "lunco://scripting/prelude/mission.rhai",
        "lunco://scripting/prelude/nav.rhai",
        "lunco://scripting/prelude/readiness.rhai",
        "lunco://scripting/prelude/tasks.rhai",
        "lunco://scripting/prelude/terrain.rhai",
        "lunco://scripting/prelude/vessel.rhai",
        "lunco://scripting/policy/lint_modelica.rhai",
        "lunco://scripting/policy/lint_rhai.rhai",
        "lunco://scripting/policy/lint_twin.rhai",
        "lunco://scripting/policy/lint_usd.rhai",
        "lunco://scripting/policy/source_classification.rhai"
    ] {
        validate_rhai(path, failures);
    }

    let schema = query("ValidateAsset", #{
        path: "lunco://schemas/lunco/generatedSchema.usda"
    });
    failures.push(t_true(schema != () && schema.kind == "usd" && schema.ok == true,
        "generated USD schema was not available through the runtime asset pipeline"));

    this.done = true;
    report_verdict(failures, "SCRIPTING ASSET CONTRACTS", "SCRIPTING_ASSET_CONTRACTS");
}

fn on_stop(me, ctx) {
    if !this.done {
        this.done = true;
        report_verdict(["scripting asset contract stopped before validation"],
            "SCRIPTING ASSET CONTRACTS", "SCRIPTING_ASSET_CONTRACTS");
    }
}
