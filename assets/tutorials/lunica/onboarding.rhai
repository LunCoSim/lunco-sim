//! Onboarding gate — the first-run policy, in rhai.
//!
//! This is the rhai reimplementation of the old `tour_seen` behaviour: decide,
//! IN SCRIPT, whether to auto-show the Overview tour on a first run, and remember
//! the decision so it never re-fires. The launcher attaches this once per process
//! at startup; it self-gates, so it's a no-op on every run after the first.
//!
//! State lives in the persisted `tour_seen` setting, reached via the scripting
//! settings verbs — `get_setting`/`set_setting("TutorialSeen.onboarded", ..)` —
//! so the "have I seen the tour?" reader/writer is now rhai, not Rust.

fn on_start(me) {
    // Already onboarded on a previous run? Nothing to do.
    if get_setting("TutorialSeen.onboarded") == true {
        return;
    }
    // First run: remember it (persists to settings.json under `tour_seen`), then
    // show the Overview. `LaunchTutorial` re-uses this same host, so the overview
    // scenario cleanly replaces this gate.
    set_setting("TutorialSeen.onboarded", true);
    cmd("LaunchTutorial", #{ id: "overview" });
}
