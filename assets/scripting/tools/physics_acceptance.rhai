// Generic live physics/readiness evidence for authored scene tests.
//
// The solver and all runtime state remain in their owning Rust systems. These
// helpers only compose the existing public reads into small, deterministic
// reports so a Twin test can use the same acceptance vocabulary for a rover,
// articulated mechanism, habitat, or landing fixture.

fn evidence_error(message) {
    #{ ok: false, kind: "physics_evidence", error: message }
}

/// Capture one entity's pose, velocity, speed, contact manifold, and optional
/// joint-drive facts at the current simulation instant.
fn sample(entity) {
    if entity == () || entity < 0 {
        return evidence_error("sample requires a registered entity id");
    }
    let position = world_pos(entity);
    let linear_velocity = velocity(entity);
    let contact = contact_debug(entity);
    let drive = joint_drive(entity);
    if position == () && linear_velocity == () && contact == () && drive == () {
        return evidence_error("entity has no readable physics evidence");
    }
    #{
        ok: true,
        kind: "physics_sample",
        entity: entity,
        position: position,
        velocity: linear_velocity,
        speed: if linear_velocity == () { () } else { vlen(linear_velocity) },
        contact: contact,
        joint_drive: drive,
    }
}

/// Read solver contact geometry for one body and preserve the raw snapshot.
fn contact_report(entity) {
    if entity == () || entity < 0 {
        return evidence_error("contact_report requires a registered entity id");
    }
    let contact = contact_debug(entity);
    if contact == () {
        return evidence_error("contact_debug returned no snapshot");
    }
    #{ ok: true, kind: "contact_report", entity: entity, contact: contact }
}

/// Check the contact evidence needed by a generic landing/release test.
/// Thresholds are supplied by the authored fixture; the tool never assumes
/// the geometry or solver tolerances of a particular vehicle.
fn contact_acceptance(report, min_touching_pairs, min_manifolds, max_penetration) {
    if report == () || report.ok != true || report.contact == () {
        return #{ ok: false, kind: "contact_acceptance", errors: [
            "contact acceptance has no contact report",
        ]};
    }
    let contact = report.contact;
    let errors = [];
    if contact.touching_pair_count < min_touching_pairs {
        errors.push("touching pair count is below the authored minimum");
    }
    if contact.manifold_count < min_manifolds {
        errors.push("manifold count is below the authored minimum");
    }
    if contact.max_penetration > max_penetration {
        errors.push("maximum penetration exceeds the authored limit");
    }
    #{
        ok: errors.len() == 0,
        kind: "contact_acceptance",
        errors: errors,
        touching_pair_count: contact.touching_pair_count,
        manifold_count: contact.manifold_count,
        max_penetration: contact.max_penetration,
        report: report,
    }
}

/// Check a measured constraint distance between two explicit bodies.
fn joint_distance_acceptance(anchor, body, max_distance, tolerance) {
    if anchor == () || body == () || anchor < 0 || body < 0 {
        return evidence_error("joint distance requires two registered entity ids");
    }
    let anchor_position = world_pos(anchor);
    let body_position = world_pos(body);
    if anchor_position == () || body_position == () {
        return evidence_error("joint distance could not read both body positions");
    }
    let measured = vlen(vsub(body_position, anchor_position));
    let limit = max_distance + tolerance;
    #{
        ok: measured <= limit,
        kind: "joint_distance_acceptance",
        anchor: anchor,
        body: body,
        measured: measured,
        limit: limit,
        error: if measured <= limit { "" } else { "joint distance exceeds the authored limit" },
    }
}

/// Check that two explicit positions demonstrate the minimum motion expected
/// from a drive/command. A zero move remains a failure, preventing dead tests.
fn motion_acceptance(before, after, minimum_distance) {
    if before == () || after == () || before.len() != 3 || after.len() != 3 {
        return evidence_error("motion acceptance requires two 3D positions");
    }
    let measured = vlen(vsub(after, before));
    #{
        ok: measured >= minimum_distance,
        kind: "motion_acceptance",
        measured: measured,
        minimum: minimum_distance,
        error: if measured >= minimum_distance { "" } else { "measured motion is below the authored minimum" },
    }
}

/// Summarize sampled states for a generic settling criterion. The scenario
/// owns sampling cadence and may combine this with contact or joint reports.
fn settling_acceptance(samples, max_speed, max_penetration) {
    if samples == () || samples.len() == 0 {
        return evidence_error("settling acceptance requires at least one sample");
    }
    let peak_speed = 0.0;
    let peak_penetration = 0.0;
    let errors = [];
    for item in samples {
        if item == () || item.ok != true || item.speed == () {
            errors.push("a settling sample has no finite speed");
            continue;
        }
        if item.speed > peak_speed { peak_speed = item.speed; }
        if item.contact != () && item.contact.max_penetration > peak_penetration {
            peak_penetration = item.contact.max_penetration;
        }
    }
    if peak_speed > max_speed {
        errors.push("peak settling speed exceeds the authored limit");
    }
    if peak_penetration > max_penetration {
        errors.push("peak settling penetration exceeds the authored limit");
    }
    #{
        ok: errors.len() == 0,
        kind: "settling_acceptance",
        samples: samples.len(),
        peak_speed: peak_speed,
        peak_penetration: peak_penetration,
        max_speed: max_speed,
        max_penetration: max_penetration,
        errors: errors,
    }
}

/// Collect the existing readiness, binding, and runtime-diagnostic surfaces.
/// This is the generic report used to distinguish a physical failure from a
/// model/connection that never became admissible.
fn system_evidence() {
    let readiness = query("GetReadiness", #{});
    let bindings = query("BindingStatus", #{});
    let diagnostics = query("RuntimeDiagnostics", #{});
    if readiness == () || bindings == () || diagnostics == () {
        return evidence_error("one or more runtime evidence queries were unavailable");
    }
    #{
        ok: true,
        kind: "physics_system_evidence",
        readiness: readiness,
        bindings: bindings,
        diagnostics: diagnostics,
    }
}

/// Fail closed on readiness faults and runtime errors while preserving the
/// complete reports for a useful authored verdict.
fn system_acceptance(evidence) {
    if evidence == () || evidence.ok != true {
        return evidence_error("system evidence is unavailable");
    }
    let errors = [];
    if evidence.readiness.faulted == true {
        errors.push("readiness reports a runtime fault");
    }
    if evidence.diagnostics.errors != () && evidence.diagnostics.errors > 0 {
        errors.push("runtime diagnostics contain errors");
    }
    if evidence.bindings.runtime_fault != () {
        errors.push("binding status reports a runtime fault");
    }
    #{ ok: errors.len() == 0, kind: "physics_system_acceptance", errors: errors, evidence: evidence }
}
