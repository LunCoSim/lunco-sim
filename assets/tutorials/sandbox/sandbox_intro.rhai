//! Sandbox Onboarding Tour — guided COACH-MARK tour with live action progression.
//!
//! Employs interactive progression for actions that have a public event
//! contract (selection, spawn, possession, and release), while camera guidance
//! uses the shared Next control. The current avatar camera is free-flight; its
//! controls come from the authored keymap, not from a tutorial-owned binding.

fn steps() {
    [
        // 0
        #{ anchor: "",                      focus: (),             title: "Welcome to LunCoSim",   body: "This is the Sandbox: a live 3D world.\n\nYou will learn the camera, selection, spawning, and control handoff. Click Next to begin.", require: "cmd:TutorialNext" },
        // 1
        #{ anchor: "panel.center",          focus: (),             title: "Look around",            body: "Hold " + input_hint("look_button") + " and drag to look around. Use the mouse wheel to move along your view. Click Next after trying both.", require: "cmd:TutorialNext" },
        // 2
        #{ anchor: "panel.center",          focus: (),             title: "Move around",            body: "Move with " + input_hint("forward") + "/" + input_hint("backward") + ". Strafe with " + input_hint("left") + "/" + input_hint("right") + ". Adjust height with " + input_hint("yaw_left") + "/" + input_hint("yaw_right") + ". Click Next after moving around the rover.", require: "cmd:TutorialNext" },
        // 3
        #{ anchor: "panel.entity_list",     focus: "entity_list",     title: "Select an object",       body: "In Entities, click the Rover. The selected object becomes the subject of Inspector.", require: "cmd:SelectEntity" },
        // 4
        #{ anchor: "panel.spawn_palette",   focus: "spawn_palette",   title: "Spawn an object",        body: "Open Spawn in the right dock. Click a rover, prop, or structure, then place it in the scene. Cancel with " + input_hint("cancel") + ".", require: "cmd:SpawnEntity" },
        // 5
        #{ anchor: "panel.command_deck",    focus: "command_deck",    title: "Take control",           body: "Select the rover. Open Command Deck and click Possess to give it your driving inputs.", require: "cmd:PossessVessel" },
        // 6
        #{ anchor: "panel.command_deck",    focus: "command_deck",    title: "Release control",        body: "Press " + input_hint("cancel") + " to release the rover and return to free flight.", require: "cmd:ReleaseVessel" },
        // 7
        #{ anchor: "",                      focus: (),             title: "Find command help",       body: "Open Help to find command and automation guidance. Rhai, the HTTP API, MCP clients, and the UI use the same typed commands.", require: "cmd:TutorialNext" },
        // 8
        #{ anchor: "",                      focus: (),             title: "Choose a lesson",         body: "You know the basic controls. Open Tutorials and choose First Drive or Moon Driving Basics to practise rover driving.", require: "cmd:TutorialNext" },
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
