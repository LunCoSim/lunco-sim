//! Sandbox Onboarding Tour — guided COACH-MARK tour with live action progression.
//!
//! Employs interactive progression: steps listen for specific user actions
//! (camera cycle, flight locomotion, shift-select, spawn, possess, release)
//! and advance automatically, while retaining "Back" / "Skip" and fallback "Next"
//! controls.

fn steps() {
    [
        // 0
        #{ anchor: "",                      title: "Welcome to LunCoSim",   body: "Welcome! This is the Sandbox, a 3D digital twin of the solar system. Let's learn the basics: camera navigation, selecting, spawning, avatars, and agentic workflows. Click Next to begin.", require: "next" },
        // 1
        #{ anchor: "panel.center",          title: "Moving: Orbit & Zoom",  body: "Let's look around! In Avatar mode (orbit rig by default), hold RIGHT MOUSE BUTTON and drag to orbit the target, or scroll the MOUSE WHEEL to zoom. Click Next when done.", require: "next" },
        // 2
        #{ anchor: "panel.center",          title: "Moving: Panning",       body: "To pan the camera in Avatar mode (orbit rig), hold the MIDDLE MOUSE BUTTON (or Shift + Right Mouse Button) and drag. Select any object and press F to focus on it. Click Next.", require: "next" },
        // 3
        #{ anchor: "panel.center",          title: "Camera Cycling",        body: "You can cycle perspectives. Press the C key to switch camera views (Avatar, Follow, First-Person). Press C now!", require: "cycle" },
        // 4
        #{ anchor: "panel.center",          title: "Locomotion: Flight",    body: "In Avatar flight mode, RMB drag looks around, and WASD keys fly you through space (hold Shift to boost speed x10). Pan and Zoom do not apply here. Try flying with W, A, S, or D now!", require: "fly" },
        // 5
        #{ anchor: "panel.side_browser",    title: "Selecting Objects",     body: "Hold SHIFT and left-click the Rover (or any object) in the 3D viewport to select it (or click multiple to extend selection). Or click it in the Twin Browser (left panel). Hold Shift and select now!", require: "select" },
        // 6
        #{ anchor: "panel.bottom",          title: "Spawning Objects",      body: "Open the Spawn Palette tab in the bottom dock. Click on any catalogued rover, prop, or structure to spawn it. Spawn something now!", require: "spawn" },
        // 7
        #{ anchor: "panel.right_inspector", title: "What is the Avatar?",   body: "Your presence in the simulation is your Avatar. You can possess a vehicle (like a rover) by selecting it and pressing F (or clicking 'Possess' in the Inspector). Possess a vehicle now!", require: "possess" },
        // 8
        #{ anchor: "panel.right_inspector", title: "Releasing Control",     body: "This links your keyboard controls directly to it. Press BACKSPACE to release control and return to Avatar mode. Press Backspace now!", require: "release" },
        // 9
        #{ anchor: "panel.bottom",          title: "Agentic Workflows",     body: "LunCoSim is built for automation! You can write scripts using Rhai, run co-simulations, or connect external AI coding agents via the HTTP API and MCP tools. Click Next to complete.", require: "next" },
        // 10
        #{ anchor: "",                      title: "You're ready!",         body: "Now you know the layout, camera, and core concepts. Next up: S1 Moon Driving Basics — take control of a rover and feel a slope.", require: "next" },
    ]
}

fn show(s, i) {
    let step = s[i];
    coach(i, s.len(), step.anchor, step.title, step.body, step.require);
}

fn on_start(me) {
    load_scene("lunco://tutorials/sandbox/sandbox_intro.usda");
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
        emit("MISSION_COMPLETE", 0);
    } else {
        show(s, this.i);
    }
}
