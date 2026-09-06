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
    this.full_length = 0.0;
    this.length = 0.0;
    this.length_fraction = 0.0;
    this.momentum_flux = 0.0;
    this.exit_dynamic_pressure = 0.0;
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
    if this.photometry >= 0 {
        this.full_length = get(this.photometry, "full_throttle_length_m");
        this.length = get(this.photometry, "length");
        this.length_fraction = get(this.photometry, "visual_length_fraction");
        this.momentum_flux = get(this.photometry, "momentum_flux_n");
        this.exit_dynamic_pressure = get(this.photometry, "exit_dynamic_pressure_pa");
    }
    print("[lander-plume-visual] engine_activity=" + activity +
          " intensity=" + intensity + " full_length=" + this.full_length +
          " length=" + this.length + " length_fraction=" + this.length_fraction +
          " momentum_flux=" + this.momentum_flux +
          " exit_dynamic_pressure=" + this.exit_dynamic_pressure);
    let failures = [];
    if !this.command_ok { failures.push("production plume stimulus command was rejected"); }
    if activity == () || activity <= 0.01 { failures.push("engine activity stayed at zero"); }
    if intensity == () || intensity <= 0.0 { failures.push("plume photometry stayed dark"); }
    if this.full_length == () || !this.full_length.is_finite() || this.full_length <= 2.0 {
        failures.push("derived full-throttle plume length was not longer than 2 m");
    }
    if this.length == () || !this.length.is_finite() || this.length <= 2.0 {
        failures.push("live plume length did not reach the readable full-throttle range");
    }
    if this.length_fraction == () || !this.length_fraction.is_finite()
        || this.length_fraction < 0.75 || this.length_fraction > 1.0 {
        failures.push("derived length did not fit the authored shader envelope");
    }
    if this.momentum_flux == () || !this.momentum_flux.is_finite() || this.momentum_flux <= 1000.0
        || this.exit_dynamic_pressure == () || !this.exit_dynamic_pressure.is_finite()
        || this.exit_dynamic_pressure <= 100.0 {
        failures.push("propulsion momentum did not reach the plume model");
    }
    report_verdict(failures, "LANDER PLUME VISUAL", "LANDER_PLUME_VISUAL");
}
