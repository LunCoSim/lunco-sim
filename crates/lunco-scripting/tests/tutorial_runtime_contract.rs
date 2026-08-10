//! Bounded Rhai execution checks for the shipped tutorial hooks.
//!
//! Parsing every tutorial is necessary but not sufficient: a script can parse
//! while its first `on_start` call still references a missing host function, or
//! while the shared Back/Next/Goto/Skip contract panics on a real event.  This
//! test installs the small command/HUD seam that production supplies, starts
//! every bundled tutorial, and drives its navigation hook through one complete
//! step list.  It deliberately does not fake a USD/physics world; world facts
//! belong to the production API/runtime gates.  The check here is the
//! language-to-command contract and the bounded lifecycle of each tutorial.

use std::sync::{Arc, Mutex};

use rhai::{Dynamic, Engine, Map, Scope, AST};

fn runtime_engine(commands: Arc<Mutex<Vec<String>>>) -> Engine {
    let mut engine = Engine::new();
    lunco_scripting::rhai_limits::apply(&mut engine);

    let command_log = commands.clone();
    engine.register_fn("cmd", move |name: String, _params: Dynamic| -> Dynamic {
        command_log.lock().expect("command log poisoned").push(name);
        Dynamic::UNIT
    });
    let command_log = commands;
    engine.register_fn("cmd", move |name: String| -> Dynamic {
        command_log.lock().expect("command log poisoned").push(name);
        Dynamic::UNIT
    });
    engine.register_fn("emit", |_name: String, _data: Dynamic| {});
    engine.register_fn("notify", |_text: String| {});
    engine.register_fn("notify_kind", |_text: String, _kind: String| {});

    // The lifecycle hooks under test should not need a world query. These
    // stubs keep the test honest about that boundary: if an on_start hook
    // unexpectedly starts depending on scene state, it fails here instead of
    // silently pretending a fake world was valid.
    engine.register_fn("find", |_path: String| Dynamic::UNIT);
    engine.register_fn("controller", |_entity: Dynamic| Dynamic::UNIT);
    engine.register_fn("is_controlled", |_entity: Dynamic| false);
    engine.register_fn("is_unattended", || false);
    engine
}

fn combined_source(script: &str) -> String {
    let preludes = lunco_assets::scripting::prelude_files()
        .into_iter()
        .map(|(_, source)| source)
        .collect::<Vec<_>>()
        .join("\n");
    format!("{preludes}\n{script}")
}

fn has_function(ast: &AST, name: &str) -> bool {
    ast.iter_functions().any(|metadata| metadata.name == name)
}

fn event(name: &str, value: i64) -> Dynamic {
    let mut map = Map::new();
    map.insert("name".into(), Dynamic::from(name.to_owned()));
    map.insert("value".into(), Dynamic::from_int(value));
    map.into()
}

fn call_hook<A: rhai::FuncArgs>(
    engine: &Engine,
    scope: &mut Scope,
    ast: &AST,
    name: &str,
    args: A,
    this: &mut Dynamic,
) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
    let options = rhai::CallFnOptions::new()
        .rewind_scope(false)
        .bind_this_ptr(this);
    engine.call_fn_with_options(options, scope, ast, name, args)
}

#[test]
fn every_bundled_tutorial_starts_and_navigates_without_a_rhai_runtime_error() {
    let files = lunco_assets::tutorials::tutorial_files();
    assert!(!files.is_empty(), "no bundled tutorials found");

    for (path, source) in files {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let engine = runtime_engine(commands.clone());
        let ast = engine
            .compile(combined_source(&source))
            .unwrap_or_else(|error| panic!("{path}: tutorial does not compile: {error}"));
        let mut scope = Scope::new();
        let mut this = Dynamic::from_map(Map::new());

        if has_function(&ast, "on_start") {
            let _ = call_hook(
                &engine,
                &mut scope,
                &ast,
                "on_start",
                (Dynamic::from_int(0),),
                &mut this,
            )
            .unwrap_or_else(|error| panic!("{path}: on_start failed: {error}"));
        }

        // A guided tour publishes a finite `steps()` array.  Drive one full
        // pass plus one extra event to exercise its terminal branch without an
        // unbounded loop. Other mission scripts simply ignore these UI events.
        let step_count = if has_function(&ast, "steps") {
            engine
                .call_fn::<Dynamic>(&mut scope, &ast, "steps", ())
                .unwrap_or_else(|error| panic!("{path}: steps() failed: {error}"))
                .try_cast::<rhai::Array>()
                .map(|steps| steps.len())
                .unwrap_or(0)
        } else {
            0
        };

        if has_function(&ast, "on_event") {
            for _ in 0..=step_count {
                let _ = call_hook(
                    &engine,
                    &mut scope,
                    &ast,
                    "on_event",
                    (Dynamic::from_int(0), event("cmd:TutorialNext", 0)),
                    &mut this,
                )
                .unwrap_or_else(|error| panic!("{path}: TutorialNext failed: {error}"));
            }

            // Exercise the other navigation payloads as well. The event map
            // always includes `value`, matching the production telemetry shape.
            for (name, value) in [
                ("cmd:TutorialBack", 0),
                ("cmd:TutorialGoto", 0),
                ("cmd:TutorialSkip", 0),
            ] {
                let _ = call_hook(
                    &engine,
                    &mut scope,
                    &ast,
                    "on_event",
                    (Dynamic::from_int(0), event(name, value)),
                    &mut this,
                )
                .unwrap_or_else(|error| panic!("{path}: {name} failed: {error}"));
            }
        }

        // The explicit panel-oriented tutorials must actually issue the typed
        // focus intent. This catches content regressions where a spotlight is
        // authored for a panel that is not the active tab.
        if source.contains("cmd(\"FocusPanel\"") {
            let log = commands.lock().expect("command log poisoned");
            assert!(
                log.iter().any(|name| name == "FocusPanel"),
                "{path}: authored FocusPanel call never reached the command seam"
            );
        }
    }
}
