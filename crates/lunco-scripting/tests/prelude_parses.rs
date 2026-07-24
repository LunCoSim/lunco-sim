//! The rhai prelude must PARSE. Nothing else checks this.
//!
//! `cargo check` never sees these files — they are assets, loaded from disk at
//! runtime (embedded only as a fallback), which is exactly what makes them
//! editable without a rebuild. The cost of that is real: a syntax error in
//! `links.rhai` is invisible to the compiler and to every Rust test, and surfaces
//! as the whole prelude silently falling back to the embedded copy
//! (`compile_prelude`) — so a scenario runs against STALE routing helpers and
//! nobody is told.
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
    let files = lunco_assets::scripting::prelude_files();
    assert!(!files.is_empty(), "no prelude files found at all");

    for (stem, src) in &files {
        if let Err(e) = engine.compile(src.as_str()) {
            panic!("prelude '{stem}.rhai' does not parse: {e}");
        }
    }
}

/// The embedded fallback must parse too — it is what a disk-load failure lands on,
/// so a broken fallback turns a recoverable problem into a dead prelude.
#[test]
fn embedded_prelude_files_all_parse() {
    let engine = runtime_engine();
    for (stem, src) in lunco_assets::scripting::embedded_prelude_files() {
        if let Err(e) = engine.compile(src.as_str()) {
            panic!("embedded prelude '{stem}.rhai' does not parse: {e}");
        }
    }
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
/// A policy that does not compile is registered as *nothing*: the seam falls back
/// to its Rust built-in and the app runs, quietly, on rules nobody chose. That is
/// the worst failure mode a policy file has — no crash, no wrong answer, just an
/// authored decision that silently stopped applying.
#[test]
fn policy_files_all_parse() {
    let engine = runtime_engine();
    let files = lunco_assets::scripting::policies();
    assert!(!files.is_empty(), "no policy files found at all");

    for (stem, src) in &files {
        if let Err(e) = engine.compile(*src) {
            panic!("policy '{stem}.rhai' does not parse: {e}");
        }
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

    let (_, src) = lunco_assets::scripting::policies()
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
