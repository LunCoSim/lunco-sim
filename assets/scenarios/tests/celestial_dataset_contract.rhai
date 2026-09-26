//! Shipped celestial dataset declarations are visible through the production
//! dataset query surface; no Rust test reads the asset library directly.

const TEST_KIND = "headless";
const WAIT_S = 8.0;

fn metadata_body_id(dataset) {
    if dataset == () || dataset.metadata == () || dataset.metadata.body == () {
        return -1;
    }
    dataset.metadata.body.naif_id
}

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ListDatasets"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
    this.clock_request_sent = false;
    this.clock_command_ok = false;
    this.clock_before = ();
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();

    if !this.clock_request_sent {
        this.clock_before = clock_snapshot();
        let result = cmd("SetCelestialClock", #{
            scale: 100000.0
        });
        this.clock_command_ok = result != () && result.ok == true;
        this.clock_request_sent = true;
        return;
    }

    let clock = clock_snapshot();
    if clock.sim_tick <= this.clock_before.sim_tick { return; }

    let invalid_rate_result = cmd("SetCelestialClock", #{ scale: 100001.0 });
    let clock_after_invalid_rate = clock_snapshot();

    let response = query("ListDatasets", #{});
    if response == () || response.datasets == () {
        if this.elapsed < WAIT_S { return; }
        this.done = true;
        report_verdict([t_true(false, "ListDatasets did not publish its dataset registry")],
            "CELESTIAL DATASET CONTRACT", "CELESTIAL_DATASET_CONTRACT");
        return;
    }

    let earth = ();
    let moon = ();
    for dataset in response.datasets {
        if dataset.group == "celestial" && dataset.key == "earth" { earth = dataset; }
        if dataset.group == "celestial" && dataset.key == "moon" { moon = dataset; }
    }
    if earth == () || moon == () {
        if this.elapsed < WAIT_S { return; }
    }

    this.done = true;
    let checks = [];
    checks.push(t_true(earth != () && metadata_body_id(earth) == 399,
        "earth imagery must declare NAIF 399 in its dataset metadata"));
    checks.push(t_true(moon != () && metadata_body_id(moon) == 301,
        "moon imagery must declare NAIF 301 in its dataset metadata"));
    checks.push(t_true(earth != () && earth.processed == true
        && earth.artifact_uri == "lunco://textures/earth.png",
        "earth must expose its processed logical artifact URI"));
    checks.push(t_true(moon != () && moon.processed == true
        && moon.artifact_uri == "lunco://textures/moon.png",
        "moon must expose its processed logical artifact URI"));
    checks.push(t_true(this.clock_command_ok,
        "SetCelestialClock must accept the 100000x celestial rate"));
    checks.push(t_true(clock.celestial_time_available == true
        && clock.celestial_rate == 100000.0 && clock.celestial_parent == "world_time",
        "the celestial clock snapshot must report a 100000x child of WorldTime"));
    checks.push(t_true(clock.virtual_rate == this.clock_before.virtual_rate
        && clock.transport_rate == this.clock_before.transport_rate,
        "scaling CelestialTime must leave the physics transport rate unchanged"));
    checks.push(t_true(clock.world_sim_s > this.clock_before.world_sim_s
        && clock.world_sim_s - this.clock_before.world_sim_s < 1.0,
        "the physical clock must advance normally while celestial time is accelerated"));
    checks.push(t_true(clock.celestial_delta_s > 0.0
        && clock.celestial_epoch_jd > this.clock_before.celestial_epoch_jd,
        "CelestialTime must advance as a scaled child of the progressing WorldTime"));
    checks.push(t_true(invalid_rate_result != () && invalid_rate_result.ok == false
        && invalid_rate_result.status == "failed",
        "SetCelestialClock must report an over-limit rate as a terminal command failure"));
    checks.push(t_true(clock_after_invalid_rate.celestial_rate == clock.celestial_rate
        && clock_after_invalid_rate.celestial_parent == "world_time",
        "rejecting an over-limit rate must leave the CelestialTime child unchanged"));
    report_verdict(checks, "CELESTIAL DATASET CONTRACT", "CELESTIAL_DATASET_CONTRACT");
}
