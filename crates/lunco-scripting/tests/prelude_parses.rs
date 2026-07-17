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
/// **The limits must match `world_bridge.rs`.** A bare `Engine::new()` uses rhai's
/// default expression-depth caps, which are far below the `set_max_expr_depths(128,
/// 128)` the real engine sets — so it rejects prelude files that run perfectly well,
/// and the test becomes a false alarm rather than a check. A parse test is only worth
/// having if it parses under the same rules as production.
fn runtime_engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_max_expr_depths(128, 128);
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

    for f in ["links", "link_ids", "neighbours", "reachable", "link_path", "can_reach"] {
        assert!(
            defined.contains(&f.to_string()),
            "links.rhai must define `{f}` (doc 49 §5): {defined:?}"
        );
    }
}
