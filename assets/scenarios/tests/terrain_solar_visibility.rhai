// Real terrain -> environment probe -> electrical network -> panel power.
// The independent ray query selects lit and occulted ground in the maintained
// DEM; the elevated case verifies that visibility uses the mount height.
const ROOT = "/TerrainSolarVisibility";
const POWER = ROOT + "/Power";
const PROBE = POWER + "/Panel/Environment";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [find_path(POWER)],
       entity_reads: [find_path(PROBE)], entity_writes: [find_path(POWER), find_path(ROOT + "/Blocker")],
       query_reads: ["TerrainHeight", "TerrainRaycast"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.updates = 0;
    this.phase_time = 0.0;
    this.phase = "find_sites";
    this.done = false;
    this.power = find_path(POWER);
    this.probe = find_path(PROBE);
    this.shadow = ();
    this.clear = ();
    this.results = [];
    this.raised_power = ();
    this.blocker = find_path(ROOT + "/Blocker");
}

fn move_panel(position, phase) {
    cmd("MoveEntity", #{ entity_id: this.power, translation: position });
    this.phase = phase;
    this.phase_time = 0.0;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    this.updates += 1;
    this.phase_time += dt();
    if this.elapsed > 15.0 || this.updates > 20000 {
        this.done = true;
        report_verdict(["terrain solar visibility timed out in " + this.phase],
            "TERRAIN SOLAR VISIBILITY", "TERRAIN_SOLAR_VISIBILITY");
        return;
    }
    if this.phase == "find_sites" {
        let ready = query("TerrainHeight", #{x: 0.0, z: 0.0});
        if ready == () || ready.found != true || get(this.power, "available_power") == () { return; }
        let sy = sin(5.0 * PI() / 180.0);
        let sz = cos(5.0 * PI() / 180.0);
        for x in [-400.0, -200.0, 0.0, 200.0, 400.0] {
            for z in [-400.0, -300.0, -200.0, -100.0, 0.0, 100.0, 200.0, 300.0, 400.0] {
                let h = query("TerrainHeight", #{x: x, z: z});
                if h.found != true { continue; }
                let point = [x, h.height + 1.5, z];
                let hit = query("TerrainRaycast", #{origin: point, dir: [0.0, sy, sz], max: 2000.0});
                if hit.hit == true && this.shadow == () { this.shadow = point; }
                if hit.hit == false && this.clear == () { this.clear = point; }
            }
        }
        if this.shadow == () || this.clear == () {
            this.done = true;
            report_verdict(["fixture must contain both lit and terrain-occluded sites"],
                "TERRAIN SOLAR VISIBILITY", "TERRAIN_SOLAR_VISIBILITY");
            return;
        }
        print("[terrain-solar] shadow=" + this.shadow + " clear=" + this.clear);
        this.move_panel([this.shadow[0], -1800.0, this.shadow[2]], "raised");
        return;
    }
    if this.phase_time < 0.25 { return; }
    let visibility = get(this.probe, "sun_visibility");
    let available = get(this.power, "available_power");
    let incidence = get(this.power, "incidence");
    let irradiance = get(this.probe, "sun_irradiance_w_m2");
    let incident = get(this.power, "incident_irradiance");
    if visibility == () || available == () || incidence == () { return; }
    print("[terrain-solar] " + this.phase + " visibility=" + visibility
        + " available=" + available + " incidence=" + incidence);
    if this.phase == "raised" {
        this.results.push(t_true(visibility == 1.0 && available > 10.0,
            "raised panel must clear the terrain and generate power"));
        this.results.push(t_true(incidence > 0.05, "fixture panel must face the Sun"));
        this.results.push(t_true(abs(irradiance - 1361.0) < 1.0e-9 && abs(incident - irradiance * incidence) < 1.0e-6,
            "Modelica must consume the physical irradiance without duplicating incidence"));
        this.raised_power = available;
        cmd("MoveEntity", #{entity_id: this.blocker, translation: [this.shadow[0] - 2.0, -1799.7, this.shadow[2] + 2.0]});
        this.phase = "partial";
        this.phase_time = 0.0;
    } else if this.phase == "partial" {
        this.results.push(t_true(abs(visibility - 2.0/3.0) < 1.0e-9 && abs(available - this.raised_power * 2.0/3.0) < 1.0e-6,
            "a thin optical proxy must occlude exactly one of three sampling columns"));
        cmd("MoveEntity", #{entity_id: this.blocker, translation: [-20.0, -1799.7, 2.0]});
        this.phase = "unblocked";
        this.phase_time = 0.0;
    } else if this.phase == "unblocked" {
        this.results.push(t_true(visibility == 1.0 && abs(available - this.raised_power) < 1.0e-6,
            "moving the optical proxy must invalidate the shadow snapshot and restore generation"));
        this.move_panel(this.shadow, "shadow");
    } else if this.phase == "shadow" {
        this.results.push(t_true(visibility == 0.0 && abs(available) < 1.0e-6,
            "terrain-occluded panel must publish zero available solar power"));
        this.results.push(t_true(incidence > 0.05,
            "terrain shade must not be confused with a back-facing panel"));
        this.move_panel(this.clear, "clear");
    } else if this.phase == "clear" {
        this.results.push(t_true(visibility == 1.0 && available > 10.0,
            "moving out of terrain shade must restore power"));
        this.done = true;
        report_verdict(this.results, "TERRAIN SOLAR VISIBILITY", "TERRAIN_SOLAR_VISIBILITY");
    }
}
