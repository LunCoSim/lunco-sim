//! LunCoSim Controls — camera focus, vessels, and taking command.
//!
//! A guided coach-mark tour (advances on Next / Back / Skip). Follows the
//! Welcome tour in the onboarding chain. Pure narration — safe everywhere,
//! no dependency on specific scene entities.

fn steps() {
    [
        #{ anchor: "", title: "Focus a body",      body: "Select a celestial body (Earth, Moon) to orbit the camera around it. The camera frames it automatically." },
        #{ anchor: "", title: "Fly the camera",    body: "The avatar camera has several rigs — orbit a body, free-fly through space, or ride along a vessel. Right-drag + scroll to move." },
        #{ anchor: "", title: "Take command",      body: "Select a vessel and possess it to fly or drive it yourself. Release control and its autopilot takes back over — the same `piloted` handoff a real GNC uses." },
        #{ anchor: "", title: "Mission Control",   body: "The right-hand panel shows live mission telemetry — time, epoch, and vessel state, updating every frame." },
        #{ anchor: "", title: "You're set!",       body: "Explore the system, warp time, and take a vessel for a spin. Pick another lesson any time from the Tutorials panel." },
    ]
}

fn show(s, i) {
    let step = s[i];
    coach(i, s.len(), step.anchor, step.title, step.body);
}

fn on_start(me) {
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
        notify_kind("You're ready — welcome aboard 🚀", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        show(s, this.i);
    }
}
