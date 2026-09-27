// Production regression for the runtime catalog spawn path.
//
// The production scene runner exercises the catalog spawn, USD body projection,
// fixed-tick admission record, Avian motion, and public simulation/render reads.

const CHANNEL = "SPAWN_FOLLOWS_PHYSICS";
const TITLE = "SPAWN FOLLOWS PHYSICS";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ListSpawnCatalog", "ReadSessionInputStream"], required_inputs: [] }
}

fn fail(reason) {
    report_verdict([reason], TITLE, CHANNEL);
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.spawn_requested = false;
    this.runtime_admission = false;
    this.spawn_admission = ();
    this.spawn_root_gid = ();
    this.spawn_correlation_id = ();
    this.spawn_record_checked = false;
    this.balloon = -1;
    this.balloon_access_declared = false;
    this.start_pos = ();
    this.start_render = ();
    this.sampled = false;
    this.done = false;
    let started = cmd("StartSessionInputCapture", #{});
    if !started.ok {
        this.done = true;
        fail("session input capture did not start: " + started.error);
    }
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
            // The command has been admitted; set the guard before inspecting
            // its typed acknowledgement so a test assertion cannot resubmit it.
            this.spawn_requested = true;
            let admission = if result.data == () { () } else { result.data["admission"] };
            if admission == () {
                // A document-backed Twin records this edit through ApplyUsdOps;
                // only raw-file scenes use the session-input spawn contract.
                this.runtime_admission = false;
                this.spawn_record_checked = true;
            } else {
                this.runtime_admission = true;
                this.spawn_admission = admission;
                this.spawn_root_gid = admission["spawned_root_gid"];
                this.spawn_correlation_id = result.data["correlation_id"];
                if type_of(admission["scene_generation"]) != "u64" ||
                   type_of(admission["effective_tick"]) != "u64" ||
                   type_of(admission["sequence"]) != "u64" ||
                   type_of(admission["scene_root_gid"]) != "u64" ||
                   type_of(admission["active_frame_gid"]) != "u64" ||
                   type_of(this.spawn_root_gid) != "u64" ||
                   admission["effective_tick"].to_int() <= sim_tick() {
                    this.done = true;
                    fail("SpawnEntity did not return a complete future-tick identity admission");
                    return;
                }
                let invalid_rotation = cmd("SpawnEntity", #{
                    entry_id: "modelica_balloon",
                    position: [0.0, 25.0, 0.0],
                    rotation: [1e308, 1e308, 1e308, 1e308],
                });
                let invalid_rotation_error = invalid_rotation.error;
                let invalid_rotation_rejection =
                    invalid_rotation_error.contains("non-finite pose") ||
                    invalid_rotation_error.contains("finite, nonzero magnitude");
                if invalid_rotation.ok ||
                   invalid_rotation.status != "rejected" ||
                   !invalid_rotation_rejection {
                    this.done = true;
                    fail("SpawnEntity did not reject a quaternion with an unrepresentable norm");
                    return;
                }
                let supplied_actor_identity = cmd("SpawnEntity", #{
                    entry_id: "modelica_balloon",
                    position: [0.0, 25.0, 0.0],
                    producer_id: 8181,
                });
                if supplied_actor_identity.ok ||
                   supplied_actor_identity.status != "rejected" ||
                   !supplied_actor_identity.error.contains("Twin Rhai SpawnEntity") {
                    this.done = true;
                    fail("Twin Rhai did not reject an explicit producer_id in favor of its stable actor identity");
                    return;
                }
            }
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

    if this.runtime_admission && !this.spawn_record_checked {
        let stream = query("ReadSessionInputStream", #{});
        let matching_spawn = false;
        if stream["state"] == "recording" && type_of(stream["records"]) == "array" {
            for record in stream["records"] {
                let payload = record["payload"];
                if payload["kind"] != "runtime_spawn" { continue; }
                let value = payload["value"];
                let producer = record["producer"];
                if producer["kind"] == "rhai" &&
                   type_of(producer["value"]["actor"]) == "u64" &&
                   producer["value"]["actor"].to_string() != "0" &&
                   record["target"].to_string() == this.spawn_admission["scene_root_gid"].to_string() &&
                   record["scene_generation"].to_string() == this.spawn_admission["scene_generation"].to_string() &&
                   record["effective_tick"].to_string() == this.spawn_admission["effective_tick"].to_string() &&
                   record["sequence"].to_string() == this.spawn_admission["sequence"].to_string() &&
                   value["entry_id"] == "modelica_balloon" &&
                   value["active_frame"].to_string() == this.spawn_admission["active_frame_gid"].to_string() &&
                   value["requested_position"][1] == 20.0 &&
                   value["correlation_id"].to_string() == this.spawn_correlation_id.to_string() &&
                   value["spawned_root"].to_string() == this.spawn_root_gid.to_string() {
                    matching_spawn = true;
                }
            }
        }
        if !matching_spawn {
            if this.elapsed > 30.0 {
                this.done = true;
                fail("session input capture did not retain the admitted spawn producer, pose, order, and identities");
            }
            return;
        }
        if this.runtime_admission &&
           this.balloon.to_string() != this.spawn_root_gid.to_string() {
            this.done = true;
            fail("materialized runtime spawn did not retain its reserved GlobalEntityId");
            return;
        }
        this.spawn_record_checked = true;
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
        let stopped = cmd("StopSessionInputCapture", #{});
        checks.push(t_true(stopped.ok && stopped.data["state"] == "complete",
            "session input capture did not complete after the committed spawn"));
        report_verdict(checks, TITLE, CHANNEL);
    } else if this.elapsed > 30.0 {
        this.done = true;
        fail("spawned body remained stationary; the physics step or catalog spawn is dead");
    }
}
