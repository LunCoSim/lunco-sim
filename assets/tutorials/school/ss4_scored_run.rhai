//! SS4 — The Scored Run (Space School track).
//!
//! The graded repeat (task §1.2): the same traverse, now with a clock and a score.
//! Points for reaching the POI and returning; a time bonus; a clean-finish bonus
//! for coming home upright (read via `is_tipped`, off the new `world_rotation`
//! accessor). Scoring is pure rhai — no engine primitive — and the total is
//! `emit`ted as a scoreboard event the instructor's dashboard reads.
//!
//! Three difficulty tiers are the SAME script with a harder rover and stricter
//! thresholds: swap the scene's rover (Explorer LT → Standard → Hauler) and lower
//! the time budget. A tiny CSV route importer and a live per-tip penalty are
//! optional adds (see 06_CORE_VS_EXTERNAL_SPLIT.md); the run scores without them.

const TIME_BUDGET = 150.0;   // seconds for full marks on time; tighten per tier.

fn on_start(me) {
    load_scene("tutorials/school/traverse.usda");
    notify_kind("Space School 4/4 · The Scored Run", "info");
    hint("This is the graded run: reach the POI and return, fast and upright. The clock starts when you take control. Good luck.");
}

fn mission(me) {
    let rover = "/Traverse/Rover";
    let poi   = "/Traverse/POI";
    let start = [-380.0, 1.0, -380.0];
    [
        objective("possess", #{
            text: "Take control — the clock starts now",
            requires_event: "cmd:PossessVessel",
            on_complete: |m| { this.t0 = elapsed_seconds(); hint("Clock running. Drive to the POI and bring it home."); },
        }),
        objective("reach_poi", #{
            text: "Reach the Point of Interest (+50)",
            requires: ["possess"],
            done: |m| { distance(find(rover), find(poi)) < 8.0 },
            dwell: 0.5,
            on_complete: |m| notify_kind("POI reached (+50). Now get back.", "success"),
        }),
        objective("return_home", #{
            text: "Return to the NW start (+50)",
            requires: ["reach_poi"],
            done: |m| { arrived(find(rover), start, 12.0) },
            dwell: 0.5,
            on_complete: |m| { score_run(this, find(rover)); },
        }),
    ]
}

/// Tally and publish the score. Pure rhai: base + time bonus + upright bonus.
fn score_run(state, rover) {
    let elapsed = elapsed_seconds() - state.t0;
    let base = 100.0;                                   // POI (50) + return (50)
    let time_bonus = global::TIME_BUDGET - elapsed;
    if time_bonus < 0.0 { time_bonus = 0.0; }
    let upright = if is_tipped(rover, 30.0) { 0.0 } else { 25.0 };
    let total = base + time_bonus + upright;
    state.total = total;
    emit("SCORE", total);                               // → instructor scoreboard
    let tstr = elapsed.to_int();
    let upmsg = if upright > 0.0 { "clean +25" } else { "tipped +0" };
    notify_kind(`Run complete — ${total.to_int()} pts (base 100, time +${time_bonus.to_int()}, ${upmsg}) in ${tstr}s`, "success");
}

fn on_event(me, evt) {
    if evt.name == "MISSION_COMPLETE" {
        hint("That's a scored traverse: to the POI and home, timed and judged for a clean finish. Re-run on a harder rover for the next tier — same site, less forgiving machine. That's the whole workshop.");
        notify_kind("Space School complete 🎉", "success");
    }
}

// DEBUG autopilot — run the whole scored traverse hands-free for regression.
// The traverse is authored as pure DATA (a timeline): `RegisterTimeline` can
// store it in the Twin and the BT.CPP-XML codec can export it for Groot2/ROS.
fn task(me) {
    if !is_debug() { return; }
    let r = "/Traverse/Rover";
    let drive = [
        #{ wait: 1.0 },
        #{ possess: r },
        #{ wait: 0.5 },
        #{ move_to_entity: "/Traverse/POI", speed: 0.75, radius: 8.0, subject: r },
        #{ wait: 0.5 },
        #{ move_to: [-380.0, 1.0, -380.0], speed: 0.75, radius: 11.0, subject: r },
        #{ brake: true, secs: 1.5, subject: r },
    ];
    seq(compile_timeline(drive))
}
