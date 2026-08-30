// Graphics acceptance for the production lander plume presentation.
// Rhai owns only the stimulus and verdict; Modelica owns activity/photometry,
// while USD owns the plume geometry, material bindings, and camera.
const TEST_KIND = "graphics";

fn on_start(me) {
    this.lander = find("/LanderPlumeVisual/Lander");
    this.main = find("/LanderPlumeVisual/Lander/MainPropulsion");
    this.photometry = find("/LanderPlumeVisual/Lander/FlameCore/Photometry");
    this.elapsed = 0.0;
    this.done = false;
    this.command_ok = false;
    if this.lander >= 0 {
        let possession = cmd("PossessVessel", #{
            target: this.lander,
            bind_camera: false,
        });
        if possession.ok {
            // Keep the review camera independent from the control handoff,
            // then reassert its authored absolute pose through the public
            // camera command so the offscreen avatar clone has the same view
            // as the interactive review camera.
            let camera = cmd("SetCameraLookAt", #{
                eye: [14.0, 12.0, 20.0],
                target: [0.0, 8.0, 0.0],
            });
            let active = cmd("SetActiveCamera", #{name: "VisualCamera"});
            let command = cmd("SetPorts", #{
                target: this.lander,
                writes: [["external_throttle", 1.0]],
            });
            this.command_ok = camera.ok && active.ok && command.ok;
        }
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 1.0 { return; }
    this.done = true;
    let activity = if this.main >= 0 { get(this.main, "engine_activity") } else { 0.0 };
    let intensity = if this.photometry >= 0 { get(this.photometry, "intensity") } else { 0.0 };
    print("[lander-plume-visual] engine_activity=" + activity + " intensity=" + intensity);
    if this.command_ok && activity > 0.01 && intensity > 0.0 {
        report_verdict([], "LANDER PLUME VISUAL", "LANDER_PLUME_VISUAL");
    } else {
        report_verdict(["production lander plume stimulus did not become active"],
                       "LANDER PLUME VISUAL", "LANDER_PLUME_VISUAL");
    }
}
