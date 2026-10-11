const POWER = "/TerrainSolarVisibility/Power";
const PROBE = POWER + "/Panel/Environment";
fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [find_path(POWER)], entity_reads: [find_path(PROBE)],
       entity_writes: [find_path(POWER)], query_reads: ["TerrainHeight"], required_inputs: [] }
}
fn on_start(me, ctx) { this.elapsed = 0.0; this.updates = 0; this.done = false; this.placed = false; }
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt(); this.updates += 1;
    if this.elapsed > 5.0 || this.updates > 20000 {
        this.done = true;
        report_verdict(["night solar sample timed out"], "NIGHT SOLAR", "TERRAIN_SOLAR_VISIBILITY_NIGHT");
        return;
    }
    if !this.placed {
        let h = query("TerrainHeight", #{x: 0.0, z: 0.0});
        if h == () || h.found != true {return;}
        cmd("MoveEntity", #{entity_id: find_path(POWER), translation: [0.0, h.height + 1.5, 0.0]});
        this.placed = true; this.elapsed = 0.0;
        return;
    }
    if this.elapsed < 0.25 { return; }
    let power = get(find_path(POWER), "available_power");
    let incidence = get(find_path(POWER), "incidence");
    let visibility = get(find_path(PROBE), "sun_visibility");
    if power == () || incidence == () || visibility == () { return; }
    print("[night-solar] visibility=" + visibility + " power=" + power + " incidence=" + incidence);
    this.done = true;
    report_verdict([
        t_true(incidence > 0.05, "downward-facing panel must face the below-surface Sun"),
        t_true(visibility == 0.0 && abs(power) < 1.0e-6, "terrain must block the below-surface Sun and stop generation")
    ], "NIGHT SOLAR", "TERRAIN_SOLAR_VISIBILITY_NIGHT");
}
