// Headless authored acceptance for the production lander plume source path.
// Rhai owns only the stimulus; Modelica owns engine activity and photometry,
// while USD owns the plume geometry, shader bindings, and light connections.

fn number_or_zero(value) {
    if value == () { 0.0 } else { value }
}

fn print_metrics(s) {
    print("[lander-plume-activity] active_activity=" + s.activity +
          " active_intensity=" + s.intensity + " full_length=" + s.full_length +
          " length=" + s.length + " length_fraction=" + s.length_fraction);
    print("[lander-plume-activity] momentum_flux=" + s.momentum_flux +
          " exit_dynamic_pressure=" + s.exit_dynamic_pressure +
          " rcs_activity=" + s.rcs_activity +
          " rcs_full_length=" + s.rcs_full_length +
          " rcs_length_fraction=" + s.rcs_length_fraction);
    print("[lander-plume-activity] zero_activity=" + s.zero_activity +
          " zero_intensity=" + s.zero_intensity + " zero_length=" + s.zero_length);
}

fn on_start(me) {
    this.lander = find("/LanderPlumeActivity/Lander");
    this.main = find("/LanderPlumeActivity/Lander/MainPropulsion");
    this.photometry = find("/LanderPlumeActivity/Lander/FlameCore/Photometry");
    this.rcs = find("/LanderPlumeActivity/Lander/AttitudePropulsion");
    this.done = false;
    this.elapsed = 0.0;
    this.subject_ok = this.lander >= 0 && this.main >= 0 && this.photometry >= 0
        && this.rcs >= 0;
    this.command_ok = false;
    this.activity = 0.0;
    this.intensity = 0.0;
    this.radius = 0.0;
    this.full_length = 0.0;
    this.length = 0.0;
    this.length_fraction = 0.0;
    this.momentum_flux = 0.0;
    this.exit_dynamic_pressure = 0.0;
    this.rcs_activity = 0.0;
    this.rcs_full_length = 0.0;
    this.rcs_length_fraction = 0.0;
    this.zero_command_ok = false;
    this.phase = 0;
    this.zero_activity = 1.0;
    this.zero_intensity = 1.0;
    this.zero_length = 1.0;
    if this.lander >= 0 {
        // This is a headless authored stimulus. Establish control ownership
        // without asking the presentation layer to resolve a LocalAvatar.
        let possession = cmd("PossessVessel", #{
            target: this.lander,
            bind_camera: false,
        });
        if possession.ok {
            let command = cmd("SetPorts", #{
                target: this.lander,
                writes: [["external_throttle", 1.0], ["yaw", 1.0]],
            });
            this.command_ok = command.ok;
        }
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.phase == 0 {
        if this.elapsed < 1.0 { return; }
        if this.main >= 0 {
            this.activity = get(this.main, "engine_activity");
        }
        if this.photometry >= 0 {
            this.intensity = get(this.photometry, "intensity");
            this.radius = get(this.photometry, "radius");
            this.full_length = get(this.photometry, "full_throttle_length_m");
            this.length = get(this.photometry, "length");
            this.length_fraction = get(this.photometry, "visual_length_fraction");
            this.momentum_flux = get(this.photometry, "momentum_flux_n");
            this.exit_dynamic_pressure = get(this.photometry, "exit_dynamic_pressure_pa");
        }
        let rcs_pos_activity = if this.rcs >= 0 {
            number_or_zero(get(this.rcs, "yaw_pos_a_activity"))
        } else { 0.0 };
        let rcs_neg_activity = if this.rcs >= 0 {
            number_or_zero(get(this.rcs, "yaw_neg_a_activity"))
        } else { 0.0 };
        let rcs_pos_length = if this.rcs >= 0 {
            number_or_zero(get(this.rcs, "yaw_pos_a_full_throttle_length_m"))
        } else { 0.0 };
        let rcs_neg_length = if this.rcs >= 0 {
            number_or_zero(get(this.rcs, "yaw_neg_a_full_throttle_length_m"))
        } else { 0.0 };
        let rcs_pos_fraction = if this.rcs >= 0 {
            number_or_zero(get(this.rcs, "yaw_pos_a_visual_length_fraction"))
        } else { 0.0 };
        let rcs_neg_fraction = if this.rcs >= 0 {
            number_or_zero(get(this.rcs, "yaw_neg_a_visual_length_fraction"))
        } else { 0.0 };
        this.rcs_activity = max(rcs_pos_activity, rcs_neg_activity);
        this.rcs_full_length = max(rcs_pos_length, rcs_neg_length);
        this.rcs_length_fraction = max(rcs_pos_fraction, rcs_neg_fraction);
        let zero = cmd("ReleaseControl", #{
            target: this.lander,
        });
        this.zero_command_ok = zero.ok;
        this.phase = 1;
        this.elapsed = 0.0;
        return;
    }

    // The propulsion network has authored pump/valve dynamics. Allow those
    // dynamics to settle before asserting the zero-thrust endpoint.
    if this.elapsed < 3.0 { return; }
    if this.main >= 0 {
        this.zero_activity = get(this.main, "engine_activity");
    }
    if this.photometry >= 0 {
        this.zero_intensity = get(this.photometry, "intensity");
        this.zero_length = get(this.photometry, "length");
    }
    this.done = true;
    print_metrics(this);
    let failures = [];
    if !this.subject_ok { failures.push("production plume topology did not materialize"); }
    if !this.command_ok { failures.push("full-throttle plume stimulus command was rejected"); }
    if !this.zero_command_ok { failures.push("zero-throttle plume stimulus command was rejected"); }
    if this.activity == () || this.activity <= 0.01 { failures.push("engine activity stayed at zero"); }
    if this.intensity == () || this.intensity <= 0.0 || this.radius == () || this.radius <= 0.0 {
        failures.push("active plume photometry did not become positive");
    }
    if this.full_length == () || !this.full_length.is_finite() || this.full_length <= 2.0
        || this.length == () || !this.length.is_finite() || this.length <= 2.0 {
        failures.push("derived plume length did not become longer than 2 m");
    }
    if this.length_fraction == () || !this.length_fraction.is_finite()
        || this.length_fraction < 0.75 || this.length_fraction > 1.0 {
        failures.push("derived plume length did not fit the shader envelope");
    }
    if this.momentum_flux == () || !this.momentum_flux.is_finite() || this.momentum_flux <= 1000.0
        || this.exit_dynamic_pressure == () || !this.exit_dynamic_pressure.is_finite()
        || this.exit_dynamic_pressure <= 100.0 {
        failures.push("propulsion momentum did not reach plume photometry");
    }
    if this.rcs_activity == () || !this.rcs_activity.is_finite() || this.rcs_activity <= 0.01
        || this.rcs_full_length == () || !this.rcs_full_length.is_finite()
        || this.rcs_full_length <= 0.2 || this.rcs_length_fraction == ()
        || !this.rcs_length_fraction.is_finite() || this.rcs_length_fraction <= 0.01 {
        failures.push("RCS command did not reach a derived plume length");
    }
    if this.zero_activity == () || !this.zero_activity.is_finite() || this.zero_activity > 0.01
        || this.zero_intensity == () || !this.zero_intensity.is_finite() || this.zero_intensity > 0.0
        || this.zero_length == () || !this.zero_length.is_finite() || this.zero_length > 0.0 {
        failures.push("zero throttle did not remove activity, light, and plume length");
    }
    report_verdict(failures, "LANDER PLUME ACTIVITY", "LANDER_PLUME_ACTIVITY");
}
