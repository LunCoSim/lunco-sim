// Run on a settled rendered fixture with camera_path, sun_path, caster_path,
// previous_clip_signature and expect_projection_change parameters.
fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["SceneCameraAudit"], required_inputs: [] }
}

fn projection_signature(snapshot, camera, sun_path) {
    let signature = "";
    for light in snapshot.lights {
        if light.usd_path != sun_path { continue; }
        for map in light.maps {
            if map.camera != camera { continue; }
            for bits in map.clip_from_world_bits { signature += bits.to_string() + ","; }
        }
    }
    signature
}

fn pose_signature(snapshot, camera, sun_path) {
    let signature = "";
    for light in snapshot.lights {
        if light.usd_path != sun_path { continue; }
        for map in light.maps {
            if map.camera == camera && map.caster_fingerprints != () {
                signature += map.caster_fingerprints.poses + ",";
            }
        }
    }
    signature
}

fn on_start(me, ctx) {
    let ordinary = query("SceneCameraAudit", #{});
    let audit = query("SceneCameraAudit", #{shadow_inputs: true, shadow_caster_limit: 64});
    if audit == () || audit.shadow_inputs == () {
        report_verdict(["requested native shadow inputs are missing"],
            "SHADOW INPUT AUDIT", "SHADOW_INPUT_AUDIT");
        return;
    }
    if ctx.empty_world == true {
        let inputs = audit.shadow_inputs;
        let empty = if inputs.native_types_registered { inputs.lights.len == 0 }
            else { inputs.lights == () };
        let invalid = query("SceneCameraAudit", #{shadow_inputs: "invalid"});
        report_verdict([
            t_true(ordinary.shadow_inputs == (), "shadow traversal must remain opt-in"),
            t_true(empty, "an empty world must preserve unavailable or empty native data"),
            t_true(invalid.ok == false && invalid.code == 422,
                "non-boolean shadow_inputs must return a typed rejection"),
        ], "EMPTY SHADOW INPUT AUDIT", "EMPTY_SHADOW_INPUT_AUDIT");
        return;
    }
    if !audit.shadow_inputs.native_types_registered {
        report_verdict(["native shadow input types are unavailable"],
            "SHADOW INPUT AUDIT", "SHADOW_INPUT_AUDIT");
        return;
    }
    let camera = ();
    for candidate in audit.candidates {
        if candidate.usd_path == ctx.camera_path { camera = candidate.entity; }
    }
    let valid_matrices = true;
    let fingerprints_present = true;
    let maps = 0;
    let caster_found = false;
    let caster_pose = false;
    for light in audit.shadow_inputs.lights {
        if light.usd_path != ctx.sun_path { continue; }
        for map in light.maps {
            if map.camera != camera { continue; }
            maps += 1;
            valid_matrices = valid_matrices && map.matrices_finite
                && map.camera_3d == true
                && map.world_from_cascade_bits.len == 16
                && map.clip_from_cascade_bits.len == 16
                && map.clip_from_world_bits.len == 16;
            fingerprints_present = fingerprints_present && map.caster_fingerprints != ();
            if map.caster_candidates == () { continue; }
            for caster in map.caster_candidates {
                if caster.usd_path != ctx.caster_path { continue; }
                caster_found = true;
                caster_pose = caster.world_from_local_bits != ()
                    && caster.world_from_local_bits.len == 16
                    && caster.world_from_local_finite == true && caster.mesh_asset != ();
            }
        }
    }
    let signature = projection_signature(audit.shadow_inputs, camera, ctx.sun_path);
    let repeat = query("SceneCameraAudit", #{shadow_inputs: true, shadow_caster_limit: 64});
    let compact = query("SceneCameraAudit", #{shadow_inputs: true});
    let paged = query("SceneCameraAudit", #{shadow_inputs: true, shadow_caster_offset: 1, shadow_caster_limit: 1});
    let page_valid = true;
    let fingerprints_valid = fingerprints_present;
    for li in 0..audit.shadow_inputs.lights.len {
        for mi in 0..audit.shadow_inputs.lights[li].maps.len {
            let full = audit.shadow_inputs.lights[li].maps[mi];
            let page = paged.shadow_inputs.lights[li].maps[mi];
            let small = compact.shadow_inputs.lights[li].maps[mi];
            let expected_len = if full.caster_candidates.len > 1 { 1 } else { 0 };
            page_valid = page_valid && page.caster_candidates.len == expected_len;
            if expected_len == 1 {
                page_valid = page_valid && page.caster_candidates[0].entity == full.caster_candidates[1].entity;
            }
            page_valid = page_valid && small.caster_candidates == ();
            if full.caster_fingerprints == () || page.caster_fingerprints == () || small.caster_fingerprints == () {
                fingerprints_valid = false;
                continue;
            }
            fingerprints_valid = fingerprints_valid
                && full.caster_fingerprints == page.caster_fingerprints
                && full.caster_fingerprints == small.caster_fingerprints;
        }
    }
    let invalid = query("SceneCameraAudit", #{shadow_inputs: "invalid"});
    let invalid_limit = query("SceneCameraAudit", #{shadow_inputs: true, shadow_caster_limit: 65});
    let invalid_offset = query("SceneCameraAudit", #{shadow_inputs: true, shadow_caster_offset: -1});
    report_verdict([
        t_true(ordinary.shadow_inputs == (), "shadow traversal must remain opt-in"),
        t_true(audit.shadow_inputs.native_types_registered && audit.shadow_inputs.host_frame != (),
            "native shadow inputs and their host frame must be available"),
        t_true(camera != () && maps > 0, "the requested camera must own native cascades"),
        t_true(valid_matrices, "native matrices must retain all finite f32 coefficients"),
        t_true(caster_found, "native cascade candidates must include the authored caster"),
        t_true(caster_pose, "caster pose and native mesh identity must be present"),
        t_true(signature == projection_signature(repeat.shadow_inputs, camera, ctx.sun_path),
            "repeated reads in one callback must agree"),
        t_true(invalid.ok == false && invalid.code == 422
            && invalid_limit.ok == false && invalid_limit.code == 422
            && invalid_offset.ok == false && invalid_offset.code == 422,
            "invalid flags, page limits and offsets must return typed rejections"),
        t_true(!ctx.expect_projection_change || signature != ctx.previous_clip_signature,
            "a changed native light projection must change the exact matrix signature"),
        t_true(page_valid, "raw caster pages must match the requested bounds and native order"),
        t_true(fingerprints_valid, "compact and paged reads must retain the same native fingerprints"),
        t_true(!ctx.expect_caster_change || (
            pose_signature(audit.shadow_inputs, camera, ctx.sun_path) != ctx.previous_pose_signature
                && signature == ctx.previous_clip_signature),
            "moving a caster must change its pose fingerprint while the light projection stays fixed"),
    ], "SHADOW INPUT AUDIT", "SHADOW_INPUT_AUDIT");
}
