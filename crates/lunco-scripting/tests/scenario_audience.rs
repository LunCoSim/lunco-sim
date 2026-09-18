//! An authored scenario driver must be gated on the AUDIENCE, never on the
//! build profile.
//!
//! The fact that answers "should this drive itself?" is whether anything can
//! receive input, i.e. whether a window exists. These tests pin that generic
//! resolution and its fail-safe default.

use lunco_scripting::scenario::scenario_audience;
use lunco_scripting_bridge_core::ScenarioAudience;

/// The load-bearing case: a windowed session has a student in it, so a lesson
/// must wait for them.
#[test]
fn a_window_means_a_human_is_watching() {
    assert_eq!(
        scenario_audience(true, None),
        ScenarioAudience::Attended,
        "with a window open a person can click Next, so lessons must NOT self-play"
    );
}

/// The CI case: nothing can click, so an authored driver has to carry the lesson or it
/// tests nothing.
#[test]
fn no_window_means_nobody_is_watching() {
    assert_eq!(scenario_audience(false, None), ScenarioAudience::Unattended);
}

#[test]
fn explicit_override_wins_over_window_presence() {
    assert_eq!(
        scenario_audience(true, Some("1")),
        ScenarioAudience::Unattended
    );
    assert_eq!(
        scenario_audience(false, Some("0")),
        ScenarioAudience::Attended
    );
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
