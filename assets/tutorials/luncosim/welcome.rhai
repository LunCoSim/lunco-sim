//! LunCoSim Welcome — a guided coach-mark tour of the flagship simulator.
//!
//! The guided-tour kind (same pattern as `sandbox/sandbox_intro.rhai`): a
//! narrated slideshow that advances on the coach card's Back / Next / Skip
//! (which emit `cmd:TutorialNext` / `cmd:TutorialBack` / `cmd:TutorialSkip`).
//! The cursor lives on `this.i`. No scene is loaded — LunCoSim already boots
//! the live solar system (celestial bodies + ephemeris), so this tour just
//! frames what's already on screen. `steps()` is a fn (not a const) so helpers
//! can call it; `this` is bound only in the directly-called hook.

fn steps() {
    [
        #{ anchor: "", title: "Welcome to LunCoSim",   body: "The full mission simulator. You're looking at a live, ephemeris-accurate solar system rendered at true scale. Use Next / Back below — or Skip anytime." },
        #{ anchor: "", title: "The world is real",     body: "Earth, Moon, and the Sun are placed from real ephemeris data. Physics runs in f64 (double precision) — stable from millimetre parts to lunar orbits." },
        #{ anchor: "", title: "Look around",           body: "Right-drag to orbit the camera, scroll to zoom. The view boots focused on Earth." },
        #{ anchor: "", title: "Warp time",             body: "Press T to toggle slow-motion / time-warp. Watch the bodies move along their orbits." },
        #{ anchor: "", title: "Next: the controls",    body: "Up next — focus a body and take command of a vessel." },
    ]
}

fn show(s, i) {
    let step = s[i];
    coach(i, s.len(), step.anchor, step.title, step.body);
}

fn on_start(me) {
    this.i = 0;
    show(steps(), 0);
    notify_kind("Tutorial: Welcome to LunCoSim", "info");
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext" { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("Welcome tour complete!", "success");
        emit("MISSION_COMPLETE", 0);   // marks done + advances the `next:` chain (→ Controls)
    } else {
        show(s, this.i);
    }
}
