// Headless authored acceptance for the production lander plume source path.
// Rhai owns only the stimulus; Modelica owns engine activity and photometry,
// while USD owns the plume geometry, shader bindings, and light connections.

fn on_start(me) {
    this.lander = find("/LanderPlumeActivity/Lander");
    this.main = find("/LanderPlumeActivity/Lander/MainPropulsion");
    this.photometry = find("/LanderPlumeActivity/Lander/FlameCore/Photometry");
    this.done = false;
    this.elapsed = 0.0;
    this.subject_ok = this.lander >= 0 && this.main >= 0 && this.photometry >= 0;
    this.command_ok = false;
    this.activity = 0.0;
    this.intensity = 0.0;
    this.radius = 0.0;
    if this.lander >= 0 {
        let possession = cmd("PossessVessel", #{target: this.lander});
        if possession.ok {
            let command = cmd("SetPorts", #{
                target: this.lander,
                writes: [["external_throttle", 1.0]],
            });
            this.command_ok = command.ok;
        }
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 0.5 { return; }
    if this.main >= 0 {
        this.activity = get(this.main, "engine_activity");
    }
    if this.photometry >= 0 {
        this.intensity = get(this.photometry, "intensity");
        this.radius = get(this.photometry, "radius");
    }
    this.done = true;
    print("[lander-plume-activity] engine_activity=" + this.activity +
          " intensity=" + this.intensity + " radius=" + this.radius);
    if this.subject_ok && this.command_ok && this.activity > 0.01
        && this.intensity > 0.0 && this.radius > 0.0 {
        report_verdict([], "LANDER PLUME ACTIVITY", "LANDER_PLUME_ACTIVITY");
    } else {
        report_verdict(["production lander did not propagate the authored plume stimulus"],
                       "LANDER PLUME ACTIVITY", "LANDER_PLUME_ACTIVITY");
    }
}
