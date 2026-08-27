// Production acceptance test for semantic free-flight speed boost.
//
// The test drives the avatar through the public SimulateIntent command, first
// with forward alone and then with speed_boost held. It measures the resulting
// authored camera pose, so a parser-only test or a multiplier unit test cannot
// pass while the binding/port/consumer path is broken.

const WARMUP_S = 1.0;
const MEASURE_S = 2.0;

fn planar_distance(a, b) {
    let dx = a[0] - b[0];
    let dz = a[2] - b[2];
    hypot(dx, dz)
}

fn fail(message) {
    report_verdict([message], "FREE FLIGHT SPEED BOOST", "FREE_FLIGHT_SPEED_BOOST");
}

fn on_start(me) {
    this.elapsed = 0.0;
    this.phase = "warmup";
    this.avatar = find("/FreeFlightSpeedBoost/Avatar");
    this.base_start = ();
    this.boost_start = ();
    this.base_distance = 0.0;
    this.boost_distance = 0.0;
    this.base_speed = 0.0;
    this.boost_speed = 0.0;
    this.done = false;

    if this.avatar < 0 {
        this.done = true;
        fail("free-flight avatar did not compose");
        return;
    }

    let result = cmd("SimulateIntent", #{
        intent: "forward",
        held: true,
        target: this.avatar,
    });
    if !result.ok {
        this.done = true;
        fail("could not hold semantic forward intent: " + result.error);
        return;
    }
    this.base_start = world_pos(this.avatar);
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    if this.phase == "warmup" && this.elapsed >= WARMUP_S {
        this.base_start = world_pos(this.avatar);
        this.phase = "base";
        return;
    }

    if this.phase == "base" && this.elapsed >= WARMUP_S + MEASURE_S {
        this.base_distance = planar_distance(this.base_start, world_pos(this.avatar));
        this.base_speed = this.base_distance / MEASURE_S;
        this.boost_start = world_pos(this.avatar);
        let result = cmd("SimulateIntent", #{
            intent: "speed_boost",
            held: true,
            target: this.avatar,
        });
        if !result.ok {
            this.done = true;
            fail("could not hold semantic speed_boost intent: " + result.error);
            return;
        }
        this.phase = "boost";
        return;
    }

    if this.phase == "boost" && this.elapsed >= WARMUP_S + 2.0 * MEASURE_S {
        this.boost_distance = planar_distance(this.boost_start, world_pos(this.avatar));
        this.boost_speed = this.boost_distance / MEASURE_S;
        this.phase = "release";
        return;
    }

    if this.phase == "release" {
        cmd("SimulateIntent", #{ intent: "speed_boost", held: false, target: this.avatar });
        cmd("SimulateIntent", #{ intent: "forward", held: false, target: this.avatar });

        let checks = [];
        checks.push(t_true(this.base_speed > 5.0,
            "normal forward intent did not move the free-flight avatar"));
        checks.push(t_true(this.boost_speed > this.base_speed * 5.0,
            "semantic speed_boost did not produce the expected tenfold speed"));
        checks.push(t_bounded(world_pos(this.avatar), 1000.0,
            "free-flight avatar left the bounded acceptance fixture"));
        print("[free-flight-speed-boost] base=" + r4(this.base_speed) +
            " m/s boost=" + r4(this.boost_speed) + " m/s");
        this.done = true;
        report_verdict(checks, "FREE FLIGHT SPEED BOOST", "FREE_FLIGHT_SPEED_BOOST");
    }
}
