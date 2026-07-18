//! Inspect the Simulation — a guided tour of reading live state (the inspect-simulation skill).
//!
//! Coach-mark tour. Teaches selection → inspector, telemetry ports and plots,
//! and reading it all back over the API without touching the log.

fn steps() {
    [
        #{ anchor: "",                      title: "Inspect the Simulation", body: "A running sim is fully observable — pose, telemetry ports, and Modelica variables, all live. Let's find them. Next / Back below." },
        #{ anchor: "panel.center",          title: "Select to inspect",      body: "Click any object in the viewport to select it. Everything about it becomes readable." },
        #{ anchor: "panel.right_inspector", title: "The Inspector",          body: "Pose, components, and editable properties for the selection show here — a live view that updates every frame." },
        #{ anchor: "panel.bottom",          title: "Telemetry & plots",      body: "Named ports (throttle, altitude, battery…) stream to the plots and console below. This is the same telemetry a ground station would read." },
        #{ anchor: "",                      title: "Read it from code",      body: "Over the API: list_entities → read_ports (filtered) for values, watch_ports for a time-series, cosim_status to verify a Modelica→physics chain. Read the ports — never poll the log." },
        #{ anchor: "",                      title: "You can see everything", body: "Selection + ports + a screenshot answer 'is it working?' without guessing. See the inspect-simulation skill for the full read surface." },
    ]
}

fn show(s, i) { let step = s[i]; coach(i, s.len(), step.anchor, step.title, step.body); }

fn on_start(me) {
    load_scene("lunco://tutorials/sandbox/first_drive.usda");
    this.i = 0;
    show(steps(), 0);
    notify_kind("Tutorial: Inspect the Simulation", "info");
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext" { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("Inspect the Simulation complete 🎉", "success");
        emit("MISSION_COMPLETE", 0);
    } else { show(s, this.i); }
}
