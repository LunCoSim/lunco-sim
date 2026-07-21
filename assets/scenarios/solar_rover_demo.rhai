// Solar Rover Autopilot & Flight Software
// drives the rover through the waypoint course, monitors its Modelica power system,
// and handles battery depletion by pausing to solar-charge.

fn wp_pos(i) { world_pos(find("/SolarRoverTest/RoverTarget" + i)) }

// The power PORTS (`soc_out`, `solar_charging`, `vehicle_throttle`) live on the
// PROGRAM prim that binds SolarRoverPower.mo — not on the rover body `me`.
// Reading them off `me` silently returned () and fell through to the "battery
// full" defaults, so the pack never drained, the cinematic's
// `wait_until(soc < 0.05)` never fired, and the demo hung after the last waypoint.
fn power() { find("/SolarRoverTest/SolarRover/PowerSubsystem") }

fn on_tick(me) {
    if this.active != true { return; }   // wait for mission start signal

    // 1. Read the battery State of Charge from our Modelica power program
    let soc = get(power(), "soc_out");
    if soc == () { soc = 1.0; } // Fallback to full if program isn't fully loaded

    // Read the solar charging current from the model
    let charging = get(power(), "solar_charging");
    if charging == () { charging = 0.0; }

    // Print battery status to console periodically
    if this.print_tick == () { this.print_tick = 0; }
    this.print_tick += 1;
    if this.print_tick % 30 == 0 {
        print("[Telemetry] Battery SOC: " + (soc * 100.0).to_string() + "%, Solar charging: " + charging.to_string() + " Amperes");
    }

    // 2. Handle battery states
    if this.dead_battery == true {
        // Battery is dead: apply brakes and wait to solar-charge
        brake(me);
        set(power(), "vehicle_throttle", 0.0); // No motor current draw

        // Charged to 20%? Ready to resume!
        if soc >= 0.20 {
            this.dead_battery = false;
            notify_kind("Power restored (SOC >= 20%). Resuming mission!", "success");
        }
        return;
    }

    // Is the battery depleted?
    if soc < 0.05 {
        this.dead_battery = true;
        brake(me);
        set(power(), "vehicle_throttle", 0.0);
        notify_kind("Warning: Battery depleted (< 5% SOC). Entering solar charging mode.", "warn");
        return;
    }

    // 3. Normal driving logic
    if this.i == () { this.i = 1; }
    if this.i > 3 {
        brake(me);
        set(power(), "vehicle_throttle", 0.0);
        return; // Course complete
    }

    let target = wp_pos(this.i);
    if target == () { return; }

    let speed = 0.8;
    let radius = 4.0;

    // Check if we arrived at current waypoint
    if arrived(me, target, radius) {
        brake(me);
        set(power(), "vehicle_throttle", 0.0);
        this.i += 1;
    } else {
        // Run navigation math
        let steer = steer_to(me, target);
        let p = world_pos(me);
        let f = world_forward(me);
        let fwd = speed;
        if p != () && f != () {
            let dot = vdot(vnorm(f), vnorm(vsub(target, p)));
            // Slow down during hard turns to pivot instead of slide
            fwd = speed * clamp(0.25 + 0.75 * dot, 0.25, 1.0);
        }
        
        // Command the wheels
        drive(me, fwd, steer);
        
        // Feed the throttle command back to the Modelica battery model to drain it!
        set(power(), "vehicle_throttle", fwd);
    }
}

fn on_event(me, evt) {
    if evt.name == "rover_deployed" {
        this.active = true;
        this.i = 1;
        this.dead_battery = false;
        notify_kind("Solar Rover autopilot engaged. Driving course.", "info");
    }
}
