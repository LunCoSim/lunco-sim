// Production acceptance test for semantic free-flight speed boost.
//
// The test drives the avatar through the public SimulateIntent command, first
// with forward alone and then with speed_boost held beside Q/E. It measures the
// resulting authored camera pose, so a parser-only test or a multiplier unit
// test cannot pass while the binding/port/consumer path is broken.

const WARMUP_S = 1.0;
const MEASURE_S = 2.0;

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["CausalTrace"], required_inputs: [] }
}

fn planar_distance(a, b) {
    let dx = a[0] - b[0];
    let dz = a[2] - b[2];
    hypot(dx, dz)
}

fn vertical_distance(a, b) {
    abs(a[1] - b[1])
}

fn fail(message) {
    report_verdict([message], "FREE FLIGHT SPEED BOOST", "FREE_FLIGHT_SPEED_BOOST");
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.phase = "warmup";
    this.avatar = find("/FreeFlightSpeedBoost/Avatar");
    this.base_start = ();
    this.diagonal_start = ();
    this.boost_start = ();
    this.base_distance = 0.0;
    this.base_speed = 0.0;
    this.diagonal_distance = 0.0;
    this.diagonal_speed = 0.0;
    this.boost_down_distance = 0.0;
    this.boost_up_distance = 0.0;
    this.edge_seen = false;
    this.edge_sent = false;
    this.edge_correlation = ();
    this.edge_event_correlation = ();
    this.verdict_ok = ();
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

fn on_event(me, evt, ctx) {
    if evt.name == "intent.edge" && evt.source == this.avatar {
        let value = evt.value;
        if value["intent"] == "release" && value["edge"] == "pulse" &&
           value["target_gid"].to_string() == this.avatar.to_string() {
            this.edge_seen = true;
            this.edge_event_correlation = value["correlation_id"];
        }
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    if !this.edge_sent {
        let edge = intent_pulse(this.avatar, "release");
        if !edge.ok {
            this.done = true;
            fail("could not emit atomic semantic intent edge: " + edge.error);
            return;
        }
        this.edge_correlation = edge.id;
        this.edge_sent = true;
    }
    this.elapsed += dt();

    if this.phase == "warmup" && this.elapsed >= WARMUP_S {
        this.base_start = world_pos(this.avatar);
        this.phase = "base";
        return;
    }

    if this.phase == "base" && this.elapsed >= WARMUP_S + MEASURE_S {
        this.base_distance = planar_distance(this.base_start, world_pos(this.avatar));
        this.base_speed = this.base_distance / MEASURE_S;
        let result = cmd("SimulateIntent", #{
            intent: "right",
            held: true,
            target: this.avatar,
        });
        if !result.ok {
            this.done = true;
            fail("could not hold semantic right intent: " + result.error);
            return;
        }
        this.diagonal_start = world_pos(this.avatar);
        this.phase = "diagonal";
        return;
    }

    if this.phase == "diagonal" && this.elapsed >= WARMUP_S + MEASURE_S * 2.0 {
        this.diagonal_distance = planar_distance(this.diagonal_start, world_pos(this.avatar));
        this.diagonal_speed = this.diagonal_distance / MEASURE_S;
        cmd("SimulateIntent", #{ intent: "right", held: false, target: this.avatar });
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
        result = cmd("SimulateIntent", #{
            intent: "yaw_left",
            held: true,
            target: this.avatar,
        });
        if !result.ok {
            this.done = true;
            fail("could not hold semantic yaw_left intent: " + result.error);
            return;
        }
        this.boost_start = world_pos(this.avatar);
        this.phase = "boost_down";
        return;
    }

    if this.phase == "boost_down" && this.elapsed >= WARMUP_S + MEASURE_S * 2.0 + 1.0 {
        this.boost_down_distance = vertical_distance(this.boost_start, world_pos(this.avatar));
        cmd("SimulateIntent", #{ intent: "yaw_left", held: false, target: this.avatar });
        let result = cmd("SimulateIntent", #{
            intent: "yaw_right",
            held: true,
            target: this.avatar,
        });
        if !result.ok {
            this.done = true;
            fail("could not hold semantic yaw_right intent: " + result.error);
            return;
        }
        this.boost_start = world_pos(this.avatar);
        this.phase = "boost_up";
        return;
    }

    if this.phase == "boost_up" && this.elapsed >= WARMUP_S + MEASURE_S * 2.0 + 2.0 {
        this.boost_up_distance = vertical_distance(this.boost_start, world_pos(this.avatar));
        this.phase = "release";
        return;
    }

    if this.phase == "release" {
        cmd("SimulateIntent", #{ intent: "speed_boost", held: false, target: this.avatar });
        cmd("SimulateIntent", #{ intent: "yaw_right", held: false, target: this.avatar });
        cmd("SimulateIntent", #{ intent: "forward", held: false, target: this.avatar });

        let checks = [];
        checks.push(t_true(this.base_speed > 5.0,
            "normal forward intent did not move the free-flight avatar"));
        checks.push(t_range(this.diagonal_speed, this.base_speed * 0.9,
                            this.base_speed * 1.1,
                            "diagonal intent must preserve single-axis speed"));
        checks.push(t_true(this.boost_down_distance > this.base_speed * 5.0,
            "Shift+Q control frame did not produce the expected tenfold descent"));
        checks.push(t_true(this.boost_up_distance > this.base_speed * 5.0,
            "Shift+E control frame did not produce the expected tenfold ascent"));
        checks.push(t_true(this.edge_seen,
            "atomic semantic intent edge was not delivered to the Rhai event bus"));
        checks.push(t_true(type_of(this.edge_event_correlation) == "u64" &&
                           this.edge_event_correlation == this.edge_correlation,
            "intent.edge telemetry did not preserve the typed command correlation id"));
        let trace = query("CausalTrace", #{
            target: this.avatar,
            correlation_id: this.edge_correlation,
        });
        checks.push(t_true(trace != () && trace.ok != false &&
                           trace.correlation_id == this.edge_correlation,
            "causal trace did not correlate the semantic edge"));
        let origin = ();
        if trace != () && trace.ok != false && trace.correlation_id == this.edge_correlation {
            origin = trace.origin;
        }
        checks.push(t_true(origin != () && origin.kind == "rhai"
                && origin.scope == "twin" && origin.cycle == "simulation"
                && origin.phase == "behavior" && type_of(origin.generation) == "u64"
                && type_of(origin.sequence) == "u64"
                && type_of(origin.actor_id) == "u64"
                && origin.actor_id.to_string() == me.to_string(),
            "causal trace did not preserve the Rhai producer route, actor, and sequence"));
        checks.push(t_true(trace != () && trace.admission == (),
            "deterministic Rhai simulation edge was classified as external input"));
        let missing_trace_is_incomplete = false;
        if trace != () && trace.ok != false {
            let missing_trace = query("CausalTrace", #{
                target: this.avatar,
                correlation_id: trace.correlation_id + trace.correlation_id,
            });
            missing_trace_is_incomplete = missing_trace != () && missing_trace.ok == false;
        }
        checks.push(t_true(missing_trace_is_incomplete,
            "causal trace did not expose an incomplete correlation"));
        checks.push(t_bounded(world_pos(this.avatar), 1000.0,
            "free-flight avatar left the bounded acceptance fixture"));
        print("[free-flight-speed-boost] base=" + r4(this.base_speed) +
            " m/s diagonal=" + r4(this.diagonal_speed) +
            " m/s Shift+Q=" + r4(this.boost_down_distance) +
            " m Shift+E=" + r4(this.boost_up_distance) + " m");
        this.verdict_ok = report_verdict(
            checks, "FREE FLIGHT SPEED BOOST", "FREE_FLIGHT_SPEED_BOOST",
        );
        this.done = true;
    }
}
