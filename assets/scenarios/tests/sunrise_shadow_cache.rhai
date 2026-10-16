// Production GPU cache lifecycle gate in sunrise_shadow_cache.usda.
// Assertions use production cache/upload and fixed-physics clock facts.
const TEST_KIND = "editor";
const SUN = "/World/Sun";
fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [find_path(SUN)], entity_writes: [],
       query_reads: ["TerrainLodStatus"], required_inputs: [] }
}
fn on_start(me, ctx) {
    this.phase = 0; this.elapsed = 0.0; this.done = false; this.failures = [];
    this.before = clock_snapshot();
    cmd("SetEnvironmentLight", #{sun_pitch: -0.25, sun_yaw: 0.5});
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 30.0 {
        this.done = true;
        report_verdict(["Sunrise cache phase timed out: " + this.phase], "SUNRISE SHADOW CACHE", "SUNRISE_SHADOW_CACHE");
        return;
    }
    let status = query("TerrainLodStatus", #{});
    if status == () || status.shadow_caches == () || status.shadow_caches.len() != 1 { return; }
    let cache = status.shadow_caches[0];
    let batches = if cache.uploads == () { 0 } else { cache.uploads.batches.to_int() };
    let now = clock_snapshot();
    let direction = qrot(world_rotation_quat(find_path(SUN)), vec3(0.0, 0.0, 1.0));
    let sampled = vec3(cache.sampled_sun_local[0], cache.sampled_sun_local[1], cache.sampled_sun_local[2]);
    let current = vlen(direction - sampled) < 0.001 && !cache.pending;
    if this.phase == 0 {
        if !current { return; }
        this.before = now;
        this.image = cache.image;
        this.batches = batches;
        this.started = now.world_sim_s;
        cmd("SetEnvironmentLight", #{sun_pitch: 0.1});
        this.phase = 1;
    } else if this.phase == 1 {
        if now.world_sim_s - this.started < 0.5 { return; }
        this.failures.push(t_true(direction.y < 0.0 && cache.bound == 1.0 && cache.image == this.image,
            "night detached the committed cache binding"));
        this.failures.push(t_true(batches == this.batches,
            "night scheduled unnecessary visibility uploads"));
        // Exercise a positive elevation very close to the geometric horizon.
        cmd("SetEnvironmentLight", #{sun_pitch: -0.00001});
        this.phase = 2;
    } else if this.phase == 2 {
        if !current || batches <= this.batches { return; }
        this.failures.push(t_true(direction.y > 0.0 && cache.bound == 1.0 && cache.image == this.image,
            "sunrise replaced or detached the cache binding"));
        this.failures.push(t_true(cache.uploads.texture_allocations.to_int() == 1,
            "a visibility replacement allocated another GPU texture"));
        this.batches = batches;
        cmd("SetEnvironmentLight", #{sun_pitch: -0.3});
        this.phase = 3;
    } else if this.phase == 3 {
        if !current || batches <= this.batches { return; }
        this.failures.push(t_true(cache.image == this.image && cache.uploads.texture_allocations.to_int() == 1,
            "a second Sun direction changed the resident GPU texture"));
        this.failures.push(t_true(cache.uploads.bytes.to_int() > 0,
            "cache updates never reached the production upload boundary"));
        this.started = now.world_sim_s;
        this.batches = batches;
        cmd("SetEnvironmentLight", #{sun_pitch: 0.00001});
        this.phase = 4;
    } else if this.phase == 4 {
        if now.world_sim_s - this.started < 0.5 { return; }
        this.failures.push(t_true(direction.y < 0.0 && cache.bound == 1.0 && cache.image == this.image,
            "reverse horizon crossing detached the cache"));
        cmd("SetEnvironmentLight", #{sun_pitch: -0.00001});
        this.phase = 5;
    } else if this.phase == 5 {
        if !current || batches <= this.batches { return; }
        this.done = true;
        this.failures.push(t_true(direction.y > 0.0 && cache.bound == 1.0 && cache.image == this.image
            && cache.uploads.texture_allocations.to_int() == 1,
            "return from night changed texture ownership"));
        this.failures.push(t_true(now.sim_tick > this.before.sim_tick
            && abs((now.physics_elapsed_s - this.before.physics_elapsed_s)
                - (now.sim_tick - this.before.sim_tick) * now.fixed_dt_s) < 0.000000001,
            "shadow preparation interrupted fixed physics cadence"));
        report_verdict(this.failures, "SUNRISE SHADOW CACHE", "SUNRISE_SHADOW_CACHE");
    }
}
