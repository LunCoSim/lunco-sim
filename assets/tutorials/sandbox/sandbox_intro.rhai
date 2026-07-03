//! Sandbox Intro — a guided COACH-MARK tour (lunica-style), authored in rhai.
//!
//! Demonstrates the guided-tour kind: manual, step-by-step, advancing on the
//! coach card's Back / Next / Skip controls (which emit `cmd:TutorialNext` /
//! `cmd:TutorialBack` / `cmd:TutorialSkip` on the event bus). The step cursor
//! lives on `this.i`; `on_event` moves it and re-renders — this gives Back/jump
//! for free, reproducing the lunica tour's behaviour with no Rust.
//!
//! `steps()` is a FUNCTION (not a top-level `const`) so it's callable from other
//! functions — rhai named fns can't see script-level consts. `show(s, i)` takes
//! the steps as an argument for the same reason (`this` is bound only in the
//! directly-called hook, not in helpers it calls).

fn steps() {
    [
        #{ anchor: "",                      title: "Welcome to LunCoSim",   body: "A 60-second tour of the workspace. Use Next / Back below — or Skip anytime." },
        #{ anchor: "panel.center",          title: "The Viewport",          body: "Your 3D world: spacecraft, rovers, lunar terrain. Right-drag to look around, scroll to zoom." },
        #{ anchor: "panel.side_browser",    title: "Browser & Tutorials",   body: "Scenes, models, and the Tutorials launcher live here on the left." },
        #{ anchor: "panel.right_inspector", title: "Inspector",             body: "Select any object to view and edit its properties here." },
        #{ anchor: "panel.bottom",          title: "Console & Plots",       body: "Logs, live telemetry plots, and the spawn palette dock along the bottom." },
        #{ anchor: "",                      title: "You're set!",           body: "Next up: the First Drive lesson — take control of a rover and drive it." },
    ]
}

fn show(s, i) {
    let step = s[i];
    coach(i, s.len(), step.anchor, step.title, step.body);
}

fn on_start(me) {
    // One source: the tutorial script loads its own light backdrop so the
    // "Viewport" step has something to frame.
    load_scene("tutorials/sandbox/sandbox_intro.usda");
    this.i = 0;
    show(steps(), 0);
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext" { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("Tour complete!", "success");
        emit("MISSION_COMPLETE", 0);   // marks the lesson done + advances the `next:` chain (→ First Drive)
    } else {
        show(s, this.i);
    }
}
