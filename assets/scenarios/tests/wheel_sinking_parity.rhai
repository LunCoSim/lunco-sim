//! Wheel-sinking parity check — does the rover's ride height stay STABLE as it
//! drives over the streamed terrain?
//!
//! The wheel-sinking artifact (visual terrain vs physics collider band
//! mismatch) shows up as the rover dropping progressively below the terrain
//! surface as it crosses tiles — the collider keeps relief the drawn/morphed
//! surface flattens, so the rover falls into dips the eye cannot see. With the
//! SurfaceBand contact-floor + morph-pinning fixes, the collider and the drawn
//! surface agree, so the rover's offset from the analytic surface stays bounded.
//!
//! ## What this measures
//!
//! `sink = TerrainHeight(x,z) − roverY` at each tick. There is an AUTHOR'd
//! spawn droop (~4.5 m on apollo15: the rover spawns below the analytic surface
//! to account for wheel radius + suspension + the collider-band offset), so the
//! absolute sink is not meaningful on its own. The wheel-sinking signature is
//! the sink **growing without bound** as the rover drives — i.e. the drift
//! (max − min over the drive) blowing out. A healthy drive keeps drift under a
//! few metres (the rover following real relief); a sinking drive sees drift
//! climb to tens of metres.
//!
//! Verdict on channel "wheel_sinking":
//!   PASS — drove DRIVE_TICKS with sink drift under DRIFT_FAIL.
//!   FAIL — sink drift exceeded DRIFT_FAIL (rover is progressively sinking).
//!
//! Baseline (unmodified code) measured: first≈4.4m, drift 0.8–0.9m, max≈5.3m.

const DRIFT_FAIL = 8.0;        // m — healthy drive stays well under this; a
                               // sinking rover (the bug) drifts 20m+
const DRIVE_TICKS = 360;       // ~6 s at 60 Hz — crosses several collider tiles
const SPEED = 1.2;             // m/s forward

fn on_start(me) {
    this.rover = find("/Traverse/Rover");
    this.tick = 0;
    this.min_sink = 9999.0;
    this.max_sink = -9999.0;
    this.first_sink = ();
    this.sampled = 0;
}

fn on_tick(me) {
    if this.rover == 0 { return; }
    this.tick += 1;

    let pos = world_pos(this.rover);
    if pos == () { return; }
    // world_pos returns [x, y, z] (array — rhai indexes by [i], not .x).
    let px = pos[0];
    let py = pos[1];
    let pz = pos[2];

    // Drive forward (+X) each tick: aim 10 m ahead.
    nav_to(this.rover, [px + 10.0, py, pz], SPEED, 2.0);

    // Sample the analytic terrain height under the rover.
    let terrain = query("TerrainHeight", #{ x: px, z: pz });
    if terrain == () || !terrain.found { return; }
    let sink = terrain.height - py;
    this.sampled += 1;
    if this.first_sink == () { this.first_sink = sink; }
    if sink < this.min_sink { this.min_sink = sink; }
    if sink > this.max_sink { this.max_sink = sink; }

    if this.tick % 90 == 0 {
        emit("sink_track", #{
            tick: this.tick, x: px, sink: sink,
            first: this.first_sink, drift: this.max_sink - this.min_sink,
        });
    }

    // Early FAIL: drift has blown out — the rover is progressively sinking.
    if this.max_sink - this.min_sink > DRIFT_FAIL {
        emit("wheel_sinking", #{
            verdict: "FAIL", first: this.first_sink, min_sink: this.min_sink,
            max_sink: this.max_sink, drift: this.max_sink - this.min_sink,
        });
        print(`wheel_sinking FAIL: drift ${this.max_sink - this.min_sink}m > ${DRIFT_FAIL} at tick ${this.tick} (x=${px})`);
        return;
    }

    // Drove long enough → verdict.
    if this.tick >= DRIVE_TICKS {
        let drift = this.max_sink - this.min_sink;
        emit("wheel_sinking", #{
            verdict: "PASS", first: this.first_sink, min_sink: this.min_sink,
            max_sink: this.max_sink, drift: drift, sampled: this.sampled,
        });
        return;
    }
}
