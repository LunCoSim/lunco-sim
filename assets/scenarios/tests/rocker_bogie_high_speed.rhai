// Full-throttle straight-line regression for the physical rocker-bogie.
//
// This deliberately measures the chassis pose, not only distance. A rover can
// keep making forward progress while the solver alternates wheel contact and
// sends the body through a visible back-and-forth oscillation.

const SETTLE_S = 4.0;
const DRIVE_S = 12.0;
// Exclude throttle spin-up from the high-speed stability window. Measuring the
// whole drive would report acceleration as "jitter" and make the gate useless.
const STEADY_STATE_DELAY_S = 3.0;
const MIN_TRAVEL_M = 10.0;
const MAX_TILT_DEG = 12.0;
const MAX_REVERSE_TICKS = 0;
const MAX_SPEED_RIPPLE = 0.20;
const MAX_HEIGHT_RIPPLE_M = 0.025;

fn vdist(a, b) {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn on_start(me) {
    this.t = 0.0;
    this.phase = "settle";
    this.rover = find("/RockerBogieHighSpeed/Rover");
    this.prev = ();
    this.start = ();
    this.last_speed = ();
    this.direction = 0.0;
    this.travel = 0.0;
    this.reverse_ticks = 0;
    this.speed_sum = 0.0;
    this.speed_sq_sum = 0.0;
    this.speed_n = 0;
    this.speed_min = ();
    this.speed_max = ();
    this.height_min = ();
    this.height_max = ();
    this.max_tilt = 0.0;
    this.done = false;

    print("[rb-high-speed] full-throttle flat-ground regression started");
}

fn finish_failure(message) {
    this.done = true;
    fail_fast(message, "ROCKER BOGIE HIGH SPEED", "ROCKER_BOGIE_HIGH_SPEED");
}

fn on_tick(me) {
    if this.done { return; }
    if this.rover == () || this.rover < 0 {
        finish_failure("rocker-bogie rover was not composed");
        return;
    }

    this.t += dt();
    let p = world_pos(this.rover);
    if p == () {
        finish_failure("rocker-bogie chassis lost its world pose");
        return;
    }

    if this.phase == "settle" {
        brake(this.rover);
        if this.t >= SETTLE_S {
            this.phase = "drive";
            this.start = p;
            this.prev = p;
            print("[rb-high-speed] settled; starting 1.0 throttle at t=" + r2(this.t));
        }
        return;
    }

    if this.phase == "drive" {
        drive(this.rover, 1.0, 0.0);

        let d = vdist(p, this.prev);
        let v = d / dt();
        this.travel += d;
        if this.last_speed != () {
            let dv = (v - this.last_speed).abs();
            // Keep this value available in the log while speed ripple below
            // captures the sustained high-speed oscillation.
            if dv > 10.0 { print("[rb-high-speed] speed step=" + r2(dv)); }
        }
        this.last_speed = v;

        if this.direction == 0.0 && d > 0.001 {
            // The authored forward direction is the sign of the first real
            // displacement along the course. Once established, a reverse tick
            // is a literal back-and-forth motion, not a signed-world guess.
            let dz = p[2] - this.prev[2];
            if dz.abs() > 0.001 { this.direction = if dz > 0.0 { 1.0 } else { -1.0 }; }
        }
        if this.direction != 0.0 {
            let dz = (p[2] - this.prev[2]) * this.direction;
            if dz < -0.01 { this.reverse_ticks += 1; }
        }

        // The regression is about the sustained high-speed regime. Do not mix
        // the intentional acceleration transient into its ripple or chassis-bob
        // statistics.
        if this.t - SETTLE_S >= STEADY_STATE_DELAY_S {
            this.speed_sum += v;
            this.speed_sq_sum += v * v;
            this.speed_n += 1;
            if this.speed_min == () || v < this.speed_min { this.speed_min = v; }
            if this.speed_max == () || v > this.speed_max { this.speed_max = v; }
            if this.height_min == () || p[1] < this.height_min { this.height_min = p[1]; }
            if this.height_max == () || p[1] > this.height_max { this.height_max = p[1]; }
        }

        let tilt = tilt_deg(this.rover).abs();
        if tilt > this.max_tilt { this.max_tilt = tilt; }
        this.prev = p;

        if this.t >= SETTLE_S + DRIVE_S {
            this.phase = "done";
            brake(this.rover);
            verdict(#{
                start: this.start,
                end: p,
                travel: this.travel,
                reverse_ticks: this.reverse_ticks,
                speed_sum: this.speed_sum,
                speed_sq_sum: this.speed_sq_sum,
                speed_n: this.speed_n,
                speed_min: this.speed_min,
                speed_max: this.speed_max,
                height_min: this.height_min,
                height_max: this.height_max,
                max_tilt: this.max_tilt
            });
        }
    }
}

fn verdict(s) {
    let f = [];
    let net = vdist(s.end, s.start);
    let mean = s.speed_sum / s.speed_n;
    let variance = (s.speed_sq_sum / s.speed_n) - mean * mean;
    let speed_ripple = if mean > 0.01 { (s.speed_max - s.speed_min) / mean } else { 999.0 };
    let height_ripple = s.height_max - s.height_min;

    f.push(t_true(net > MIN_TRAVEL_M, "rover did not reach high speed; net travel=" + r2(net) + " m"));
    f.push(t_true(s.reverse_ticks <= MAX_REVERSE_TICKS,
        "chassis moved backward during straight full-throttle drive: " + s.reverse_ticks + " ticks"));
    f.push(t_true(speed_ripple < MAX_SPEED_RIPPLE,
        "high-speed velocity ripple=" + r2(speed_ripple * 100.0) + "%"));
    f.push(t_true(height_ripple < MAX_HEIGHT_RIPPLE_M,
        "high-speed chassis height ripple=" + r4(height_ripple) + " m"));
    f.push(t_true(s.max_tilt < MAX_TILT_DEG,
        "high-speed chassis tilt=" + r2(s.max_tilt) + " deg"));

    print("[rb-high-speed] net=" + r2(net) + " m travel=" + r2(s.travel) +
        " m mean_speed=" + r2(mean) + " m/s speed_ripple=" + r2(speed_ripple * 100.0) +
        "% height_ripple=" + r4(height_ripple) + " m max_tilt=" + r2(s.max_tilt) +
        " deg reverse_ticks=" + s.reverse_ticks + " variance=" + r4(variance));
    report_verdict(f, "ROCKER BOGIE HIGH SPEED", "ROCKER_BOGIE_HIGH_SPEED");
}
