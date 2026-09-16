//! An authored scenario driver must be gated on the AUDIENCE, never on the
//! build profile.
//!
//! The fact that answers "should this drive itself?" is whether anything can
//! receive input, i.e. whether a window exists. These tests pin that generic
//! resolution and its fail-safe default.

use bevy::prelude::*;
use lunco_scripting::scenario::resolve_scenario_audience;
use lunco_scripting_bridge_core::ScenarioAudience;

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

/// The CI case: nothing can click, so an authored driver has to carry the lesson or it
/// tests nothing.
#[test]
fn no_window_means_nobody_is_watching() {
    if env_override_is_set() {
        return;
    }
    assert_eq!(resolve_with_windows(0), ScenarioAudience::Unattended);
}

/// A world that never resolves the audience (a unit test, a plugin-less `World`)
/// has no window by construction — so the fail-safe is `Unattended`. An authored driver
/// that runs when it should not is visible; a lesson that silently refuses to run
/// in CI is a green test that tested nothing.
#[test]
fn the_default_is_unattended() {
    assert_eq!(ScenarioAudience::default(), ScenarioAudience::Unattended);
    assert!(ScenarioAudience::default().is_unattended());
}
