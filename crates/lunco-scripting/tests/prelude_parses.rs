//! The rhai prelude must PARSE. Nothing else checks this.
//!
//! `cargo check` never sees these files — they are assets, loaded from disk at
//! runtime (embedded when no editable asset tree is available), which is exactly what makes them
//! editable without a rebuild. The cost of that is real: a syntax error in
//! `links.rhai` is invisible to the compiler and to every Rust test, and surfaces
//! as a startup failure with a useful file-local diagnostic. The runtime uses
//! the selected source set as authoritative rather than silently running stale
//! embedded routing helpers.
//!
//! A parse test is cheap and catches that. It does not (and cannot) check that the
//! host functions the prelude calls are registered — a call to a missing `query()`
//! is a runtime error, not a parse error — but a typo in a `while`, a stray brace,
//! or a bad map literal all land here.

use rhai::Engine;

/// An engine configured like the runtime's.
///
/// **The policy must match runtime.** A bare engine can accept a different language
/// than production, making this a false alarm rather than a check.
fn runtime_engine() -> Engine {
    let mut engine = Engine::new();
    lunco_scripting::rhai_limits::apply(&mut engine);
    engine
}

/// Every prelude file the runtime would load, compiled. A failure names the file.
#[test]
fn prelude_files_all_parse() {
    let engine = runtime_engine();
    let files = lunco_assets::scripting::prelude_files().expect("active prelude source");
    assert!(!files.is_empty(), "no prelude files found at all");

    for (stem, src) in &files {
        if let Err(e) = engine.compile(src.as_str()) {
            panic!("prelude '{stem}.rhai' does not parse: {e}");
        }
    }
}

/// The embedded source must parse too — it is the wasm and installed-build source
/// when no editable asset directory is present.
#[test]
fn embedded_prelude_files_all_parse() {
    let engine = runtime_engine();
    for (stem, src) in lunco_assets::scripting::embedded_prelude_files() {
        if let Err(e) = engine.compile(src.as_str()) {
            panic!("embedded prelude '{stem}.rhai' does not parse: {e}");
        }
    }
}

/// Every embedded tool library is part of the production scripting surface.
/// Tool files are namespaced and are otherwise outside the prelude parse gate,
/// so a syntax error here would otherwise be discovered only after startup.
#[test]
fn embedded_tool_libraries_all_parse() {
    let engine = runtime_engine();
    let tools = lunco_assets::scripting::tool_libraries();
    assert!(!tools.is_empty(), "no embedded tool libraries found");
    for (name, src) in tools {
        if let Err(e) = engine.compile(src) {
            panic!("embedded tool library '{name}' does not parse: {e}");
        }
    }
}

/// USD authoring has one namespaced Rhai surface. The old global helpers were
/// removed so agents cannot select between two wrappers for the same typed
/// command and journal path.
#[test]
fn usd_authoring_surface_is_namespaced() {
    let engine = runtime_engine();
    let (_, authoring) = lunco_assets::scripting::prelude_files()
        .expect("active prelude source")
        .into_iter()
        .find(|(stem, _)| stem == "authoring")
        .expect("authoring.rhai must be in the prelude");
    let authoring_ast = engine.compile(authoring).expect("authoring.rhai parses");
    let authoring_functions: Vec<_> = authoring_ast
        .iter_functions()
        .map(|function| function.name.to_string())
        .collect();
    assert!(
        authoring_functions.iter().all(|name| {
            !name.starts_with("usd_")
                && !name.starts_with("attach_")
                && !name.starts_with("program_")
                && name != "detach_component"
        }),
        "authoring.rhai must not expose global USD assembly helpers: {authoring_functions:?}"
    );

    let (_, assembly_edit) = lunco_assets::scripting::tool_libraries()
        .into_iter()
        .find(|(name, _)| *name == "assembly_edit")
        .expect("assembly_edit.rhai must be embedded");
    let assembly_edit_ast = engine
        .compile(assembly_edit)
        .expect("assembly_edit.rhai parses");
    let assembly_edit_functions: Vec<_> = assembly_edit_ast
        .iter_functions()
        .map(|function| function.name.to_string())
        .collect();
    for required in [
        "add_prim",
        "remove_prim",
        "move_prim",
        "transform",
        "attribute",
        "keyframe",
        "remove_keyframe",
        "relationship",
        "connection",
        "schema",
        "variant",
        "payload",
        "active",
        "batch",
        "attach_component",
        "detach_component",
        "attach_program",
        "program_input_connection",
        "program_input_default",
        "program_output",
    ] {
        assert!(
            assembly_edit_functions.iter().any(|name| name == required),
            "assembly_edit.rhai must define `{required}`"
        );
    }
}

