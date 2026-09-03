//! LunCoSim Authority Handoff — a real control-state lesson.
//!
//! The learner performs the two meaningful actions: take a rover from the
//! simulation and release it again. The checklist reads the authoritative
//! controller state, so a command aimed at the wrong entity cannot pass the
//! lesson merely because some other vessel was possessed.

fn human_drives(id) {
    if id < 0 { return false; }
    let role = controller(id);
    role != () && role != "AiAgent"
}

fn authority_taken(me) {
    human_drives(find("/FirstDrive/Rover"))
}

fn authority_taken_message(me) {
    focus_panel("command_deck");
    clear_spotlight();
    hint("You own the rover now. Confirm the Command Deck says Driving (you), then release control.");
    spotlight("panel.command_deck", "Review the live authority state before releasing control.");
}

fn authority_released(me) {
    clear_spotlight();
    notify_kind("Authority handoff complete — the rover is free again.", "success");
}

fn authority_is_released(me) {
    let rover = find("/FirstDrive/Rover");
    rover >= 0 && !is_controlled(rover)
}

fn authority_possess(me) {
    cmd("PossessVessel", #{ target: find("/FirstDrive/Rover") })
}

fn authority_release(me) {
    cmd("ReleaseVessel", #{ target: find("/FirstDrive/Avatar") })
}

fn on_start(me) {
    focus_panel("command_deck");
    hint("Select the rover, open Command Deck, and take control. Watch the status change before releasing it again.");
    spotlight("panel.command_deck", "The Command Deck is the authoritative handoff surface.");
    notify_kind("Tutorial: Authority Handoff", "info");
}

fn mission(me) {
    [
        objective("take_control", #{
            text: "Click the rover, then click Possess in the Inspector or Command Deck",
            done: Fn("authority_taken"),
            on_complete: Fn("authority_taken_message"),
        }),
        objective("release_control", #{
            text: "Click Release or use " + input_hint("cancel") + " to return control to the simulation",
            requires: ["take_control"],
            done: Fn("authority_is_released"),
            dwell: 0.2,
            on_complete: Fn("authority_released"),
        }),
    ]
}

fn on_event(me, evt) {
    if evt.name == "MISSION_COMPLETE" {
        hint("Authority handoff complete. Click Continue in the Tutorial complete dialog to begin Moon Driving Basics.");
        notify_kind("Authority Handoff complete — choose Continue for Moon Driving Basics", "success");
    }
}

// Headless verification performs the same possession/release state changes as
// the learner. The UI path remains manual because this branch is gated by the
// actual host mode, not by a debug build flag.
fn task(me) {
    if !is_unattended() { return; }
    seq([
        wait(1.0),
        once(|m| authority_possess(m)),
        wait(1.0),
        once(|m| authority_release(m)),
        wait(1.0),
    ])
}
