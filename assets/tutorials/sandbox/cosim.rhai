//! Cosim — when a Model flies physics. A coach-mark tour over the lander scene.
//!
//! The lander in `lander_test.usda` is CO-SIMULATED: a Modelica model
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
           body: "A program is a PRIM, like a UsdShade shader. The lander's flight-control system is inseparable from the airframe, so it applies `LunCoProgramAPI` in place: `uniform asset lunco:program:sourceAsset = @models/Lander.mo@` on the vessel prim itself. The model's `inputs:` (throttle, pitch, roll, yaw) ARE the vessel's control surface — the live command ports a session writes when it takes the stick. A program that is bolted on instead — a guidance law, a supervisory script — is a `def LunCoProgram` CHILD prim, so deleting the prim removes the behaviour." },

        #{ anchor: "",
           title: "The wire",
           body: "Native USD `connectionPaths` are derived into a `SimConnection` — a source output port linked to a target input port, the FMI/SSP pattern. Forces land as PendingForces on the body; the model's `piloted` port yields control to whoever possesses the vessel. No script steps the loop — it is pure wiring." },

        #{ anchor: "panel.bottom",
           title: "The model speaks in events",
           body: "A threshold crossing on a port is published as a named event. One `def LunCoPortEvent` child prim per rule: `lunco:event:port = \"m_prop\"`, `lunco:event:op = \"lt\"`, `lunco:event:threshold = 200`, `lunco:event:emit = \"lander_low_fuel\"` — so when propellant mass drops under 200, a `lander_low_fuel` event reaches scripts and the HUD. The lander declares a second one, `Depleted`, at 0.5. Watch for them in the Console below as the descent burns fuel." },

        #{ anchor: "",
           title: "Verify it from code",
           body: "Over the API, `cosim_status` snapshots every cosim entity and its live Modelica variables, and `read_ports` / `watch_ports` trace one signal over time. That is how you confirm a Modelica→physics chain is actually live — read the ports, never guess from the picture." },

        #{ anchor: "",
           title: "Your turn: a battery on a rover",
           body: "A whole subsystem, one model. A rover's electrical system is a CIRCUIT — a panel, a pack and the hub motors on a shared bus — and a circuit is acausal: the `LunCo.Electrical` components (`Pin` with a `flow` current) `connect()` to a node and Modelica writes Kirchhoff's law, so nothing counts or sums the loads. That is one model (`vessels/rovers/rucheyok/rucheyok_electrical.mo`), hence one `def LunCoProgram \"System\"` under a `def Scope \"Electrical\"`. Its `inputs:` are the circuit's parameters (battery capacity, panel area) plus its boundary — irradiance from the sun, each wheel's shaft speed from physics — and `outputs:soc` is the charge draining as you drive. Open it in the Design workspace to see the circuit you are flying." },
    ]
}

fn on_start(me) {
    // One source: the tutorial loads its own environment so the ideas have a
    // live, cosim-flown lander to look at. (Same load as lander_mission.rhai.)
    load_scene("lunco://scenes/sandbox/lander_test.usda");
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
