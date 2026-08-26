//! Sandbox Onboarding Tour — guided COACH-MARK tour with live action progression.
//!
//! Employs interactive progression for actions that have a public event
//! contract (selection, spawn, possession, and release), while camera guidance
//! uses the shared Next control. The current avatar camera is free-flight; its
//! controls come from the authored keymap, not from a tutorial-owned binding.

fn steps() {
    [
        // 0
        #{ anchor: "",                      focus: (),             title: "Welcome to LunCoSim",   body: "Welcome! This is the Sandbox, a 3D digital twin of the solar system. Let's learn the basics: camera navigation, selecting, spawning, avatars, and agentic workflows. Click Next to begin.", require: "cmd:TutorialNext" },
        // 1
        #{ anchor: "panel.center",          focus: (),             title: "Free-flight camera",    body: "The lesson starts in the authored free-flight camera. Hold " + input_hint("look_button") + " and drag to look around; use the mouse wheel to dolly along your look direction. Click Next after trying both.", require: "cmd:TutorialNext" },
        // 2
        #{ anchor: "panel.center",          focus: (),             title: "Locomotion",            body: "Use the authored avatar keymap to move through the scene: " + input_hint("forward") + "/" + input_hint("backward") + " move forward and backward, " + input_hint("left") + "/" + input_hint("right") + " strafe, and " + input_hint("yaw_left") + "/" + input_hint("yaw_right") + " move down and up. Click Next after moving around the rover.", require: "cmd:TutorialNext" },
        // 3
        #{ anchor: "panel.side_browser",    focus: (),             title: "Selecting Objects",     body: "Select the Rover (or any object) in the Entities panel. In a build or object-builder view, Help shows the viewport-selection gesture. The selected object becomes the subject of the Inspector.", require: "cmd:SelectEntity" },
        // 4
        #{ anchor: "panel.right_inspector", focus: "spawn",          title: "Spawning Objects",      body: "Open Spawn in the right dock. Click a catalogued rover, prop, or structure, then place it in the scene. Cancel placement with " + input_hint("cancel") + ".", require: "cmd:SpawnEntity" },
        // 5
        #{ anchor: "panel.right_inspector", focus: (),             title: "Taking control",        body: "Select the rover, then click Possess in the Inspector or Command Deck. Possession grants the avatar's mapped driving inputs to that vessel.", require: "cmd:PossessVessel" },
        // 6
        #{ anchor: "panel.right_inspector", focus: (),             title: "Releasing control",      body: "Use " + input_hint("cancel") + " to release possession and return to free-flight. This is the shared Cancel intent, so it also closes an active placement interaction first.", require: "cmd:ReleaseVessel" },
        // 7
        #{ anchor: "",                      focus: (),             title: "Agentic workflows",      body: "Open Help for command and automation guidance. Rhai scenarios, the HTTP API, and MCP clients use the same typed command surface as the UI.", require: "cmd:TutorialNext" },
        // 8
        #{ anchor: "",                      focus: (),             title: "You're ready!",         body: "Now you know the current camera, input, selection, spawn, and authority surfaces. Choose First Drive or Moon Driving Basics from Tutorials to practise rover mechanics.", require: "cmd:TutorialNext" },
    ]
}

fn show(s, i) {
    coach_step(s, i);
}

fn on_start(me) {
    // The world this lesson teaches in is DECLARED in the curriculum layer
    // (a `payload` arc) and mounted by the launcher before this runs.
    this.i = 0;
    show(steps(), 0);
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else if this.i >= s.len() { return; }
    else {
        let required = s[this.i].require;
        let matched = if type_of(required) == "string" {
            evt.name == required
        } else {
            let found = false;
            for name in required {
                if evt.name == name { found = true; break; }
            }
            found
        };
        if !matched { return; }
        this.i += 1;
    }

    if this.i >= s.len() {
        end_tour();
        notify_kind("Tour complete!", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        show(s, this.i);
    }
}
