// Parallel-lane lunar shooting range.
// The visible projectiles are USD time samples; this policy applies the
// solver-facing recoil to each rover and impact impulses to separate targets.

fn set_impulse(body, force_z, torque_x) {
    cmd("SetPorts", #{
        target: body,
        writes: [["force_z", force_z], ["torque_x", torque_x]],
        seq: 0,
        tick: 0,
    });
}

fn clear_impulse(body) {
    set_impulse(body, 0.0, 0.0);
}

fn in_pulse(t, start, duration) {
    t >= start && t < start + duration
}

fn on_start(me) {
    this.rover_a = find("/LunarRoverShootingRangeShort/GunnerA");
    this.rover_b = find("/LunarRoverShootingRangeShort/GunnerB");
    this.target_a = find("/LunarRoverShootingRangeShort/Targets/CannonTarget");
    this.target_b = find("/LunarRoverShootingRangeShort/Targets/MissileTarget");
    this.t = 0.0;
    this.finished = false;

    if this.rover_a < 0 || this.rover_b < 0 || this.target_a < 0 || this.target_b < 0 {
        print("[lunar-range] ERROR missing rover or target");
        this.finished = true;
        return;
    }

    brake(this.rover_a);
    brake(this.rover_b);
    clear_impulse(this.rover_a);
    clear_impulse(this.rover_b);
    clear_impulse(this.target_a);
    clear_impulse(this.target_b);
    seek_animation(0.0);
    play_animation();
    print("[lunar-range] READY: parallel lanes, 1.62 m/s^2, solver recoil and target impact enabled");
}

fn on_tick(me) {
    if this.finished { return; }
    this.t += dt();

    let rover_a_force = 0.0;
    let rover_a_torque = 0.0;
    let rover_b_force = 0.0;
    let rover_b_torque = 0.0;
    let target_a_force = 0.0;
    let target_a_torque = 0.0;
    let target_b_force = 0.0;
    let target_b_torque = 0.0;

    // Both weapons fire along -Z, so both rover reactions are +Z.
    if in_pulse(this.t, 1.50, 0.10) || in_pulse(this.t, 2.34, 0.10) {
        rover_a_force = 6200.0;
        rover_a_torque = 2200.0;
    }
    if in_pulse(this.t, 4.00, 0.16) {
        rover_b_force = 5200.0;
        rover_b_torque = -1800.0;
    }

    // Target impulses occur when each sampled projectile reaches its plate.
    if in_pulse(this.t, 1.78, 0.12) || in_pulse(this.t, 2.62, 0.12) {
        target_a_force = -1500.0;
        target_a_torque = 2800.0;
    }
    if in_pulse(this.t, 4.84, 0.18) {
        target_b_force = -2600.0;
        target_b_torque = 4600.0;
    }

    set_impulse(this.rover_a, rover_a_force, rover_a_torque);
    set_impulse(this.rover_b, rover_b_force, rover_b_torque);
    set_impulse(this.target_a, target_a_force, target_a_torque);
    set_impulse(this.target_b, target_b_force, target_b_torque);
    brake(this.rover_a);
    brake(this.rover_b);

    if this.t >= 7.75 {
        clear_impulse(this.rover_a);
        clear_impulse(this.rover_b);
        clear_impulse(this.target_a);
        clear_impulse(this.target_b);
        emit("LUNAR_ROVER_SHOOTING_RANGE_SHORT_COMPLETE", true);
        print("[lunar-range] COMPLETE");
        this.finished = true;
    }
}

fn on_stop(me) {
    if this.rover_a != () && this.rover_a >= 0 { brake(this.rover_a); clear_impulse(this.rover_a); }
    if this.rover_b != () && this.rover_b >= 0 { brake(this.rover_b); clear_impulse(this.rover_b); }
    if this.target_a != () && this.target_a >= 0 { clear_impulse(this.target_a); }
    if this.target_b != () && this.target_b >= 0 { clear_impulse(this.target_b); }
}