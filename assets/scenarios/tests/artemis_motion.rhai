// Production scene test for standard USD motion on the Artemis mission root.
const TEST_KIND = "headless";
const ORION = "/Artemis2Review/Artemis2/Orion";
const NASA_START_TDB_JD = 2461132.5824799724;
const RATE = 64.0;
const MOVE_AFTER_S = 8.0;
const WAIT_LIMIT_S = 120.0;
const SCENE_ADDRESS = "twin://tests/artemis_motion.usda";
const PAUSE_PROBE_1 = "lunco.test.artemis_motion.pause_probe_1";
const PAUSE_PROBE_2 = "lunco.test.artemis_motion.pause_probe_2";
const PAUSE_PROBE_3 = "lunco.test.artemis_motion.pause_probe_3";
const PAUSE_PROBE_4 = "lunco.test.artemis_motion.pause_probe_4";

fn finish(checks) {
    report_verdict(checks, "ARTEMIS USD MOTION", "ARTEMIS_USD_MOTION");
}

fn sample_value(value) {
    if value == () { return "MISSING"; }
    value.to_string()
}

fn on_start(me, ctx) {
    this.phase = 0;
    this.elapsed = 0.0;
    this.done = false;
    this.checks = [];
    subscribe(PAUSE_PROBE_1);
    subscribe(PAUSE_PROBE_2);
    subscribe(PAUSE_PROBE_3);
    subscribe(PAUSE_PROBE_4);
    set_rate(RATE);
    play();
}

fn on_tick(me, ctx) {
    if this.done || this.phase >= 2 { return; }
    this.elapsed += dt();

    let clock = clock_snapshot();
    if clock == () || clock.clock_contract_ok != true
        || clock.presentation_time_available != true {
        if this.elapsed > 20.0 {
            this.done = true;
            finish([t_true(false, "physical or presentation clock was unavailable")]);
        }
        return;
    }

    if this.phase == 0 {
        let orion = find(ORION);
        let position = if orion >= 0 { world_pos(orion) } else { () };
        if position == () {
            if this.elapsed > 20.0 {
                this.done = true;
                finish([t_true(false, "composed Orion pose was unavailable")]);
            }
            return;
        }
        this.orion = orion;
        this.start_pos = position;
        this.start_sim_s = clock.world_sim_s;
        this.start_epoch_jd = clock.epoch_jd;
        this.mission_epoch0_jd = clock.mission_epoch0_jd;
        print("SAMPLE | elapsed_s | presentation_s | rate | Orion_delta_m | TDB_delta_s");
        print("0 | 0.0 | " + sample_value(clock.presentation_sim_s) + " | "
            + clock.virtual_rate + " | 0.0 | 0.0");
        this.phase = 1;
        return;
    }

    let elapsed_sim_s = clock.world_sim_s - this.start_sim_s;
    if elapsed_sim_s < MOVE_AFTER_S {
        if elapsed_sim_s > WAIT_LIMIT_S {
            this.done = true;
            finish([t_true(false, "mission motion did not advance with physical time")]);
        }
        return;
    }

    let position = world_pos(this.orion);
    let travel = if position != () { vlen(vsub(position, this.start_pos)) } else { () };
    let tdb_elapsed_s = (clock.epoch_jd - this.start_epoch_jd) * 86400.0;
    this.checks.push(t_present(position, "Orion position"));
    this.checks.push(t_moved(travel, 1.0,
        "standard USD Orion motion with physical time"));
    this.checks.push(t_moved(abs(tdb_elapsed_s), 1.0,
        "mission TDB epoch with physical time"));
    this.checks.push(t_max(abs(tdb_elapsed_s - elapsed_sim_s), 0.01,
        "mission epoch follows the physical clock"));

    this.checks.push(t_max(abs(this.mission_epoch0_jd - NASA_START_TDB_JD), 0.000000001,
        "scene mission epoch"));
    this.checks.push(t_true(clock.virtual_rate == RATE,
        "physical transport did not run at the requested 64x rate"));
    this.checks.push(t_true(clock.presentation_sim_s <= clock.world_sim_s + 0.000001,
        "presentation time extrapolated beyond completed physical time"));
    this.checks.push(t_true(clock.presentation_interpolation >= 0.0
        && clock.presentation_interpolation <= 1.0,
        "physical interpolation fraction was outside [0, 1]"));
    this.checks.push(t_max(abs(clock.presentation_epoch_jd
        - (this.mission_epoch0_jd + clock.presentation_sim_s / 86400.0)),
        0.00000001, "presentation TDB epoch alignment"));
    print("1 | " + sample_value(elapsed_sim_s) + " | "
        + sample_value(clock.presentation_sim_s) + " | " + clock.virtual_rate
        + " | " + sample_value(travel) + " | " + sample_value(tdb_elapsed_s));

    this.phase = 2;
    this.stop_requested_tick = clock.sim_tick;
    this.stop_requested_admitted_s = clock.admitted_sim_elapsed_s;
    pause();
    emit(PAUSE_PROBE_1, 0);
}

