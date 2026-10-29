// Task presentation is authored here. Rust validates and materializes the plan.
fn has_panel(facts, id) {
    for panel in facts.panels { if panel.id == id { return true; } }
    false
}
fn canonical_plot(facts, slot) {
    for panel in facts.instance_panels {
        if panel.id == "modelica_plot" && panel.canonical_instance != () {
            let kind = panel.id;
            let instance = panel.canonical_instance;
            let tab = #{ kind: kind, instance: instance, slot: slot };
            return [tab];
        }
    }
    []
}
fn assemble(facts) {
    let result = [];
    if has_panel(facts, "workbench::viewport") {
        result.push(#{
            id: "sandbox_view", title: "◉ View", interaction: "simulation",
            description: "Observe and operate the running Twin. Click an input endpoint to take control. Right-drag to orbit and scroll to zoom. Selection, control and camera targets are separate.",
            scene_visible: true, restore: false,
        });
    }
    let live_scene = has_panel(facts, "workbench::viewport");
    let plots = canonical_plot(facts, if live_scene { "bottom" } else { "center" });
    if plots.len() > 0 && has_panel(facts, "telemetry_browser") {
        let analysis = #{
            id: "analysis", title: "↗ Analyze", interaction: "editor", revision: 1,
            scene_visible: true,
            description: "Compare signals in Graphs. Check or drag channels from Telemetry into the active plot. Select an entity to narrow the signal list; use Ports for current values and manual inputs. View → Analyze exposes the additional analysis tools.",
            left: ["telemetry_browser"], instances: plots,
        };
        if live_scene { analysis.center = ["workbench::viewport"]; }
        if has_panel(facts, "entity_list") { analysis.left_bottom = ["entity_list"]; }
        if has_panel(facts, "port_inspector") { analysis.right = ["port_inspector"]; }
        result.push(analysis);
    }
    if has_panel(facts, "spawn_palette") && has_panel(facts, "entity_list")
        && has_panel(facts, "sandbox_inspector") && has_panel(facts, "lunco.workbench.twin_browser")
        && has_panel(facts, "sandbox_environment") && has_panel(facts, "workbench::viewport") {
        result.push(#{
            id: "rover_build", title: "⚒ Build", interaction: "editor", revision: 4,
            description: "Compose the live scene from reusable assemblies. Browse Entities and Navigator sources, place objects with Spawn and edit their properties in Inspector. Use Editor for internal assembly parts and Analyze for live signals.",
            left: ["entity_list"], left_bottom: ["lunco.workbench.twin_browser"],
            center: ["workbench::viewport"],
            right: ["sandbox_inspector", "sandbox_environment"], right_bottom: ["spawn_palette"],
        });
    }
    if has_panel(facts, "usd_prim_tree") && has_panel(facts, "usd::viewport")
        && has_panel(facts, "lunco.workbench.twin_browser") && has_panel(facts, "sandbox_inspector") {
        let editor = #{
            id: "editor", title: "✎ Editor", interaction: "editor", revision: 6,
            description: "Open a document or choose an editing tool to begin. Prim and Navigator share the left tabs; Requirements sits beside Inspector, with change History below. The source document owns Save and Undo; the live scene remains a separate context.",
            left: ["lunco.workbench.twin_browser", "usd_prim_tree"],
            right: ["sandbox_inspector"],
        };
        if has_panel(facts, "sysml_requirements") { editor.right.push("sysml_requirements"); }
        if has_panel(facts, "sysml_requirement_details") { editor.right.push("sysml_requirement_details"); }
        if has_panel(facts, "modelica_journal") { editor.right_bottom = ["modelica_journal"]; }
        if has_panel(facts, "authoring_review") { editor.right.push("authoring_review"); }
        if has_panel(facts, "sandbox_environment") { editor.right.push("sandbox_environment"); }
        result.push(editor);
    }
    if has_panel(facts, "lunco.workbench.twin_browser") && "modelica_model_view" in facts.instance_kinds {
        let model = #{
            id: "modelica_analyze", title: "∿ Lunica", interaction: "editor", revision: 1,
            description: "Engineer continuous models. Open a Modelica source from Navigator; edit its diagram or equations, compile and run. Use Analyze for signal comparison. Compile and save use the commands shown in the menus and Keybindings settings.",
            left: ["lunco.workbench.twin_browser"],
        };
        if has_panel(facts, "modelica_welcome") { model.center = ["modelica_welcome"]; }
        if has_panel(facts, "workbench::viewport") {
            result.push(model);
        } else {
            let other_tasks = result;
            result = [model];
            for task in other_tasks { result.push(task); }
        }
    }
    if has_panel(facts, "tools_palette") && has_panel(facts, "workbench::viewport")
        && has_panel(facts, "sandbox_inspector") && has_panel(facts, "sandbox_environment")
        && has_panel(facts, "entity_list") {
        result.push(#{
            id: "terrain_sculpt", title: "Terrain", interaction: "editor", visible: false,
            description: "Arm a terrain brush in Tools and click the scene. Escape cancels the active brush. Edits rebuild the visuals and collider through their existing owners.",
            left: ["tools_palette"], center: ["workbench::viewport"],
            right: ["sandbox_inspector", "sandbox_environment", "entity_list"],
        });
    }
    if has_panel(facts, "input_keybindings") {
        result.push(#{
            id: "input_settings", title: "Keybindings", interaction: "editor", visible: false,
            description: "Edit global command shortcuts and inspect the active Twin's overrides. Conflicts are recorded in Recent Events.",
            center: ["input_keybindings"],
        });
    }
    result
}