#[test]
fn assembly_ui_templates_use_existing_surfaces_and_workflows() {
    let (_, source) = lunco_assets::scripting::tool_libraries()
        .into_iter()
        .find(|(name, _)| *name == "assembly_ui")
        .expect("assembly_ui.rhai must be embedded");
    let template_count = runtime_engine()
        .eval::<i64>(&format!(
            "{source}\npanel_templates(7, 3, \"@root@\").len()"
        ))
        .expect("assembly_ui templates must evaluate without command bindings");
    assert_eq!(template_count, 9);

    let engine = runtime_engine();
    for (role, expected_panel) in [
        ("browser", "lunco.workbench.twin_browser"),
        ("structure", "usd_prim_tree"),
        ("preview", "usd::viewport"),
        ("inspector", "sandbox_inspector"),
        ("connections", "usd_connection_canvas"),
        ("animation", "sandbox_environment"),
        ("mount", "sandbox_inspector"),
        ("review", "sandbox_inspector"),
    ] {
        let panel = engine
            .eval::<String>(&format!("{source}\npanel_id(\"{role}\")"))
            .expect("known assembly surface must resolve to a registered panel");
        assert_eq!(panel, expected_panel, "surface role {role}");
    }

    let persistence = engine
        .eval::<rhai::Map>(&format!(
            "{source}\npanel_template(\"persistence\", 7, 3, \"@root@\")"
        ))
        .expect("persistence must be represented as a workflow, not a fake panel");
    assert_eq!(
        persistence
            .get("kind")
            .expect("workflow kind")
            .clone_cast::<String>(),
        "workflow"
    );
    assert!(
        !persistence.contains_key("panel"),
        "persistence must use lifecycle commands rather than inventing a panel"
    );
}

#[test]
fn authored_timeline_requires_one_explicit_operation() {
    let source = lunco_assets::scripting::prelude_files()
        .expect("active prelude source")
        .into_iter()
        .map(|(_, source)| source)
        .collect::<Vec<_>>()
        .join("\n");
    let engine = runtime_engine();

    let valid = engine
        .eval::<i64>(&format!(
            "{source}\ncompile_timeline([#{{ wait: 1.0 }}]).len()"
        ))
        .expect("a single explicit timeline operation must lower");
    assert_eq!(valid, 1);

    for invalid in [
        "compile_timeline([#{}])",
        "compile_timeline([#{ wait: 1.0, emit: \"AMBIGUOUS\" }])",
    ] {
        let error = engine
            .eval::<rhai::Dynamic>(&format!("{source}\n{invalid}"))
            .expect_err("timeline operation ambiguity must be rejected");
        assert!(error.to_string().contains("operation word"), "{error}");
    }
}

#[test]
fn set_property_helper_uses_the_reflected_command_fields() {
    let (_, src) = lunco_assets::scripting::prelude_files()
        .expect("active prelude source")
        .into_iter()
        .find(|(stem, _)| stem == "control")
        .expect("control.rhai must be in the prelude");
    assert!(src.contains("entity_id: id"));
    assert!(src.contains("property: key"));
    assert!(!src.contains("target: id"));
    assert!(!src.contains("key: key"));
}

/// Every BUNDLED tutorial must parse.
///
/// Same blind spot as the prelude, sharper consequence: a tutorial is a rhai ASSET,
/// so a syntax error is invisible to `cargo check` and to every Rust test, and
/// surfaces only when a student launches that specific lesson and gets nothing.
///
/// **Scope — bundled only.** This enumerates `assets/tutorials/`, so it covers the
/// tracks this app ships and nothing else. A TWIN's curriculum (the Summer Space
/// School lives at `<twin>/sim/tutorials/`, outside this repo) is loaded at runtime
/// by `sync_twin_tutorials` and CANNOT be reached from here — including its
/// `teleop_policy.rhai`, which fails closed, so a parse error there would not
/// disable the tele-op refusal but make it refuse *everything*. Twin content needs
/// its own check in the twin; do not assume this test speaks for it.
#[test]
fn bundled_tutorial_scripts_all_parse() {
    let engine = runtime_engine();
    let files = lunco_assets::tutorials::tutorial_files();
    assert!(!files.is_empty(), "no tutorial scripts found at all");

    for (path, src) in &files {
        if let Err(e) = engine.compile(src.as_str()) {
            panic!("tutorial '{path}' does not parse: {e}");
        }
    }
}

/// The connectivity routing helpers, by name. These are the surface doc 49 promises
/// scripts and the school lessons call; renaming one without updating the callers is
/// a silent break (rhai resolves calls at runtime).
#[test]
fn links_prelude_exposes_the_routing_surface() {
    let (_, src) = lunco_assets::scripting::prelude_files()
        .expect("active prelude source")
        .into_iter()
        .find(|(stem, _)| stem == "links")
        .expect("links.rhai must be in the prelude");

    let ast = runtime_engine()
        .compile(src.as_str())
        .expect("links.rhai parses");
    let defined: Vec<String> = ast.iter_functions().map(|f| f.name.to_string()).collect();

    for f in [
        "links",
        "link_ids",
        "neighbours",
        "reachable",
        "link_path",
        "can_reach",
    ] {
        assert!(
            defined.contains(&f.to_string()),
            "links.rhai must define `{f}` (doc 49 §5): {defined:?}"
        );
    }
}