fn on_event(me, evt, ctx) {
    if this.done { return; }

    if this.phase == 2 && evt.name == PAUSE_PROBE_1 {
        // This event runs at the pause command boundary, before PostUpdate
        // publishes the just-completed fixed tick to render-time consumers.
        // Probe again on later paused updates to inspect the settled sample.
        this.phase = 3;
        emit(PAUSE_PROBE_2, 0);
        return;
    }

    if this.phase == 3 && evt.name == PAUSE_PROBE_2 {
        let clock = clock_snapshot();
        let position = world_pos(this.orion);
        print("PAUSE_SETTLED | tick=" + sample_value(clock.sim_tick)
            + " | admitted=" + sample_value(clock.admitted_sim_elapsed_s)
            + " | world=" + sample_value(clock.world_sim_s)
            + " | presentation=" + sample_value(clock.presentation_sim_s)
            + " | alpha=" + sample_value(clock.presentation_interpolation)
            + " | paused=" + sample_value(clock.virtual_paused));
        this.checks.push(t_true(clock.virtual_paused == true
            && clock.simulation_is_running == false,
            "pause did not stop the physical transport"));
        this.checks.push(t_true(clock.sim_tick == this.stop_requested_tick,
            "a physical tick completed after pause was requested"));
        this.checks.push(t_max(abs(clock.admitted_sim_elapsed_s
            - this.stop_requested_admitted_s), 0.000001,
            "admitted physical time after pause"));
        this.checks.push(t_max(abs(clock.presentation_sim_s - clock.world_sim_s),
            0.000001, "paused presentation endpoint"));
        this.checks.push(t_true(clock.presentation_interpolation == 1.0,
            "paused presentation did not select the latest completed physical tick"));
        this.paused_tick = clock.sim_tick;
        this.paused_admitted_s = clock.admitted_sim_elapsed_s;
        this.paused_sim_s = clock.world_sim_s;
        this.paused_presentation_s = clock.presentation_sim_s;
        this.paused_pos = position;
        this.paused_epoch_jd = clock.epoch_jd;
        this.phase = 4;
        emit(PAUSE_PROBE_3, 0);
        return;
    }

    if this.phase == 4 && evt.name == PAUSE_PROBE_3 {
        let clock = clock_snapshot();
        let position = world_pos(this.orion);
        let orion_delta = if position != () {
            vlen(vsub(position, this.start_pos))
        } else { () };
        print("PAUSE_STABLE | tick=" + sample_value(clock.sim_tick)
            + " | admitted=" + sample_value(clock.admitted_sim_elapsed_s)
            + " | world=" + sample_value(clock.world_sim_s)
            + " | presentation=" + sample_value(clock.presentation_sim_s)
            + " | alpha=" + sample_value(clock.presentation_interpolation)
            + " | paused=" + sample_value(clock.virtual_paused)
            + " | Orion_delta=" + sample_value(orion_delta));
        let total_orion_travel = if position != () {
            vlen(vsub(position, this.start_pos))
        } else { () };
        this.checks.push(t_true(clock.sim_tick == this.paused_tick,
            "SimTick advanced while paused"));
        this.checks.push(t_max(abs(clock.admitted_sim_elapsed_s - this.paused_admitted_s),
            0.000001, "admitted physical time while paused"));
        this.checks.push(t_max(abs(clock.world_sim_s - this.paused_sim_s),
            0.000001, "physical time while paused"));
        this.checks.push(t_max(abs(clock.presentation_sim_s - this.paused_presentation_s),
            0.000001, "presentation time while paused"));
        this.checks.push(t_max(abs(clock.epoch_jd - this.paused_epoch_jd),
            0.000000001, "celestial TDB epoch while paused"));
        if position != () && this.paused_pos != () {
            this.checks.push(t_max(vlen(vsub(position, this.paused_pos)), 0.01,
                "Orion pose while paused"));
        } else {
            this.checks.push("Orion pose while paused: MISSING");
        }
        this.phase = 5;
        this.noop_waits = 0;
        load_scene(SCENE_ADDRESS);
        emit(PAUSE_PROBE_4, 0);
        return;
    }

    if this.phase == 5 && evt.name == PAUSE_PROBE_4 {
        let readiness = query("GetReadiness", #{});
        if readiness == () {
            this.done = true;
            this.checks.push("readiness query after idempotent scene load: MISSING");
            finish(this.checks);
            return;
        }
        if readiness.faulted == true {
            this.done = true;
            this.checks.push("idempotent scene load faulted the settled time policy");
            finish(this.checks);
            return;
        }
        if readiness.ready != true {
            this.noop_waits += 1;
            if this.noop_waits >= 20 {
                this.done = true;
                this.checks.push("idempotent scene load did not settle");
                finish(this.checks);
            } else {
                emit(PAUSE_PROBE_4, 0);
            }
            return;
        }
        let clock = clock_snapshot();
        let position = world_pos(this.orion);
        let total_orion_travel = if position != () {
            vlen(vsub(position, this.start_pos))
        } else { () };
        this.checks.push(t_true(clock.virtual_paused == true
            && clock.sim_tick == this.paused_tick,
            "loading the active scene again changed the physical clock"));
        this.checks.push(t_max(abs(clock.epoch_jd - this.paused_epoch_jd),
            0.000000001, "idempotent scene load changed the selected epoch"));
        print("2 | paused | " + sample_value(clock.presentation_sim_s) + " | "
            + clock.virtual_rate + " | " + sample_value(total_orion_travel) + " | "
            + sample_value((clock.epoch_jd - this.start_epoch_jd) * 86400.0));
        this.done = true;
        finish(this.checks);
    }
}
