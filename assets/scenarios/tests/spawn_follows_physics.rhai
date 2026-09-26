// Production regression for the runtime catalog spawn path.
//
// This used to be a Rust integration test that loaded a shipped USD asset,
// manually inserted Avian components, and then inspected the resulting ECS
// world. The production scene runner is the authoritative host for that
// composition: SpawnEntity uses the catalog, USD authors the body, Avian
// advances it, and the public script bridge reads both simulation and render
// state.

const CHANNEL = "SPAWN_FOLLOWS_PHYSICS";
const TITLE = "SPAWN FOLLOWS PHYSICS";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ListSpawnCatalog"], required_inputs: [] }
}

fn fail(reason) {
    report_verdict([reason], TITLE, CHANNEL);
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.spawn_requested = false;
    this.balloon = -1;
    this.balloon_access_declared = false;
    this.start_pos = ();
    this.start_render = ();
    this.sampled = false;
    this.done = false;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();

    if !this.spawn_requested {
        let catalog = query("ListSpawnCatalog");
        let catalog_has_balloon = false;
        if catalog != () && catalog.entries != () {
            for entry in catalog.entries {
                if entry.entry_id == "modelica_balloon" {
                    catalog_has_balloon = true;
                }
                if entry.description == () || entry.description.len() == 0 {
                    this.done = true;
                    fail("spawn catalog entry has no authored USD doc: " + entry.entry_id);
                    return;
                }
            }
        }
        if !catalog_has_balloon {
            if this.elapsed > 30.0 {
                this.done = true;
                fail("modelica_balloon never appeared in the spawn catalog");
            }
            return;
        }

        let result = cmd("SpawnEntity", #{
            entry_id: "modelica_balloon",
            position: [0.0, 20.0, 0.0],
        });
        if result.ok {
            this.spawn_requested = true;
        } else if this.elapsed > 30.0 {
            this.done = true;
            fail("SpawnEntity did not accept modelica_balloon: " + result.error);
        }
        return;
    }

    if this.balloon < 0 {
        this.balloon = find_catalog("modelica_balloon");
        if this.balloon < 0 && this.elapsed > 30.0 {
            this.done = true;
            fail("the catalog spawn did not materialize a modelica_balloon entity");
        }
        return;
    }

    if !this.balloon_access_declared {
        try {
            this.balloon_access_declared = track_entity_read(this.balloon);
        } catch (error) {
            this.done = true;
            fail("spawned balloon access declaration failed: " + error);
            return;
        }
        if !this.balloon_access_declared {
            this.done = true;
            fail("spawned balloon read access was not committed");
            return;
        }
    }

    let simulation = world_pos(this.balloon);
    let render = get(this.balloon, "Transform.translation");
    if simulation == () || render == () {
        if this.elapsed > 30.0 {
            this.done = true;
            fail("spawned balloon did not publish both simulation and render poses");
        }
        return;
    }

    if !this.sampled {
        this.start_pos = simulation;
        this.start_render = render;
        this.sampled = true;
        return;
    }

    // The body starts at y=20 and authored gravity is non-zero. Require actual
    // motion before comparing the two pose owners, so two frozen values cannot
    // produce a vacuous pass.
    let moved = this.start_pos[1] - simulation[1];
    let pose_error = vlen(vsub(simulation, render));
    if moved > 1.0 {
        this.done = true;
        let checks = [];
        checks.push(t_moved(moved, 1.0, "spawned body did not advance under gravity"));
        checks.push(t_max(pose_error, 0.5,
            "render Transform diverged from the simulation Position"));
        checks.push(t_present(this.start_render, "initial render pose"));
        report_verdict(checks, TITLE, CHANNEL);
    } else if this.elapsed > 30.0 {
        this.done = true;
        fail("spawned body remained stationary; the physics step or catalog spawn is dead");
    }
}