/// Every shipped policy must parse.
///
/// A policy that does not compile is registered as *nothing*: the app reports a
/// visible startup error and the selected policy cannot silently disappear.
#[test]
fn policy_files_all_parse() {
    let engine = runtime_engine();
    let files = lunco_assets::scripting::policy_files().expect("active policy source");
    assert!(!files.is_empty(), "no policy files found at all");

    for (stem, src) in &files {
        if let Err(e) = engine.compile(src.as_str()) {
            panic!("policy '{stem}.rhai' does not parse: {e}");
        }
    }
}

/// The direct-link policy is intentionally narrower than the generic geometry
/// kernel: rover endpoints may use Earth, base, or relay peers, rover-to-rover
/// remains on the separate Wi-Fi graph, and a failed geometry verdict must never
/// be reopened by the role rule.
#[test]
fn link_policy_allows_rover_station_or_relay() {
    let (_, src) = lunco_assets::scripting::policy_files()
        .expect("active policy source")
        .into_iter()
        .find(|(stem, _)| *stem == "link")
        .expect("link.rhai must be a shipped policy");
    let engine = runtime_engine();
    let ast = engine.compile(src).expect("link.rhai parses");

    let cases = [
        ("rover", "earth", true, true),
        ("earth", "rover", true, true),
        ("rover", "rover", true, false),
        ("rover", "earth", false, false),
        ("rover", "relay", true, true),
        ("relay", "rover", true, true),
        ("rover", "base", true, true),
        ("base", "rover", true, true),
        ("relay", "earth", true, true),
    ];
    for (class_a, class_b, builtin, expected) in cases {
        let mut ctx = rhai::Map::new();
        ctx.insert("class_a".into(), rhai::Dynamic::from(class_a));
        ctx.insert("class_b".into(), rhai::Dynamic::from(class_b));
        ctx.insert("builtin".into(), rhai::Dynamic::from_bool(builtin));
        let mut scope = rhai::Scope::new();
        let actual: bool = engine
            .call_fn(&mut scope, &ast, "link_connected", (ctx,))
            .unwrap_or_else(|e| panic!("link_connected({class_a}, {class_b}) failed: {e}"));
        assert_eq!(actual, expected, "{class_a}↔{class_b}, builtin={builtin}");
    }
}

/// The readiness policy and `lunco_readiness::Action::builtin` must AGREE.
///
/// They are two statements of one rule — the Rust one runs when scripting is
/// absent or the hook faults, the rhai one runs otherwise — so a scene must not
/// behave differently depending on which is in force. Nothing but this test
/// couples them: they are in different languages, in different crates, and a
/// change to either compiles perfectly well on its own.
#[test]
fn readiness_policy_agrees_with_the_engines_builtin() {
    use lunco_readiness::{kinds, Action, Subject};

    let (_, src) = lunco_assets::scripting::policy_files()
        .expect("active policy source")
        .into_iter()
        .find(|(stem, _)| *stem == "readiness")
        .expect("readiness.rhai must be a shipped policy");
    let engine = runtime_engine();
    let ast = engine.compile(src).expect("readiness.rhai parses");

    let entity = bevy::prelude::Entity::from_raw_u32(3).unwrap();
    let cases = [
        (kinds::SCENE_LOAD, Subject::World, 0.0),
        (kinds::SCENE_LOAD, Subject::World, Action::DEADLINE_S + 1.0),
        (kinds::PROGRAM_COMPILE, Subject::Entity(entity), 0.5),
        (kinds::PROGRAM_COMPILE, Subject::World, 0.5),
        (kinds::PARTICIPANT_INIT, Subject::Entity(entity), 2.0),
        (
            kinds::PARTICIPANT_INIT,
            Subject::Entity(entity),
            Action::DEADLINE_S,
        ),
        ("something_nobody_implemented", Subject::World, 0.0),
    ];

    for (kind, subject, elapsed) in cases {
        let mut ctx = rhai::Map::new();
        ctx.insert("kind".into(), rhai::Dynamic::from(kind.to_string()));
        ctx.insert(
            "subject".into(),
            rhai::Dynamic::from(
                match subject {
                    Subject::World => "world",
                    Subject::Entity(_) => "entity",
                }
                .to_string(),
            ),
        );
        ctx.insert("entity".into(), rhai::Dynamic::from_int(-1));
        ctx.insert("label".into(), rhai::Dynamic::from("x".to_string()));
        ctx.insert("elapsed_s".into(), rhai::Dynamic::from_float(elapsed));
        ctx.insert(
            "deadline_s".into(),
            rhai::Dynamic::from_float(Action::DEADLINE_S),
        );

        let mut scope = rhai::Scope::new();
        let answer: rhai::Dynamic = engine
            .call_fn(&mut scope, &ast, "readiness_action", (ctx,))
            .unwrap_or_else(|e| panic!("readiness_action({kind}, {subject:?}) failed: {e}"));
        let answer = answer
            .into_immutable_string()
            .expect("the policy must answer with a string");

        let scripted = Action::parse(&answer)
            .unwrap_or_else(|| panic!("policy returned '{answer}', not a known action"));
        let native = Action::builtin(kind, subject, elapsed);
        assert_eq!(
            scripted, native,
            "readiness.rhai and Action::builtin disagree for \
             kind={kind} subject={subject:?} elapsed={elapsed}"
        );
    }
}
