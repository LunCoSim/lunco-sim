//! Cosim — when a Model flies physics. A coach-mark tour over the lander scene.
//!
//! The lander in `lander_ops.usda` is CO-SIMULATED: a Modelica model
//! (`models/Lander.mo`) holds the guidance law, and a `SimConnection` pipes its
//! force outputs to the Avian physics body, while physics feeds altitude and
//! velocity back in. This tour walks that loop — the one pattern behind every
//! LunCoSim vessel that "thinks" in Modelica and "moves" in physics.
//!
//! Coach-mark tour (advances on Next / Back / Skip). Loads the lander scene so the
//! ideas have a live example to point at. It does NOT re-fly the mission — see the
//! *Lander & Rover Mission* lesson for that. For the prose version, see
//! `docs/tutorials/03-cosim.md`.

fn steps() {
    [
        #{ anchor: "",
           title: "Model meets Physics",
           body: "Some vessels are flown by a Modelica model. The model computes the control law; physics moves the body. Wiring the two together is CO-SIMULATION — cosim. This lander is one. Next / Back below, Skip anytime." },

        #{ anchor: "",
           title: "Two engines, one loop",
           body: "Every fixed step the same round-trip runs (the FMI master algorithm): (1) the Modelica model emits outputs — forces, throttle; (2) a SimConnection pipes them to the physics body; (3) Avian integrates the body; (4) altitude & velocity feed back into the model. All of it in FixedUpdate, at one shared dt." },

        #{ anchor: "",
           title: "Where the model is attached",
           body: "A program is an applied capability. The lander's flight-control system is inseparable from the airframe, so the vessel applies `LunCoProgramAPI` and authors `info:sourceAsset = @models/Lander.mo@` in place. Its `inputs:` are the vessel's live control surface. A separable guidance law or supervisor is an ordinary `Scope` child applying the same API, so deleting that scope removes only the bolted-on behaviour." },

        #{ anchor: "",
           title: "The wire",
           body: "Native USD `connectionPaths` are derived into a `SimConnection` — a source output port linked to a target input port, the FMI/SSP pattern. Forces land as PendingForces on the body; the model's `piloted` port yields control to whoever possesses the vessel. No script steps the loop — it is pure wiring." },

        #{ anchor: "panel.bottom",
           title: "The model speaks in events",
           body: "Modelica publishes each physical condition as a 0/1 output. A `def LunCoEvent` connects `inputs:trigger` to that output and supplies `lunco:event:name` plus severity. The lander wires `low_fuel` and `depleted` this way, so scripts and the HUD react without polling or duplicating thresholds outside the model. Watch for those events in the Console below as the descent burns fuel." },

        #{ anchor: "",
           title: "Verify it from code",
           body: "Over the API, `cosim_status` snapshots every cosim entity and its live Modelica variables, and `read_ports` / `watch_ports` trace one signal over time. That is how you confirm a Modelica→physics chain is actually live — read the ports, never guess from the picture." },

        #{ anchor: "",
           title: "Your turn: a battery on a rover",
           body: "A whole subsystem, one compiled model. A rover's electrical system is an acausal circuit: each part applies `LunCoProgramAPI`, exposes its Modelica `Pin` as `connectors:*`, and ordinary USD connections assemble the bus. Runtime projection emits one Modelica wrapper for the enclosing `Electrical` Scope; Modelica `connect()` enforces Kirchhoff balance. The Scope exposes only causal boundary `inputs:` and `outputs:` such as drive demand and SoC." },
    ]
}

fn on_start(me) {
    // One source: the tutorial loads its own environment so the ideas have a
    // live, cosim-flown lander to look at. (Same load as lander_mission.rhai.)
    load_scene("lunco://scenes/sandbox/lander_ops.usda");
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
