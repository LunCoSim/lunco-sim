//! A lesson's autopilot must be gated on the AUDIENCE, never on the build profile.
//!
//! Every tutorial that can play itself (`first_drive`, `build_base`,
//! `lander_mission`, …) carries a `task` behaviour guarded by one condition. That
//! guard used to be `is_debug()` → `cfg!(debug_assertions)`, which is true for
//! every `cargo run`: the lesson spawned its own base / drove its own rover,
//! emitted `MISSION_COMPLETE` seconds after starting and chained on to its
//! successor, so a student watched the whole curriculum play itself.
//!
//! The fact that actually answers "should this drive itself?" is whether anything
//! can receive a click, i.e. whether a window exists. These tests pin that
//! resolution and the fail-safe default.

use bevy::prelude::*;
use lunco_scripting::scenario::{resolve_scenario_audience, ScenarioAudience};

/// `LUNCO_SCENARIO_UNATTENDED` overrides the window check, so a set variable in
/// the ambient environment would decide these cases instead of the window.
fn env_override_is_set() -> bool {
    std::env::var("LUNCO_SCENARIO_UNATTENDED").is_ok()
}

fn resolve_with_windows(count: usize) -> ScenarioAudience {
    let mut app = App::new();
    app.init_resource::<ScenarioAudience>();
    for _ in 0..count {
        app.world_mut().spawn(Window::default());
    }
    app.add_systems(Startup, resolve_scenario_audience);
    app.update();
    *app.world().resource::<ScenarioAudience>()
}

/// The load-bearing case: a windowed session has a student in it, so a lesson
/// must wait for them.
#[test]
fn a_window_means_a_human_is_watching() {
    if env_override_is_set() {
        return;
    }
    assert_eq!(
        resolve_with_windows(1),
        ScenarioAudience::Attended,
        "with a window open a person can click Next, so lessons must NOT self-play"
    );
}

/// The CI case: nothing can click, so an autopilot has to carry the lesson or it
/// tests nothing.
#[test]
fn no_window_means_nobody_is_watching() {
    if env_override_is_set() {
        return;
    }
    assert_eq!(resolve_with_windows(0), ScenarioAudience::Unattended);
}

/// A world that never resolves the audience (a unit test, a plugin-less `World`)
/// has no window by construction — so the fail-safe is `Unattended`. An autopilot
/// that runs when it should not is visible; a lesson that silently refuses to run
/// in CI is a green test that tested nothing.
#[test]
fn the_default_is_unattended() {
    assert_eq!(ScenarioAudience::default(), ScenarioAudience::Unattended);
    assert!(ScenarioAudience::default().is_unattended());
}

/// No shipped lesson may reason about the build profile. `is_debug()` is gone
/// from the bridge, and rhai resolves verbs at CALL time — a lesson still calling
/// it would compile, ship, and fail at runtime only when its autopilot fires,
/// which is exactly the silent-lesson-failure mode. Catch it here instead.
#[test]
fn no_lesson_gates_on_the_build_profile() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/tutorials")
        .canonicalize()
        .expect("assets/tutorials");
    let mut offenders = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_some_and(|e| e == "rhai")
                && std::fs::read_to_string(&path)
                    .is_ok_and(|src| src.contains("is_debug(") || src.contains("debug_assertions"))
            {
                offenders.push(path.strip_prefix(&root).unwrap().to_path_buf());
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these lessons gate behaviour on the build profile — use is_unattended(): {offenders:?}"
    );
}
