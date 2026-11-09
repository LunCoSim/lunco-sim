// @timing presentation
//! A short guided walkthrough of how a LunCoSim scene becomes a running model.

fn steps() {
    [
        #{ anchor: "panel.entity_list", focus: "entity_list", title: "Start with an authored scene", body: "USD describes the scene: prims, references, transforms, properties, physics schemas, and connections. Expand the training rover in Entities and select a wheel or motor to see it as a separate prim composed from a reusable asset." },
        #{ anchor: "panel.sandbox_inspector", focus: "sandbox_inspector", title: "Authored facts and live state", body: "Inspector shows the selected object's authored settings and projected runtime state. A composed USD stage describes the scene; it does not run a vehicle by itself." },
        #{ anchor: "", title: "Continuous behavior", body: "Modelica owns continuous equations such as motor, battery, and controller behavior. Its values and outputs are connected through authored model interfaces; use Lunica when you need to inspect or change those equations." },
        #{ anchor: "", title: "Motion and contact", body: "The physics solver advances rigid bodies, joints, wheel contact, and collisions. At the simulation communication boundary, physics state is exchanged with connected Modelica participants so their outputs can affect the next physical step." },
        #{ anchor: "", title: "Mission logic", body: "Rhai scenarios sequence actions and respond to semantic events such as a sensor arrival or touchdown. They describe what should happen next; continuous control math stays in Modelica, and scene structure stays in USD." },
        #{ anchor: "panel.telemetry_browser", focus: "telemetry_browser", title: "Observe, then act", body: "Select the rover, open Analyze → Telemetry, and plot a speed channel. Pause with the simulation-time control while you inspect. Then use First Drive to take control and reach the marker; use Model Meets Physics for a live lander connection." },
        #{ anchor: "", title: "The scene-to-simulation loop", body: "Author or compose USD → admit the scene → initialize its physics and Modelica participants → advance simulation steps → publish telemetry and events → let Rhai react. Finish this tour, then open Model Meets Physics for the lander example." },
    ]
}

fn on_start(me, ctx) {
    cmd("ActivatePerspective", #{ id: "analysis" });
    this.i = 0;
    coach_step(steps(), 0);
    notify_kind("Tutorial: How Simulation Works", "info");
}

fn on_event(me, evt, ctx) {
    if tutorials::navigation(evt, ctx) { return; }
    this = tutorials::tour_event(this, evt, steps(), ctx);
}
