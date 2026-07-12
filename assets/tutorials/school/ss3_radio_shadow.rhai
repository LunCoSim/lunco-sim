//! SS3 — Radio-Shadow & Autonomy (Space School track).
//!
//! On the low centre ground, the terrain hides Earth (low on the E horizon), so
//! the link home drops: radio-shadow (task §3.2). There the team cannot
//! tele-operate — the rover must run autonomously until it climbs back into line
//! of sight. This lesson explains the hand-off and (in a debug build) demonstrates
//! it: the rover drives itself to the POI while the comms-degradation subsystem
//! flags the lost link.
//!
//! The gameplay gate is a COMPOSITION of existing systems, not new engine work:
//! `set_subsystem("comms-degradation", true)` for the HUD, plus a
//! `comms.link.connected` policy hook (register_hook) that reports the link down
//! inside the footprint so manual commands are refused. Wiring that hook to the
//! real radio-shadow footprint is the one NEEDS here.
//! NEEDS: radio-shadow footprint → `comms.link.connected` hook (roadmap P1.4).

fn steps() {
    [
        #{ anchor: "",             title: "The link drops",       body: "Earth sits low on the eastern horizon. As you descend toward the POI, the ground behind you rises and cuts the line to Earth — you enter radio-shadow. Watch the comms indicator." },
        #{ anchor: "panel.bottom", title: "No tele-op down here", body: "With the link down you cannot drive by hand — commands don't arrive. This is not a bug; it's the point. A real rover on the far side of a ridge is on its own." },
        #{ anchor: "panel.center", title: "Hand off to autonomy", body: "So before you lose the link, you hand the rover a plan: reach the POI, do its task, and drive back UP until the link returns. In a debug build you're watching it run that plan itself right now." },
        #{ anchor: "panel.center", title: "Climb back into link",  body: "Line of sight returns the moment the rover crests back out of the low ground. Notice the comms indicator recover as it climbs — control is yours again." },
        #{ anchor: "",             title: "Plan for the dark",     body: "Radio-shadow means a stretch you script rather than steer. In the scored run, time spent unable to act still counts — so a good route minimises the blackout. Next: put it all together and get scored." },
    ]
}

fn show(s, i) {
    let step = s[i];
    coach(i, s.len(), step.anchor, step.title, step.body);
}

fn on_start(me) {
    load_scene("tutorials/school/traverse.usda");
    this.i = 0;
    notify_kind("Space School 3/4 · Radio-Shadow & Autonomy", "info");
    show(steps(), 0);
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext" { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else { return; }

    if this.i >= s.len() {
        set_subsystem("comms-degradation", false);   // clear the flag on the way out
        end_tour();
        notify_kind("Autonomy understood — on to the Scored Run", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        show(s, this.i);
    }
}

// DEBUG demo — drive the rover autonomously into the radio-shadow floor and back,
// toggling comms-degradation by distance to the POI so the coach cards have
// something live to point at. Not gated as a mission; purely illustrative.
fn task(me) {
    if !is_debug() { return; }
    let rover = "/Traverse/Rover";
    let poi   = "/Traverse/POI";
    seq([
        wait(1.0),
        once(|m| cmd("PossessVessel", #{ target: find(rover) })),
        wait(0.5),
        // Descend toward the POI; flag the link lost once inside the shadow ring.
        step(|m| nav_to(find(rover), world_pos(find(poi)), 0.7, 80.0), |m| arrived(find(rover), world_pos(find(poi)), 80.0)),
        once(|m| { set_subsystem("comms-degradation", true); notify_kind("[demo] LINK LOST — radio-shadow, autonomy only", "warn"); }),
        step(|m| nav_to(find(rover), world_pos(find(poi)), 0.5, 8.0), |m| arrived(find(rover), world_pos(find(poi)), 8.0)),
        #{ act: |m| brake(find(rover)), secs: 1.5 },
        // Climb back out; link returns.
        step(|m| nav_to(find(rover), [-200.0, 1.0, -200.0], 0.7, 20.0), |m| arrived(find(rover), [-200.0, 1.0, -200.0], 20.0)),
        once(|m| { set_subsystem("comms-degradation", false); notify_kind("[demo] link restored", "info"); }),
    ])
}
