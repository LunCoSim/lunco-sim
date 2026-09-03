//! Cosim — when a Model flies physics. A coach-mark tour over the lander scene.
//!
//! The lander in `lander_ops.usda` is CO-SIMULATED: a Modelica model
//! (`models/Lander.mo`) holds the guidance law, and a `SimConnection` pipes its
//! force outputs to the Avian physics body, while physics feeds altitude and
//! velocity back in. This tour walks that loop — the one pattern behind every
//! LunCoSim vessel that "thinks" in Modelica and "moves" in physics.
//!
//! Coach-mark tour (advances on Next / Back / Skip). Its curriculum payload mounts
//! the lander scene so the ideas have a live example to point at. It does NOT
//! re-fly the mission — see the *Lander & Rover Mission* lesson for that. For the prose version, see
//! `docs/tutorials/03-cosim.md`.

fn steps() {
    [
        #{ anchor: "",
           title: "Model meets Physics",
           body: "A Modelica model computes the control law. Physics moves the body. Wiring them together is co-simulation.\n\nThis lander is a live example. Press Next to continue." },

        #{ anchor: "",
           title: "Two engines, one loop",
           body: "At each communication point, the model publishes outputs. SimConnections pass them to physics, and physics sends the next state sample back to the model.\n\nBetween points, the last output is held." },

        #{ anchor: "",
           title: "Where the model is attached",
           body: "Select the lander in the scene. Its flight-control `Scope` applies `LunCoProgramAPI` and points to `models/Lander.mo`.\n\nA separate guidance or supervisor `Scope` can be removed without changing the airframe." },

        #{ anchor: "",
           title: "The wire",
           body: "The USD `connectionPaths` become `SimConnection` links: model output to physics input, and physics output back to the model.\n\nForces become PendingForces on the body. The `piloted` port yields control to the current driver." },

        #{ anchor: "panel.telemetry_browser", focus: "telemetry_browser",
           title: "The model speaks in events",
           body: "Modelica publishes conditions such as `low_fuel` and `depleted` as outputs. `LunCoEvent` turns each output into a named event with a severity.\n\nOpen status or Telemetry to read the resulting state while the lander burns fuel." },

        #{ anchor: "",
           title: "Verify it from code",
           body: "Use `CosimStatus` to inspect the co-simulation graph. Use `ReadPorts` for live named values and `GetBrokenConnections` for dropped wires.\n\nRead typed state to verify the link; do not infer it from the picture." },

        #{ anchor: "",
           title: "Your turn: a battery on a rover",
           body: "For a rover battery, include the battery, motors, and panel in one explicit `CollectionAPI:components` collection on the rover root. The root owns the solver boundary; no domain-named child is required. Author `connectors:p.connect` wires in USD. If another solver needs battery state, connect its input directly to `</Rover/Battery.outputs:soc_out>` so the producer stays visible. The projector passes the graph to Modelica `connect()`, which solves the Pin voltage and current balance." },
    ]
}

fn on_start(me) {
    // The curriculum payload is mounted before this script starts, so the tour
    // points at a live cosim-flown lander without opening a scene itself.
    this.i = 0;
    coach_step(steps(), 0);
    notify_kind("Tutorial: Cosim — Model meets Physics", "info");
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext"      { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else if evt.name == "cmd:TutorialGoto" { this.i = evt.value; }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("Lesson complete!", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        coach_step(s, this.i);
    }
}
