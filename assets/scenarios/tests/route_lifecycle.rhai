// Production acceptance for the generic route tool and scene-level route
// program. All observable route policy stays here; Rust is exercised only as
// the generic typed USD command and event/port boundary.

const TEST_KIND = "editor";

const ROOT = "/RouteLifecycle";
const ROUTE = "/RouteLifecycle/Route";
const ALTERNATE_ROUTE = "/RouteLifecycle/AlternateRoute";
const SUBJECT_PATH = "/RouteLifecycle/Subject";
const RIBBON_GEOMETRY = "/RouteLifecycle/Route/RouteRibbon/Geometry";
const ADMISSION_LANDING_PATH = "/RouteLifecycle/AdmissionLanding";
const ADMISSION_POINT_PATH = "/RouteLifecycle/AdmissionPoint";
const APPLICATION_ADMISSION_PATH = "/RouteLifecycle/ApplicationAdmissionMarker";
const APPLICATION_ADMISSION_EVENT = "ROUTE_LIFECYCLE_APPLICATION_EDIT";

fn simulation_dependencies(me, ctx) {
    let subject = find_path(SUBJECT_PATH);
    #{ modelica_entities: [subject], entity_reads: [subject], entity_writes: [],
       query_reads: ["QueryUsdPrim", "QueryUsdPrims", "QueryPhysicsState", "ResolveUsdTarget", "InspectSelection", "InspectUsdSelection", "ScriptInspect", "ScriptStatus", "ReadExposures"],
       required_inputs: [] }
}

fn direct_points(doc) {
    let route = waypoint_editor::route_positions(doc, ROUTE);
    if route == () || route.ok != true { return (); }
    route.paths
}

fn point_count(doc) {
    let points = direct_points(doc);
    if points == () { return (); }
    points.len()
}

fn child_index(paths, target) {
    for i in 0..paths.len() {
        if paths[i] == target { return i; }
    }
    -1
}

fn route_revision(doc) {
    usd_document_generation(doc)
}

fn ribbon_present(doc) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: RIBBON_GEOMETRY });
    result != () && result.ok != false
}

fn ribbon_is_open(doc) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: RIBBON_GEOMETRY,
        attrs: ["wrap", "curveVertexCounts", "points"],
    });
    if result == () || result.ok == false || result.attrs == ()
        || result.attrs["wrap"] != "nonperiodic"
        || result.attrs["curveVertexCounts"] == ()
        || result.attrs["points"] == () {
        return false;
    }
    let points = result.attrs["points"];
    let counts = result.attrs["curveVertexCounts"];
    if counts.len() != 1 || counts[0] != points.len() || points.len() < 2 {
        return false;
    }
    let first = points[0];
    let last = points[points.len() - 1];
    first.len() == 3 && last.len() == 3
        && ((first[0] - last[0]).abs() > 0.000001
            || (first[1] - last[1]).abs() > 0.000001
            || (first[2] - last[2]).abs() > 0.000001)
}

fn ribbon_width(doc) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: RIBBON_GEOMETRY,
        attrs: ["widths"],
    });
    if result == () || result.ok == false || result.attrs == ()
        || result.attrs["widths"] == () { return (); }
    result.attrs["widths"]
}

fn ribbon_width_matches(doc, expected) {
    let widths = ribbon_width(doc);
    widths != () && widths.len() == 1 && (widths[0] - expected).abs() < 0.001
}

fn ribbon_normals_match(doc, count) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: RIBBON_GEOMETRY,
        attrs: ["normals"],
    });
    result != () && result.ok != false && result.attrs != ()
        && result.attrs["normals"] != () && result.attrs["normals"].len() == count
}

fn ribbon_anchor_matches_point(doc, point_path) {
    let point = query("QueryUsdPrim", #{
        doc_id: doc,
        path: point_path,
    });
    let ribbon = query("QueryUsdPrim", #{
        doc_id: doc,
        path: "/RouteLifecycle/Route/RouteRibbon",
    });
    if point == () || ribbon == () || point.world_position == ()
        || ribbon.world_position == () { return false; }
    let a = point.world_position;
    let b = ribbon.world_position;
    (a[0] - b[0]).abs() < 0.001
        && (a[1] - b[1]).abs() < 0.001
        && (a[2] - b[2]).abs() < 0.001
}

fn has_path(doc, path) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: path });
    result != () && result.ok != false && result.active == true
}

fn point_position(doc, path) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: path,
        attrs: ["xformOp:translate"],
    });
    if result == () || result.ok == false || result.attrs == () { return (); }
    result.attrs["xformOp:translate"]
}

fn point_distance(left, right) {
    if left == () || right == () || left.len() != 3 || right.len() != 3 {
        return ();
    }
    let dx = left[0] - right[0];
    let dy = left[1] - right[1];
    let dz = left[2] - right[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn marker_color(doc, path) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: path + "/Dome",
        attrs: ["primvars:displayColor"],
    });
    if result == () || result.ok == false || result.attrs == () {
        return ();
    }
    let colors = result.attrs["primvars:displayColor"];
    if colors == () || colors.len() != 1 { return (); }
    colors[0]
}

fn color_matches(actual, expected) {
    actual != () && expected != () && actual.len() == 3 && expected.len() == 3
        && (actual[0] - expected[0]).abs() < 0.000001
        && (actual[1] - expected[1]).abs() < 0.000001
        && (actual[2] - expected[2]).abs() < 0.000001
}

fn context(doc, path, position) {
    let value = #{
        button: "primary",
        screen_position: [64.0, 64.0],
        doc_id: doc,
        scene_root_path: ROOT,
        route_path: ROUTE,
        controlled_path: SUBJECT_PATH,
        target_path: path,
        hit_path: path,
        prim_paths: [path],
        target_entity_id: if path == () { () } else { find_path(path) },
        hit_entity_id: if path == () { () } else { find_path(path) },
        modifiers: #{ alt: true, shift: false, ctrl: false },
        pointer_intents: ["route.add_point"],
        world_position: if position == () { () } else { point3(position, "active_physics") },
    };
    if position != () {
        value.surface_world_position = point3(position, "active_physics");
    }
    value
}

fn modified_context(doc, path, position, modifiers) {
    let value = context(doc, path, position);
    value.modifiers = modifiers;
    value
}

fn implicit_route_context(doc, path, position) {
    let value = context(doc, path, position);
    value.route_path = ();
    value
}

fn selected_route_context(doc, path, position, program) {
    let value = implicit_route_context(doc, path, position);
    let listed = program_editor::list(doc, program);
    if listed.ok && listed.programs.len() > 0 {
        value.selected_path = listed.programs[0]["path"];
    }
    return value;
}

fn invalid_edit_checks(doc) {
    let before = route_revision(doc);
    let missing_context = waypoint_editor::on_click(#{
        button: "primary",
        doc_id: doc,
        scene_root_path: ROOT,
        route_path: ROUTE,
        controlled_path: SUBJECT_PATH,
    });
    let missing_position = waypoint_editor::on_move(context(doc, "/RouteLifecycle/NotAPoint", ()));
    let program = program_editor::list(doc, ROUTE);
    let program_path = if program.ok && program.programs.len() > 0 {
        program.programs[0]["path"]
    } else { () };
    let program_delete = waypoint_editor::on_delete(context(doc, program_path, ()));
    let program_as_route = context(doc, (), [0.0, 1.0, -7.0]);
    program_as_route.route_path = program_path;
    let normalized = waypoint_editor::route_edit_path(program_as_route);
    let after = route_revision(doc);
    [
        t_true(missing_context.ok == false, "click without a world position is rejected"),
        t_true(missing_position.ok == false, "move without a world position is rejected"),
        t_true(program_delete.ok == false, "delete cannot target the route program"),
        t_true(normalized == ROUTE,
            "a program identity is normalized to its containing route scope"),
        t_true(before == after, "invalid route edits do not advance the USD revision"),
    ]
}

fn on_visualization(me, ctx) {
    this.visualization_prepared = true;
}

fn on_start(me, ctx) {
    this.doc = ();
    this.waiting_for = "the route fixture";
    this.route_read_attempts = 0;
    this.done = false;
    this.checks = [];
    this.startup_tick = sim_tick();
    this.first_behavior_tick_checked = false;
    this.first_behavior_tick_valid = false;
    this.first_physics_state_valid = false;
    this.new_point = ROUTE + "/P3";
    this.subject = -1;
    this.started = 0;
    this.stopped = 0;
    this.reached = 0;
    this.reached_index = ();
    this.active_edit = false;
    this.hold_started = 0;
    this.route_ready = false;
    this.route_source_restore_pending = false;
    this.application_admission_pending = false;
    this.reference_admission_start_tick = -1;
    this.move_preview_started = false;
    this.move_preview_update_sent = false;
    this.program_switch_subject = -1;
    this.reached_point_path = ();
    this.program_path = ();
    this.alternate_program_path = ();
    this.reference_admission_started = false;
    this.reference_admission_checked = false;
    this.application_admission_received = false;
    this.application_admission_accepted = false;
    this.application_admission_context_ok = false;
    this.application_admission_event_tick_matches = false;
    this.application_admission_event_tick = ();
    this.application_admission_payload_tick = ();
    this.application_admission_entity_unindexed = false;
    this.application_admission_pause_observed = false;
    this.application_admission_first_tick_checked = false;
    this.route_step_route_fixture_finished = false;
    this.route_step_hud_projection_finished = false;
    this.route_step_switch_program_source_finished = false;
    this.route_step_replacement_program_ready_finished = false;
    this.route_step_restore_program_source_finished = false;
    this.route_step_transient_reference_edit_finished = false;
    this.route_step_append_route_point_finished = false;
    this.route_step_route_point_policy_finished = false;
    this.route_step_context_selection_finished = false;
    this.route_step_move_preview_finished = false;
    this.route_step_commit_move_finished = false;
    this.route_step_move_projection_finished = false;
    this.route_step_delete_appended_point_finished = false;
    this.route_step_move_referenced_point_finished = false;
    this.route_step_undo_referenced_move_finished = false;
    this.route_step_redo_referenced_move_finished = false;
    this.route_step_delete_referenced_point_finished = false;
    this.route_step_undo_referenced_delete_finished = false;
    this.route_step_redo_referenced_delete_finished = false;
    this.route_step_delete_remaining_referenced_point_finished = false;
    this.route_step_one_point_route_finished = false;
    this.route_step_empty_route_finished = false;
    this.route_step_readd_empty_route_finished = false;
    this.route_step_first_new_point_finished = false;
    this.route_step_modifier_noop_finished = false;
    this.route_step_acquire_subject_finished = false;
    this.route_step_start_route_finished = false;
    this.route_step_edit_active_route_finished = false;
    this.route_step_route_point_reached_finished = false;
    this.route_step_manual_route_pause_finished = false;
    this.route_step_release_subject_finished = false;
    this.route_step_free_avatar_input_finished = false;
    this.route_step_route_after_free_input_finished = false;
    this.route_step_hud_route_toggle_finished = false;
    this.route_step_hud_route_stopped_finished = false;
    this.route_step_hud_route_started_again_finished = false;
    this.route_step_generic_point_delete_finished = false;
    this.route_step_select_alternate_program_finished = false;
    this.route_step_add_alternate_point_finished = false;
    this.route_step_alternate_route_edit_finished = false;
    this.route_step_verify_insert_before_finished = false;
    this.route_step_verify_insert_after_finished = false;
    this.route_step_verify_ribbon_add_finished = false;
    this.route_step_application_projection_admission_finished = false;
    this.route_step_application_projection_verdict_finished = false;
}

fn on_event(me, evt, ctx) {
    let route_source = find_path(ROUTE);
    if evt.name == "route_started" && evt.source == route_source { this.started += 1; }
    if evt.name == "route_stopped" && evt.source == route_source { this.stopped += 1; }
    if evt.name == "route_point_reached" && evt.source == route_source {
        this.reached += 1;
        this.reached_index = evt.value;
    }
    if evt.name == "program_switch_hold_started" { this.hold_started += 1; }
    if evt.name == "program.ready" && this.route_source_restore_pending
        && evt.value != () && evt.value["path"] == this.program_path {
        // The readiness event is emitted by the newly started program. A
        // disposable presentation edit may advance the document generation
        // before this broadcast is delivered, so the event's path is the
        // lifecycle identity and the generation is diagnostic context only.
        this.route_ready = true;
    }
    if evt.name == APPLICATION_ADMISSION_EVENT && this.application_admission_pending {
        let execution = evt.value;
        this.application_admission_received = true;
        this.application_admission_accepted = execution["accepted"] == true;
        this.application_admission_context_ok = execution["scope"] == "application"
            && execution["cycle"] == "repl"
            && execution["phase"] == "evaluation"
            && execution["clock"] == "application"
            && execution["sequence"] != ();
        this.application_admission_event_tick_matches = execution["tick"] == evt.sim_tick;
        this.application_admission_event_tick = evt.sim_tick;
        this.application_admission_payload_tick = execution["tick"];
        this.application_admission_entity_unindexed = execution["entity_indexed"] == false;
        this.application_admission_pause_observed = clock_snapshot().virtual_paused;
    }
}

fn finish(me) {
    if me.done { return me; }
    me.done = true;
    me.checks.push(t_true(me.first_behavior_tick_checked && me.first_behavior_tick_valid,
        "the first behavior-tree tick ran one admitted tick after scenario startup"));
    me.checks.push(t_true(me.first_physics_state_valid,
        "the first behavior tick observed a finite admitted body with authored mass and inertia"));
    report_verdict(me.checks, "ROUTE LIFECYCLE", "ROUTE_LIFECYCLE");
    me
}

fn record_first_behavior_tick(me) {
    this.first_behavior_tick_checked = true;
    this.first_behavior_tick_valid = sim_tick() == this.startup_tick + 1;
    if !is_unattended() {
        this.checks.push(t_true(this.visualization_prepared == true,
            "visualization preparation completes before the first behavior-tree tick"));
    }
    let state = query("QueryPhysicsState", #{ id: find_path(SUBJECT_PATH) });
    if state != () && state.ok != false {
        let inertia = state.inertia_principal_kgm2;
        this.first_physics_state_valid = state.body_mode == "dynamic"
            && state.physics_state_ready == true
            && state.physics_state_pending == false
            && state.physics_initialization_pending == false
            && state.collider_present == true
            && state.mass_kg != () && state.mass_kg > 0.0
            && inertia != () && inertia.len() == 3
            && inertia[0] > 0.0 && inertia[1] > 0.0 && inertia[2] > 0.0;
        if !this.first_physics_state_valid {
            print("ROUTE_FIRST_PHYSICS_STATE_INVALID|" + state.to_string());
        }
    }
}

fn reference_admission_projection_checked(me) {
    if !this.reference_admission_started || this.reference_admission_checked { return this.reference_admission_checked; }
    if sim_tick() <= this.reference_admission_start_tick { return false; }
    let landing = find_path(ADMISSION_LANDING_PATH);
    let point = find_path(ADMISSION_POINT_PATH);
    if landing < 0 || point < 0 {
        this.checks.push(t_true(false,
            "the first fixed tick after reference admission observed an incomplete primary USD projection"));
        this = finish(this);
        return true;
    }
    this.checks.push(t_true(true,
        "parallel primary-scene reference preparation commits before the next fixed tick"));
    this.reference_admission_checked = true;
    true
}

fn route_lifecycle_active(me) { !this.done }

fn route_lifecycle_timeout(me) {
    if this.done { return; }
    let detail = "route lifecycle acceptance timed out waiting for " + this.waiting_for;
    if this.doc == () {
        detail += "; route=" + find_path(ROUTE).to_string()
            + ", subject=" + find_path(SUBJECT_PATH).to_string();
    } else {
        let composed_route = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: ROUTE,
            children: true,
        });
        let active_route = query("QueryUsdPrim", #{
            path: ROUTE,
            children: true,
        });
        detail += "; doc=" + this.doc.to_string()
            + ", points=" + point_count(this.doc).to_string()
            + ", ribbon=" + ribbon_present(this.doc).to_string()
            + ", composed_route=" + composed_route.to_string()
            + ", active_route=" + active_route.to_string()
            + ", selection=" + query("InspectSelection", #{}).to_string()
            + ", viewport_selection=" + query("InspectUsdSelection", #{}).to_string();
    }
    if this.waiting_for == "the application/Repl projection and event verdict" {
        detail += "; application_event_received=" + this.application_admission_received.to_string()
            + ", accepted=" + this.application_admission_accepted.to_string()
            + ", context_ok=" + this.application_admission_context_ok.to_string()
            + ", entity_unindexed=" + this.application_admission_entity_unindexed.to_string()
            + ", pause_observed=" + this.application_admission_pause_observed.to_string()
            + ", first_tick_checked=" + this.application_admission_first_tick_checked.to_string();
    }
    this.checks.push(t_true(false, detail));
    this = finish(this);
}


fn route_step_route_fixture_action(me) {
    if this.done { return; }
    this.waiting_for = "the route fixture, ribbon, and program";
    let route = query("QueryUsdPrim", #{ path: ROUTE, children: true });
    if route == () || route.ok == false || route.doc_id == () { return; }
    this.doc = route.doc_id;
    this.subject = find_path(SUBJECT_PATH);
    if this.subject < 0 || point_count(this.doc) != 3 || !ribbon_present(this.doc)
        || !ribbon_normals_match(this.doc, 3)
        || !ribbon_anchor_matches_point(this.doc, ROUTE + "/P0") {
        return;
    }
    this.ribbon_entity = find_path(RIBBON_GEOMETRY);
    if this.ribbon_entity < 0 { return; }
    let listed = program_editor::list(this.doc, ROUTE);
    if listed == () || !listed.ok || listed.programs.len() == 0 { return; }
    this.program_path = listed.programs[0]["path"];
    let program = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: this.program_path,
        attrs: ["info:implementationSource", "info:sourceAsset", "info:id", "info:sourceCode"],
    });
    this.checks.push(t_true(program != () && program.ok != false
        && program.attrs["info:implementationSource"] == "sourceAsset"
        && program.attrs["info:sourceAsset"] != ()
        && program.attrs["info:id"] == ()
        && program.attrs["info:sourceCode"] == (),
        "route program uses the sourceAsset-only USD contract"));
    this.checks.push(t_true(true,
        "route program materializes an oriented ribbon from authored points"));
    this.checks.push(t_true(ribbon_width_matches(this.doc, 0.12),
        "route ribbon keeps a narrow terrain annotation width"));
    this.checks.push(t_true(ribbon_is_open(this.doc),
        "route ribbon is open and never connects the last point to the first"));
    this.checks.push(t_true(ribbon_present(this.doc),
        "route ribbon keeps one disposable USD identity for its live mesh"));
    this.checks.push(t_true(listed.ok && listed.programs.len() == 1
        && listed.programs[0]["path"] == this.program_path,
        "generic program discovery lists the authored executable"));
    let hud = program_editor::enable_hud(this.doc, ROUTE, "always");
    this.checks.push(t_true(hud.ok, "Rhai can opt any USD program scope into the standard HUD"));
    if !hud.ok { this = finish(this); return; }
    let selected = program_editor::select(this.program_path);
    this.checks.push(t_true(selected.ok,
        "program selection establishes the active route without a Rust route registry"));
    this.route_step_route_fixture_finished = true;
    return;
}

fn route_step_route_fixture_complete(me) { this.done || this.route_step_route_fixture_finished == true }


fn route_step_hud_projection_action(me) {
    if this.done { return; }
    this.waiting_for = "the authored HUD projection and exposure";
    let hud_record = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: ROUTE,
        attrs: ["lunco:ui:surfaceId", "lunco:ui:visibilityMode"],
    });
    if hud_record == () || hud_record.ok == false || hud_record.attrs == ()
        || hud_record.attrs["lunco:ui:surfaceId"] == ()
        || hud_record.attrs["lunco:ui:visibilityMode"] == () {
        return;
    }
    if hud_record.attrs["lunco:ui:surfaceId"] != "program-browser"
        || hud_record.attrs["lunco:ui:visibilityMode"] != "always" {
        return;
    }
    let exposed = query("ReadExposures", #{ surface: "program-browser" });
    if exposed == () || exposed.ok == false || exposed.surfaces == () { return; }
    let surface = exposed.surfaces["program-browser"];
    if surface == () || surface.visible != true || surface.properties == ()
        || surface.properties.programs == () || surface.properties.programs.len() != 1 {
        return;
    }
    this.checks.push(t_true(true,
        "HUD activation is authored through the typed USD document"));
    this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
        "authoring runtime HUD metadata preserves the live subject projection"));
    this.checks.push(t_true(surface.visible == true,
        "the Application/Ui owner consumes the authored always-visible policy"));
    this.checks.push(t_true(surface.properties.program_count == "1 PROGRAMS"
            && surface.properties.programs[0].path == this.program_path,
        "the Application/Ui owner publishes the typed program collection properties"));
    for check in invalid_edit_checks(this.doc) { this.checks.push(check); }
    this.route_step_hud_projection_finished = true;
    return;
}

fn route_step_hud_projection_complete(me) { this.done || this.route_step_hud_projection_finished == true }


fn route_step_switch_program_source_action(me) {
    if this.done { return; }
    this.waiting_for = "the runtime source-switch command";
    this.program_switch_subject = find_path(SUBJECT_PATH);
    this.route_ready = false;
    let switched = program_editor::switch_source(
    this.doc,
    this.program_path,
    "lunco://scenarios/tests/program_switch_hold.rhai",
);
if !switched.ok {
    this.checks.push(t_true(false, switched.error));
    this = finish(this);
    return;
}
this.route_step_switch_program_source_finished = true;
}

fn route_step_switch_program_source_complete(me) { this.done || this.route_step_switch_program_source_finished == true }


fn route_step_replacement_program_ready_action(me) {
    if this.done { return; }
    this.waiting_for = "the replacement program readiness event";
    let program = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: this.program_path,
        attrs: ["info:implementationSource", "info:sourceAsset"],
    });
    if program == () || program.ok == false || program.attrs == ()
        || program.attrs["info:sourceAsset"] != "lunco://scenarios/tests/program_switch_hold.rhai"
        || this.hold_started == 0 {
        return;
    }
    this.checks.push(t_true(true,
        "runtime USD source switching activates a new authored Rhai program"));
    this.checks.push(t_true(find_path(SUBJECT_PATH) == this.program_switch_subject,
        "switching a program preserves the subject entity identity"));
    this.route_source_restore_pending = true;
    let restored = program_editor::switch_source(
        this.doc,
        this.program_path,
        "lunco://scenarios/route_follow.rhai",
    );
    if !restored.ok { this.checks.push(t_true(false, restored.error)); this = finish(this); return; }
    this.route_step_replacement_program_ready_finished = true;
    return;
}

fn route_step_replacement_program_ready_complete(me) { this.done || this.route_step_replacement_program_ready_finished == true }


fn route_step_restore_program_source_action(me) {
    if this.done { return; }
    this.waiting_for = "the restored route program and readiness event";
    let program = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: this.program_path,
        attrs: ["info:implementationSource", "info:sourceAsset"],
    });
    if program == () || program.ok == false || program.attrs == ()
        || program.attrs["info:sourceAsset"] != "lunco://scenarios/route_follow.rhai" {
        return;
    }
    if !this.route_ready { return; }
    this.checks.push(t_true(true,
        "the original program source can be restored without rebuilding its subject"));
    this.checks.push(t_true(find_path(SUBJECT_PATH) == this.program_switch_subject,
        "program replacement does not recreate the physical subject"));
    // Wait for the restored program's lifecycle start before delivering a
    // one-shot scene event. A semantic event is not a durable command and
    // must not be sent into a program that is still being replaced.
    // Exercise the authoring race directly: ApplyUsdOps completes on the
    // document before the canonical composed stage has projected the new
    // prim. All three edits must still use the authored document target,
    // without surfacing a transient missing-path notification.
    let admission = cmd("ApplyUsdOps", #{
        doc_id: this.doc,
        parent_gen: route_revision(this.doc),
        label: "Admit references in authored order",
        ops: [
            #{ AddPrim: #{
                edit_target: "@runtime@",
                parent_path: ROOT,
                name: "AdmissionLanding",
                type_name: "Xform",
                reference: "lunco://vessels/markers/landing_location.usda",
                reference_prim_path: "",
            }},
            #{ AddPrim: #{
                edit_target: "@runtime@",
                parent_path: ROOT,
                name: "AdmissionPoint",
                type_name: "Xform",
                reference: "lunco://markers/route_point.usda",
                reference_prim_path: "",
            }},
            #{ SetTranslate: #{
                edit_target: "@runtime@",
                path: ADMISSION_LANDING_PATH,
                value: [1000.0, 1000.0, 1000.0],
            }},
            #{ SetTranslate: #{
                edit_target: "@runtime@",
                path: ADMISSION_POINT_PATH,
                value: [1000.0, 1000.0, 1000.0],
            }},
        ],
    });
    if admission == () || admission.ok != true {
        let reason = if admission == () { "no result" } else { admission.error };
        this.checks.push(t_true(false,
            "primary-scene reference admission was rejected: "
                + reason));
        this = finish(this);
        return;
    }
    this.reference_admission_started = true;
    this.reference_admission_start_tick = sim_tick();
    this.route_source_restore_pending = false;
    this.route_step_restore_program_source_finished = true;
    return;
}

fn route_step_restore_program_source_complete(me) { this.done || this.route_step_restore_program_source_finished == true }


fn route_step_transient_reference_edit_action(me) {
    if this.done { return; }
    this.waiting_for = "transient route-point authoring";
    if !this.reference_admission_checked { return; }
    let transient = waypoint_editor::on_click(
        context(this.doc, (), [0.0, 1.0, -40.0]),
    );
    if transient.ok != true || transient.path == () {
        this.checks.push(t_true(false,
            "a newly added route point is available to the edit policy immediately"));
        this = finish(this);
        return;
    }
    let transient_path = transient.path;
    let unsettled = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: transient_path,
    });
    this.checks.push(t_true(unsettled != () && unsettled.ok == false,
        "a document-scoped composed query rejects a route edit before projection"));
    let transient_move = waypoint_editor::on_move(
        context(this.doc, transient_path, [1.0, 1.0, -41.0]),
    );
    this.checks.push(t_true(transient_move.ok != true
        && transient_move.retryable == true,
        "an unprojected route point reports a retryable move until its live entity exists"));
    let transient_delete = waypoint_editor::on_delete(
        context(this.doc, transient_path, ()),
    );
    this.checks.push(t_true(transient_delete.ok == true,
        "a newly added route point can be deleted before stage projection settles"));
    this.route_step_transient_reference_edit_finished = true;
    return;
}

fn route_step_transient_reference_edit_complete(me) { this.done || this.route_step_transient_reference_edit_finished == true }


fn route_step_append_route_point_action(me) {
    if this.done { return; }
    this.waiting_for = "the transient point deletion projection";
    // Let the disposable ribbon observe the completed delete before the
    // durable point used by the rest of this lifecycle is appended.
    if point_count(this.doc) != 3 || !ribbon_present(this.doc) { return; }
    let pointer = scene_interaction::on_pointer(
        context(this.doc, (), [0.0, 1.0, -32.0]),
    );
    if pointer.ok != true {
        this.checks.push(t_true(false, "generic pointer policy rejected the add-point intent"));
        this = finish(this);
        return;
    }
    this.new_point = pointer.path;
    this.route_step_append_route_point_finished = true;
    return;
}

fn route_step_append_route_point_complete(me) { this.done || this.route_step_append_route_point_finished == true }


fn route_step_route_point_policy_action(me) {
    if this.done { return; }
    this.waiting_for = "the appended route-point projection and pointer policy";
    if point_count(this.doc) != 4 || !has_path(this.doc, this.new_point)
        || !ribbon_present(this.doc) { return; }
    let position = point_position(this.doc, this.new_point);
    let nested = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: this.new_point + "/Dome",
    });
    let authored = query("ResolveUsdTarget", #{
        doc_id: this.doc,
        path: this.new_point,
        edit_target: "@runtime@",
    });
    let ribbon_policy = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: RIBBON_GEOMETRY,
        attrs: ["lunco:interaction:left", "lunco:interaction:right"],
        schemas: true,
    });
    if ribbon_policy == () || ribbon_policy.ok == false
        || ribbon_policy.api_schemas == () || ribbon_policy.attrs == ()
        || !ribbon_policy.api_schemas.contains("LunCoPointerInteractionAPI")
        || ribbon_policy.attrs["lunco:interaction:left"] != "pass_through"
        || ribbon_policy.attrs["lunco:interaction:right"] != "context" { return; }
    this.checks.push(t_true(true,
        "the add-point policy appends a named route point"));
    this.checks.push(t_true(position != () && position[2] == -32.0,
        "appended point has the clicked world position"));
    this.checks.push(t_true(nested != () && nested.ok != false,
        "a route point reference composes its marker subtree under a referenced route parent"));
    this.checks.push(t_true(authored != () && authored.ok != false
        && authored.authored_here == true,
        "a nested route-point spawn is authored in the runtime layer of the canonical document"));
    this.checks.push(t_true(ribbon_policy != () && ribbon_policy.ok != false
        && ribbon_policy.api_schemas != ()
        && ribbon_policy.api_schemas.contains("LunCoPointerInteractionAPI")
        && ribbon_policy.attrs != ()
        && ribbon_policy.attrs["lunco:interaction:left"] == "pass_through"
        && ribbon_policy.attrs["lunco:interaction:right"] == "context",
        "the view-layer ribbon passes primary hits through and accepts secondary context"));
    let ribbon_context = context(this.doc, RIBBON_GEOMETRY, ());
    ribbon_context.button = "secondary";
    ribbon_context.pointer_intents = ["route.context"];
    ribbon_context.pointer_policy = #{ right: "context" };
    ribbon_context.prim_paths = [
        RIBBON_GEOMETRY, ROUTE + "/RouteRibbon", ROUTE, ROOT,
    ];
    let ribbon_action = waypoint_editor::route_pointer_action(ribbon_context);
    this.checks.push(t_true(ribbon_action.handled == true
        && ribbon_action.path == RIBBON_GEOMETRY,
        "right-clicking the route ribbon routes to its authored add-point menu policy"));
    let secondary = context(this.doc, this.new_point, ());
    secondary.button = "secondary";
    secondary.modifiers = #{ alt: false, shift: false, ctrl: false };
    secondary.pointer_intents = ["route.context"];
    secondary.pointer_policy = #{ right: "block" };
    let denied = scene_interaction::on_pointer(secondary);
    this.checks.push(t_true(denied.handled == false,
        "a pointer chord cannot open route context without the USD-authored context policy"));
    secondary.pointer_policy = #{ right: "context" };
    if !is_unattended() {
        let selection = query("InspectSelection", #{});
        this.selection_before_context = if selection != () && selection.ok != false {
            selection.primary
        } else { () };
    }
    let pointer = scene_interaction::on_pointer(secondary);
    this.checks.push(t_true(pointer.ok == true && pointer.selected == false,
        "secondary-click opens waypoint context without selecting it (ok="
            + pointer.ok.to_string() + ", selected=" + pointer.selected.to_string()
            + ", handled=" + pointer.handled.to_string() + ")"));
    this.route_step_route_point_policy_finished = true;
    return;
}

fn route_step_route_point_policy_complete(me) { this.done || this.route_step_route_point_policy_finished == true }


fn route_step_context_selection_action(me) {
    if this.done { return; }
    this.waiting_for = "the editor selection after route context";
    if is_unattended() {
        this.checks.push(t_true(true,
            "secondary-click validates the route context in unattended mode"));
        this.route_step_context_selection_finished = true;
        return;
    }
    let selection = query("InspectSelection", #{});
    if selection == () || selection.ok == false
        || selection.primary != this.selection_before_context { return; }
    this.checks.push(t_true(true,
        "secondary-click preserves the existing scene selection"));
    this.route_step_context_selection_finished = true;
    return;
}

fn route_step_context_selection_complete(me) { this.done || this.route_step_context_selection_finished == true }


fn route_step_move_preview_action(me) {
    if this.done { return; }
    this.waiting_for = "the route move preview projection";
    if this.move_preview_started {
        if find_path(ROUTE + "/MovePreview") < 0 { return; }
        let point_parts = this.new_point.split("/");
        let target = query("ResolveUsdTarget", #{
            doc_id: this.doc,
            path: ROUTE + "/MovePreview/Target_"
                + point_parts[point_parts.len() - 1],
            edit_target: "@view@",
        });
        if target == () || target.ok == false || target.authored_here != true { return; }
        if !this.move_preview_update_sent {
            let preview = #{
                doc_id: this.doc,
                scene_root_path: ROOT,
                route_path: ROUTE,
                selected_path: this.new_point,
                surface_world_position: point3(this.move_preview_expected, "active_physics"),
                world_position: point3(this.move_preview_expected, "active_physics"),
            };
            let shown = waypoint_editor::on_pointer_move(preview);
            if shown.ok != true {
                this.checks.push(t_true(false,
                    "armed route-point hover updates a disposable move ghost in the view layer"));
                this = finish(this);
                return;
            }
            this.move_preview_update_sent = true;
            return;
        }
        let ghost = query("ResolveUsdTarget", #{
            doc_id: this.doc,
            path: ROUTE + "/MovePreview",
            edit_target: "@view@",
        });
        if ghost == () || ghost.ok == false || ghost.authored_here != true { return; }
        this.checks.push(t_true(true,
            "the explicit Move action selects and arms the point with a disposable view marker"));
        this.checks.push(t_true(true,
            "armed route-point hover updates a disposable move ghost in the view layer"));
        this.route_step_move_preview_finished = true;
        return;
    }
    if find_path(this.new_point) < 0 { return; }
    let selected = waypoint_editor::context_route_point(
        context(this.doc, this.new_point, ()), ROUTE,
    );
    this.checks.push(t_true(selected == this.new_point,
        "secondary-click policy resolves the direct route point independently of selection"));
    let nested = context(this.doc, this.new_point + "/Dome", ());
    nested.target_path = this.new_point + "/Dome";
    nested.hit_path = this.new_point + "/Dome";
    nested.prim_paths = [this.new_point + "/Dome", this.new_point];
    let selected_nested = waypoint_editor::context_route_point(nested, ROUTE);
    this.checks.push(t_true(selected_nested == this.new_point,
        "secondary-click resolves nested marker visuals to the canonical route point"));
    let destination = [6.0, 1.0, -36.0];
    let preview = #{
        doc_id: this.doc,
        scene_root_path: ROOT,
        route_path: ROUTE,
        selected_path: this.new_point,
        surface_world_position: point3(destination, "active_physics"),
        world_position: point3(destination, "active_physics"),
    };
    let unarmed_hover = waypoint_editor::on_pointer_move(preview);
    let unarmed_ghost = query("ResolveUsdTarget", #{
        doc_id: this.doc,
        path: ROUTE + "/MovePreview",
        edit_target: "@view@",
    });
    this.checks.push(t_true(unarmed_hover.ok == true
        && (unarmed_ghost == () || unarmed_ghost.ok == false
            || unarmed_ghost.authored_here != true),
        "hovering a selected point does not arm route movement"));
    let move_action = waypoint_editor::on_move_begin(
        context(this.doc, this.new_point, ()),
    );
    if move_action.ok != true || move_action.armed != true {
        this.checks.push(t_true(false,
            "the explicit Move action arms the selected route point"));
        this = finish(this);
        return;
    }
    this.checks.push(t_true(move_action.ok == true && move_action.armed == true,
        "the explicit Move action arms the selected route point"));
    this.move_preview_expected = destination;
    this.move_preview_started = true;
}

fn route_step_move_preview_complete(me) { this.done || this.route_step_move_preview_finished == true }


fn route_step_commit_move_action(me) {
    if this.done { return; }
    this.waiting_for = "the view-layer move target";
    // The placement hit can arrive before the disposable preview has been
    // projected into the live scene. Resolve its target from the view
    // layer and apply the click without waiting for composed reads.
    let point_parts = this.new_point.split("/");
    let target_path = ROUTE + "/MovePreview/Target_"
        + point_parts[point_parts.len() - 1];
    let preview = query("ResolveUsdTarget", #{
        doc_id: this.doc,
        path: ROUTE + "/MovePreview",
        edit_target: "@view@",
        authored_children: true,
    });
    let preview_target_found = false;
    if preview != () && preview.ok != false && preview.authored_children != () {
        for child in preview.authored_children {
            if child == target_path { preview_target_found = true; }
        }
    }
    this.checks.push(t_true(preview_target_found,
        "the transient move target resolves directly from its authored view layer"));
    let destination = this.move_preview_expected;
    let place = context(this.doc, (), destination);
    place.active_pointer_move = #{
        interaction_id: waypoint_editor::move_interaction_id(ROUTE),
        tool: "scene_interaction", hook: "on_pointer_move",
        context: #{ route_path: ROUTE },
    };
    place.selected_path = this.new_point;
    place.target_path = ();
    place.hit_path = ();
    place.prim_paths = [];
    place.pointer_intents = ["selection.replace"];
    place.modifiers = #{ alt: false, shift: false, ctrl: false };
    let moved = scene_interaction::on_pointer(place);
    this.checks.push(t_true(moved.ok == true && moved.handled == true
        && moved.path == this.new_point,
        "the next scene hit moves the explicitly armed route point before generic selection"));
    this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
        "moving a selected point preserves the live subject identity"));
    this.route_step_commit_move_finished = true;
    return;
}

fn route_step_commit_move_complete(me) { this.done || this.route_step_commit_move_finished == true }


fn route_step_move_projection_action(me) {
    if this.done { return; }
    this.waiting_for = "the moved route-point projection";
    let position = point_position(this.doc, this.new_point);
    let ghost = query("ResolveUsdTarget", #{
        doc_id: this.doc,
        path: ROUTE + "/MovePreview",
        edit_target: "@view@",
    });
    if position == () || position[0] != 6.0 || position[2] != -36.0
        || !ribbon_present(this.doc) { return; }
    this.checks.push(t_true(ghost == () || ghost.ok == false
        || ghost.authored_here != true,
        "committing the move removes its transient ghost"));
    this.route_step_move_projection_finished = true;
    return;
}

fn route_step_move_projection_complete(me) { this.done || this.route_step_move_projection_finished == true }


fn route_step_delete_appended_point_action(me) {
    if this.done { return; }
    this.waiting_for = "the appended route-point deletion projection";
    let position = point_position(this.doc, this.new_point);
    if position == () || position[0] != 6.0 || position[2] != -36.0
        || !ribbon_present(this.doc) { return; }
    this.checks.push(t_true(true,
        "moving a selected point refreshes the reusable ribbon in place"));
    let deleted = waypoint_editor::on_delete(context(this.doc, this.new_point, ()));
    this.checks.push(t_true(deleted.ok == true && deleted.path == this.new_point,
        "delete removes an authored route point"));
    this.route_step_delete_appended_point_finished = true;
    return;
}

fn route_step_delete_appended_point_complete(me) { this.done || this.route_step_delete_appended_point_finished == true }


fn route_step_move_referenced_point_action(me) {
    if this.done { return; }
    this.waiting_for = "the referenced route-point move projection";
    if point_count(this.doc) != 3 || has_path(this.doc, this.new_point)
        || !ribbon_present(this.doc) { return; }
    this.checks.push(t_true(true, "deleting a point updates ribbon topology"));
    this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
        "deleting a point does not rebuild the subject"));
    let moved = waypoint_editor::on_move(context(
        this.doc, ROUTE + "/P2", [2.0, 1.0, -26.0],
    ));
    this.checks.push(t_true(moved.ok == true && moved.path == ROUTE + "/P2",
        "moving a referenced route point authors a local USD transform opinion"));
    this.route_step_move_referenced_point_finished = true;
    return;
}

fn route_step_move_referenced_point_complete(me) { this.done || this.route_step_move_referenced_point_finished == true }


fn route_step_undo_referenced_move_action(me) {
    if this.done { return; }
    this.waiting_for = "undo of the referenced point transform";
    let position = point_position(this.doc, ROUTE + "/P2");
    if position == () || position[0] != 2.0 || position[2] != -26.0
        || !ribbon_present(this.doc) { return; }
    this.checks.push(t_true(true,
        "moving a referenced route point updates its composed ribbon"));
    this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
        "moving a referenced point preserves the subject identity"));
    let undone = cmd("UndoDocument", #{ doc_id: this.doc });
    if undone == () || undone.ok != true { return; }
    this.route_step_undo_referenced_move_finished = true;
    return;
}

fn route_step_undo_referenced_move_complete(me) { this.done || this.route_step_undo_referenced_move_finished == true }


fn route_step_redo_referenced_move_action(me) {
    if this.done { return; }
    this.waiting_for = "redo of the referenced point transform";
    let position = point_position(this.doc, ROUTE + "/P2");
    if position == () || position[0] != 0.0 || position[2] != -24.0
        || !ribbon_present(this.doc) { return; }
    this.checks.push(t_true(true,
        "undo removes only the local transform opinion from a referenced point"));
    this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
        "undoing a referenced transform preserves the subject identity"));
    let redone = cmd("RedoDocument", #{ doc_id: this.doc });
    if redone == () || redone.ok != true { return; }
    this.route_step_redo_referenced_move_finished = true;
    return;
}

fn route_step_redo_referenced_move_complete(me) { this.done || this.route_step_redo_referenced_move_finished == true }


fn route_step_delete_referenced_point_action(me) {
    if this.done { return; }
    this.waiting_for = "the referenced route-point deletion";
    let position = point_position(this.doc, ROUTE + "/P2");
    if position == () || position[0] != 2.0 || position[2] != -26.0
        || !ribbon_present(this.doc) { return; }
    this.checks.push(t_true(true,
        "redo reapplies the referenced transform without scene reprojection"));
    this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
        "redoing a referenced transform preserves the subject identity"));
    let deleted = waypoint_editor::on_delete(context(this.doc, ROUTE + "/P2", ()));
    this.checks.push(t_true(deleted.ok == true,
        "referenced route point uses a local USD active override on delete"));
    this.route_step_delete_referenced_point_finished = true;
    return;
}

fn route_step_delete_referenced_point_complete(me) { this.done || this.route_step_delete_referenced_point_finished == true }


fn route_step_undo_referenced_delete_action(me) {
    if this.done { return; }
    this.waiting_for = "undo of the referenced point deletion";
    if point_count(this.doc) != 2 || has_path(this.doc, ROUTE + "/P2")
        || !ribbon_present(this.doc) { return; }
    this.checks.push(t_true(true, "deactivating a referenced point does not rebuild the subject"));
    let undone = cmd("UndoDocument", #{ doc_id: this.doc });
    if undone == () || undone.ok != true { return; }
    this.route_step_undo_referenced_delete_finished = true;
    return;
}

fn route_step_undo_referenced_delete_complete(me) { this.done || this.route_step_undo_referenced_delete_finished == true }


fn route_step_redo_referenced_delete_action(me) {
    if this.done { return; }
    this.waiting_for = "redo of the referenced point deletion";
    if point_count(this.doc) != 3 || !has_path(this.doc, ROUTE + "/P2")
        || !ribbon_present(this.doc) { return; }
    this.checks.push(t_true(true,
        "undo restores a referenced route point and its ribbon"));
    this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
        "undoing a referenced route edit preserves the subject identity"));
    let redone = cmd("RedoDocument", #{ doc_id: this.doc });
    if redone == () || redone.ok != true { return; }
    this.route_step_redo_referenced_delete_finished = true;
    return;
}

fn route_step_redo_referenced_delete_complete(me) { this.done || this.route_step_redo_referenced_delete_finished == true }


fn route_step_delete_remaining_referenced_point_action(me) {
    if this.done { return; }
    this.waiting_for = "the remaining referenced-point deletion";
    if point_count(this.doc) != 2 || has_path(this.doc, ROUTE + "/P2")
        || !ribbon_present(this.doc) { return; }
    this.checks.push(t_true(true,
        "redo reapplies the referenced route-point deletion"));
    this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
        "redoing a referenced route edit preserves the subject identity"));
    let remaining_deleted = waypoint_editor::on_delete(
        context(this.doc, ROUTE + "/P1", ()),
    );
    this.checks.push(t_true(remaining_deleted.ok == true,
        "the remaining route point accepts a persistent delete"));
    if remaining_deleted.ok != true {
        this = finish(this);
        return;
    }
    this.route_step_delete_remaining_referenced_point_finished = true;
    return;
}

fn route_step_delete_remaining_referenced_point_complete(me) { this.done || this.route_step_delete_remaining_referenced_point_finished == true }


fn route_step_one_point_route_action(me) {
    if this.done { return; }
    this.waiting_for = "the one-point route projection";
    if point_count(this.doc) != 1 || has_path(this.doc, ROUTE + "/P1")
        || find_path(RIBBON_GEOMETRY) != this.ribbon_entity { return; }
    this.checks.push(t_true(true,
        "a one-point route keeps the stable ribbon identity"));
    waypoint_editor::on_delete(context(this.doc, ROUTE + "/P0", ()));
    this.route_step_one_point_route_finished = true;
    return;
}

fn route_step_one_point_route_complete(me) { this.done || this.route_step_one_point_route_finished == true }


fn route_step_empty_route_action(me) {
    if this.done { return; }
    this.waiting_for = "the empty route projection";
    let points = direct_points(this.doc);
    if points == () || points.len() != 0
        || find_path(RIBBON_GEOMETRY) != this.ribbon_entity { return; }
    let route_projection = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: ROUTE,
        children: true,
    });
    if route_projection == () || route_projection.ok == false
        || route_projection.children == () { return; }
    this.checks.push(t_true(true,
        "an empty route retains its reusable ribbon identity"));
    let selected = program_editor::select(this.program_path);
    if selected.ok != true {
        this.checks.push(t_true(false,
            "the route program can be selected after its last point is removed"));
        this = finish(this);
        return;
    }
    this.route_step_empty_route_finished = true;
    return;
}

fn empty_route_program_is_selected_and_current(me) {
    if this.done { return false; }
    let points = direct_points(this.doc);
    if points == () || points.len() != 0 { return false; }
    let route_projection = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: ROUTE,
        children: true,
    });
    if route_projection == () || route_projection.ok == false
        || route_projection.children == () { return false; }
    let editor_selection = query("InspectUsdSelection", #{});
    if editor_selection != () && editor_selection.ok != false
        && editor_selection.focused == true && editor_selection.doc_id == this.doc {
        return editor_selection.primary != ()
            && editor_selection.primary.path == this.program_path;
    }
    let entity = find_path_in_document(this.doc, this.program_path);
    if entity < 0 { return false; }
    let selection = query("InspectSelection", #{});
    selection != () && selection.ok != false
        && selection.primary.to_string() == entity.to_string()
        && selection.selected != () && selection.selected.len() == 1
        && selection.selected[0].to_string() == entity.to_string()
}

fn route_step_readd_empty_route_action(me) {
    if this.done || this.route_step_readd_empty_route_finished == true { return; }
    this.waiting_for = "the selected program re-adding the first point";
    let add_context = context(this.doc, (), [0.0, 1.0, -8.0]);
    add_context.route_path = ();
    add_context.selected_path = this.program_path;
    let resolved = waypoint_editor::route_edit_path(add_context);
    this.checks.push(t_true(resolved == ROUTE,
        "the selected empty-route program resolves its route scope: " + resolved.to_string()));
    if resolved != ROUTE { this = finish(this); return; }
    let added = waypoint_editor::on_click(add_context);
    this.checks.push(t_true(added.ok == true && added.path != (),
        "a selected route program accepts a new point after the route becomes empty"));
    if added.ok != true {
        this = finish(this);
        return;
    }
    this.route_step_readd_empty_route_finished = true;
    return;
}

fn route_step_empty_route_complete(me) { this.done || this.route_step_empty_route_finished == true }
fn route_step_readd_empty_route_complete(me) { this.done || this.route_step_readd_empty_route_finished == true }


fn route_step_first_new_point_action(me) {
    if this.done { return; }
    this.waiting_for = "the first new route-point projection";
    if point_count(this.doc) != 1
        || find_path(RIBBON_GEOMETRY) != this.ribbon_entity { return; }
    let added = waypoint_editor::on_click(context(this.doc, (), [0.0, 1.0, -16.0]));
    this.checks.push(t_true(added.ok == true && added.path != (),
        "the route accepts a second point after rebuilding an empty path"));
    if added.ok != true {
        this = finish(this);
        return;
    }
    this.route_step_first_new_point_finished = true;
    return;
}

fn route_step_first_new_point_complete(me) { this.done || this.route_step_first_new_point_finished == true }


fn route_step_modifier_noop_action(me) {
    if this.done { return; }
    this.waiting_for = "the second new route-point projection";
    if point_count(this.doc) != 2 || !ribbon_present(this.doc) { return; }
    let modified = modified_context(this.doc, (), [0.0, 1.0, -24.0],
        #{ alt: true, shift: true, ctrl: true });
    modified.pointer_intents = [];
    scene_interaction::on_pointer(modified);
    this.route_step_modifier_noop_finished = true;
    return;
}

fn route_step_modifier_noop_complete(me) { this.done || this.route_step_modifier_noop_finished == true }


fn route_step_acquire_subject_action(me) {
    if this.done { return; }
    this.waiting_for = "subject possession";
    if point_count(this.doc) != 2 || !ribbon_present(this.doc) { return; }
    let possession = cmd("AcquireControl", #{
        target: this.subject,
        bind_camera: false,
    });
    this.checks.push(t_true(possession != () && possession.ok != false,
        "clicking the rover establishes the human control target"));
    this.route_step_acquire_subject_finished = true;
    return;
}

fn route_step_acquire_subject_complete(me) { this.done || this.route_step_acquire_subject_finished == true }


fn route_step_start_route_action(me) {
    if this.done { return; }
    this.waiting_for = "the authored route program start";
    if !is_controlled(this.subject) { return; }
    this.checks.push(t_true(owner_of(this.subject) == local_session_id(),
        "the possessed rover is owned by the active session"));
    let edge = intent_pulse(this.subject, "action");
    this.checks.push(t_true(edge != () && edge.ok != false,
        "F is delivered through the generic semantic action edge"));
    this.route_step_start_route_finished = true;
    return;
}

fn route_step_start_route_complete(me) { this.done || this.route_step_start_route_finished == true }


fn route_step_edit_active_route_action(me) {
    if this.done { return; }
    this.waiting_for = "the live route edit and sensor transition";
    if !this.active_edit {
        if point_count(this.doc) != 2 || !ribbon_present(this.doc) { return; }
        let added = waypoint_editor::on_click(
            context(this.doc, (), [0.0, 1.0, -24.0]),
        );
        if added.ok != true {
            this.checks.push(t_true(false,
                "editing a route while it is running appends a point"));
            this = finish(this);
            return;
        }
        this.checks.push(t_true(true,
            "a non-matching modifier combination does not append a route point"));
        this.checks.push(t_true(true,
            "editing a route while it is running appends a point"));
        this.active_edit = true;
        return;
    }
    if point_count(this.doc) != 3 || !ribbon_present(this.doc) { return; }
    if this.started == 0 || this.stopped != 0 { return; }
    let guidance = port(this.subject, "guidance_throttle");
    if guidance == () || guidance.abs() < 0.01 { return; }
    let program_active = port(this.subject, "program_active");
    if program_active == () || program_active < 0.5 { return; }
    let points = direct_points(this.doc);
    if points == () || points.len() == 0 { return; }
    let zone = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: points[0] + "/Trigger",
        attrs: ["lunco:triggerZone"],
    });
    if zone == () || zone.ok == false || zone.attrs == ()
        || zone.attrs["lunco:triggerZone"] == () { return; }
    this.checks.push(t_true(true,
        "an active route keeps running through a live point edit"));
    this.checks.push(t_true(true,
        "F starts the authored program without replacing local possession"));
    this.checks.push(t_true(true,
        "the running route program produces generic drive commands"));
    this.reached_point_path = points[0];
    emit("enter:" + zone.attrs["lunco:triggerZone"], this.subject);
    this.route_step_edit_active_route_finished = true;
    return;
}

fn route_step_edit_active_route_complete(me) { this.done || this.route_step_edit_active_route_finished == true }


fn route_step_route_point_reached_action(me) {
    if this.done { return; }
    this.waiting_for = "the route-point reached event and marker state";
    if this.reached == 0 { return; }
    let reached_color = marker_color(this.doc, this.reached_point_path);
    let points = direct_points(this.doc);
    if reached_color == () || points == () || points.len() < 2 { return; }
    let pending_path = if points[0] == this.reached_point_path { points[1] } else { points[0] };
    let pending_color = marker_color(this.doc, pending_path);
    if !color_matches(reached_color, [0.55, 0.55, 0.55])
        || !color_matches(pending_color, [0.10, 0.95, 0.22]) { return; }
    this.checks.push(t_true(true,
        "visited route points turn gray while unvisited points stay green"));
    this.checks.push(t_true(this.reached_index == 0,
        "the sensor event advances the stable current route point"));
    let manual_edge = intent_pulse(this.subject, "forward");
    if manual_edge == () || manual_edge.ok == false {
        this.checks.push(t_true(false,
            "a possessed rover accepts a semantic manual-control edge"));
        this = finish(this);
        return;
    }
    this.route_step_route_point_reached_finished = true;
    return;
}

fn route_step_route_point_reached_complete(me) { this.done || this.route_step_route_point_reached_finished == true }


fn route_step_manual_route_pause_action(me) {
    if this.done { return; }
    this.waiting_for = "the manual route pause";
    let program_active = port(this.subject, "program_active");
    if this.stopped == 0 || program_active == () || program_active > 0.5 { return; }
    this.checks.push(t_true(is_controlled(this.subject)
        && owner_of(this.subject) == local_session_id(),
        "a possessed rover's non-Action intent yields route guidance without releasing possession"));
    this.stopped = 0;
    let resumed = intent_pulse(this.subject, "action");
    if resumed == () || resumed.ok == false {
        this.checks.push(t_true(false,
            "the possessed rover accepts the route-resume action"));
        this = finish(this);
        return;
    }
    this.route_step_manual_route_pause_finished = true;
    return;
}

fn route_step_manual_route_pause_complete(me) { this.done || this.route_step_manual_route_pause_finished == true }


fn route_step_release_subject_action(me) {
    if this.done { return; }
    this.waiting_for = "the resumed route and subject release";
    let program_active = port(this.subject, "program_active");
    if this.started < 2 || program_active == () || program_active < 0.5 { return; }
    let released = release_control_claim(this.subject);
    if released == () || released.ok == false {
        this.checks.push(t_true(false,
            "the test releases rover possession before exercising non-subject input"));
        this = finish(this);
        return;
    }
    this.route_step_release_subject_finished = true;
    return;
}

fn route_step_release_subject_complete(me) { this.done || this.route_step_release_subject_finished == true }


fn route_step_free_avatar_input_action(me) {
    if this.done { return; }
    this.waiting_for = "the released route and free-avatar intent";
    let program_active = port(this.subject, "program_active");
    let guidance = port(this.subject, "guidance_throttle");
    if is_controlled(this.subject) || program_active == () || program_active < 0.5
        || guidance == () || guidance.abs() < 0.01 { return; }
    // This synthetic non-subject edge has the same self-targeted shape as
    // free-avatar input: source and target_gid match, but neither is rover.
    let free_input = emit("intent.edge", #{
        intent: "forward",
        edge: "pressed",
        target_gid: me,
    });
    if !free_input {
        this.checks.push(t_true(false,
            "the test publishes the free-avatar forward edge after rover release"));
        this = finish(this);
        return;
    }
    this.route_step_free_avatar_input_finished = true;
    return;
}

fn route_step_free_avatar_input_complete(me) { this.done || this.route_step_free_avatar_input_finished == true }


fn route_step_route_after_free_input_action(me) {
    if this.done { return; }
    this.waiting_for = "the route response to free-avatar intent";
    let program_active = port(this.subject, "program_active");
    let guidance = port(this.subject, "guidance_throttle");
    if is_controlled(this.subject) || program_active == () { return; }
    this.checks.push(t_true(this.stopped == 0 && program_active > 0.5
        && guidance != () && guidance.abs() > 0.01,
        "an unpossessed self-targeted forward edge leaves rover route guidance active"));
    let possession = cmd("AcquireControl", #{
        target: this.subject,
        bind_camera: false,
    });
    if possession == () || possession.ok == false {
        this.checks.push(t_true(false,
            "the test reacquires the rover after checking free-avatar input"));
        this = finish(this);
        return;
    }
    this.route_step_route_after_free_input_finished = true;
    return;
}

fn route_step_route_after_free_input_complete(me) { this.done || this.route_step_route_after_free_input_finished == true }


fn route_step_hud_route_toggle_action(me) {
    if this.done { return; }
    this.waiting_for = "subject reacquisition and HUD action";
    if !is_controlled(this.subject)
        || owner_of(this.subject) != local_session_id() { return; }
    this.checks.push(t_true(true,
        "reacquiring the rover preserves its active route after free-avatar input"));
    let hud_toggle = emit("runtime.ui.action", #{ action: "program.toggle", parameters: #{} });
    this.checks.push(t_true(hud_toggle,
        "the authored program HUD dispatches a generic toggle action"));
    this.route_step_hud_route_toggle_finished = true;
    return;
}

fn route_step_hud_route_toggle_complete(me) { this.done || this.route_step_hud_route_toggle_finished == true }


fn route_step_hud_route_stopped_action(me) {
    if this.done { return; }
    this.waiting_for = "the HUD route stop event";
    // The authored HUD action is delivered on the shared event bus; wait
    // for the route program's stop edge before checking the authority
    // release. This keeps the test at the same lifecycle boundary as the
    // production UI and avoids a fixed-delay assertion.
    let program_active = port(this.subject, "program_active");
    if this.stopped == 0 || program_active == () || program_active > 0.5 { return; }
    this.checks.push(t_true(true, "the program HUD toggles the route off"));
    this.checks.push(t_true(is_controlled(this.subject)
        && owner_of(this.subject) == local_session_id(),
        "stopping the route releases program authority without dropping human possession"));
    let started = emit("runtime.ui.action", #{ action: "program.toggle", parameters: #{} });
    this.checks.push(t_true(started, "the program HUD dispatches its on action"));
    this.route_step_hud_route_stopped_finished = true;
    return;
}

fn route_step_hud_route_stopped_complete(me) { this.done || this.route_step_hud_route_stopped_finished == true }


fn route_step_hud_route_started_again_action(me) {
    if this.done { return; }
    this.waiting_for = "the HUD route start event";
    let program_active = port(this.subject, "program_active");
    if this.started < 2 || program_active == () || program_active < 0.5 { return; }
    this.checks.push(t_true(true, "the program HUD toggles the route on"));
    let stopped = emit("runtime.ui.action", #{ action: "program.toggle", parameters: #{} });
    this.checks.push(t_true(stopped, "the program HUD dispatches its off action"));
    this.route_step_hud_route_started_again_finished = true;
    return;
}

fn route_step_hud_route_started_again_complete(me) { this.done || this.route_step_hud_route_started_again_finished == true }


fn route_step_generic_point_delete_action(me) {
    if this.done { return; }
    this.waiting_for = "the second HUD stop and generic deletion";
    let program_active = port(this.subject, "program_active");
    if this.stopped < 2 || program_active == () || program_active > 0.5 { return; }
    this.checks.push(t_true(true, "the program HUD toggles the route off again"));
    let points = direct_points(this.doc);
    if points == () || points.len() == 0 { return; }
    let delete_path = points[0];
    let target = find_path(delete_path);
    if target < 0 { return; }
    this.generic_delete_path = delete_path;
    let deleted = cmd("DeleteEntity", #{ target: target });
    if deleted == () || deleted.ok == false {
        this.checks.push(t_true(false,
            "generic DeleteEntity accepted a selected authored route point"));
        this = finish(this);
        return;
    }
    this.route_step_generic_point_delete_finished = true;
    return;
}

fn route_step_generic_point_delete_complete(me) { this.done || this.route_step_generic_point_delete_finished == true }


fn route_step_select_alternate_program_action(me) {
    if this.done { return; }
    this.waiting_for = "the generic point deletion projection and alternate program";
    if point_count(this.doc) != 2 || has_path(this.doc, this.generic_delete_path)
        || !ribbon_present(this.doc) { return; }
    this.checks.push(t_true(true,
        "generic DeleteEntity persists a runtime active override for an authored point"));
    let alternate = program_editor::list(this.doc, ALTERNATE_ROUTE);
    if alternate == () || !alternate.ok || alternate.programs.len() == 0 { return; }
    this.alternate_program_path = alternate.programs[0]["path"];
    let selected = program_editor::select(this.alternate_program_path);
    if !selected.ok {
        this.checks.push(t_true(false, selected.error));
        this = finish(this);
        return;
    }
    this.route_step_select_alternate_program_finished = true;
    return;
}

fn route_step_select_alternate_program_complete(me) { this.done || this.route_step_select_alternate_program_finished == true }


fn route_step_add_alternate_point_action(me) {
    if this.done { return; }
    this.waiting_for = "alternate-program projection and point creation";
    // Program selection is a typed scene transaction. Wait for its
    // canonical projection before deriving the next authored child name;
    // do not guess from a stale composed-stage read.
    let alternate = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: ALTERNATE_ROUTE,
        children: true,
    });
    if alternate == () || alternate.ok == false || alternate.children == () {
        return;
    }
    let added = waypoint_editor::on_click(
        selected_route_context(this.doc, (), [8.0, 1.0, -16.0], ALTERNATE_ROUTE),
    );
    this.checks.push(t_true(added.ok && added.path == ALTERNATE_ROUTE + "/P1",
        "the selected program chooses which authored route receives a new point"));
    this.route_step_add_alternate_point_finished = true;
    return;
}

fn route_step_add_alternate_point_complete(me) { this.done || this.route_step_add_alternate_point_finished == true }


fn route_step_alternate_route_edit_action(me) {
    if this.done { return; }
    this.waiting_for = "alternate-route query and insertion";
    if !has_path(this.doc, ALTERNATE_ROUTE + "/P1") { return; }
    this.checks.push(t_true(point_count(this.doc) == 2,
        "editing the alternate program does not mutate the first route"));
    let route_read = waypoint_editor::route_positions(this.doc, ALTERNATE_ROUTE);
    if route_read.ok != true || route_read.paths.len() < 2 {
        this.route_read_attempts += 1;
        if this.route_read_attempts < 120 { return; }
        let children = query("QueryUsdPrim", #{
            doc_id: this.doc, path: ALTERNATE_ROUTE, children: true,
        });
        let p0 = query("ResolveUsdTarget", #{
            doc_id: this.doc, path: ALTERNATE_ROUTE + "/P0/Trigger",
            edit_target: "@root@",
        });
        let p1 = query("ResolveUsdTarget", #{
            doc_id: this.doc, path: ALTERNATE_ROUTE + "/P1/Trigger",
            edit_target: "@root@",
        });
        this.checks.push(t_true(false,
            "alternate route read did not settle: route=" + route_read.to_string()
            + "; children=" + children.to_string()
            + "; P0=" + p0.to_string() + "; P1=" + p1.to_string()));
        this = finish(this);
        return;
    }
    this.checks.push(t_true(route_read.paths.contains(ALTERNATE_ROUTE + "/P0")
        && route_read.paths.contains(ALTERNATE_ROUTE + "/P1"),
        "route queries include referenced children and newer local children"));
    let before_context = context(this.doc, ALTERNATE_ROUTE + "/P1", ());
    before_context.route_path = ALTERNATE_ROUTE;
    let before = waypoint_editor::on_insert_before(before_context);
    this.checks.push(t_true(before.ok == true && before.path == ALTERNATE_ROUTE + "/P2",
        "Add point before creates an ordinary point with a stable new path"));
    if before.ok != true { this = finish(this); return; }
    this.insert_before_path = before.path;
    this.route_step_alternate_route_edit_finished = true;
    return;
}

fn route_step_alternate_route_edit_complete(me) { this.done || this.route_step_alternate_route_edit_finished == true }


fn route_step_verify_insert_before_action(me) {
    if this.done { return; }
    this.waiting_for = "the insert-before projection";
    let children = query("QueryUsdPrim", #{
        doc_id: this.doc, path: ALTERNATE_ROUTE, children: true,
    });
    if children == () || children.ok == false || children.children == ()
        || !children.children.contains(this.insert_before_path) { return; }
    let before_index = child_index(children.children, this.insert_before_path);
    let target_index = child_index(children.children, ALTERNATE_ROUTE + "/P1");
    this.checks.push(t_true(before_index >= 0 && target_index > before_index,
        "USD primOrder places an inserted waypoint before its target"));
    let before_position = point_position(this.doc, this.insert_before_path);
    this.checks.push(t_true(before_position != () && before_position[2] == -12.0,
        "Add point before places the new marker at the adjacent-point midpoint"));
    let after_context = context(this.doc, ALTERNATE_ROUTE + "/P1", ());
    after_context.route_path = ALTERNATE_ROUTE;
    let after = waypoint_editor::on_insert_after(after_context);
    this.checks.push(t_true(after.ok == true && after.path == ALTERNATE_ROUTE + "/P3",
        "Add point after creates the next stable route-point path"));
    if after.ok != true { this = finish(this); return; }
    this.insert_after_path = after.path;
    this.route_step_verify_insert_before_finished = true;
    return;
}

fn route_step_verify_insert_before_complete(me) { this.done || this.route_step_verify_insert_before_finished == true }


fn route_step_verify_insert_after_action(me) {
    if this.done { return; }
    this.waiting_for = "the insert-after projection and ribbon action";
    let children = query("QueryUsdPrim", #{
        doc_id: this.doc, path: ALTERNATE_ROUTE, children: true,
    });
    if children == () || children.ok == false || children.children == ()
        || !children.children.contains(this.insert_after_path) { return; }
    let target_index = child_index(children.children, ALTERNATE_ROUTE + "/P1");
    let after_index = child_index(children.children, this.insert_after_path);
    this.checks.push(t_true(target_index >= 0 && after_index > target_index,
        "USD primOrder places an inserted waypoint after its target"));
    let adjacent_position = point_position(this.doc, ALTERNATE_ROUTE + "/P1");
    let after_position = point_position(this.doc, this.insert_after_path);
    let extension = point_distance(adjacent_position, after_position);
    this.checks.push(t_true(extension != () && (extension - 5.0).abs() < 0.01
        && after_position[2] < adjacent_position[2],
        "Add point after extends five meters outward along the final segment"));
    let ribbon_context = context(
        this.doc, ALTERNATE_ROUTE + "/RouteRibbon/Geometry", [14.0, 1.0, -25.0],
    );
    ribbon_context.route_path = ();
    let ribbon_add = waypoint_editor::on_add_here(ribbon_context);
    this.checks.push(t_true(ribbon_add.ok == true
        && ribbon_add.path == ALTERNATE_ROUTE + "/P4",
        "the ribbon context action adds a route point at its typed pointer position"));
    if ribbon_add.ok != true { this = finish(this); return; }
    this.ribbon_added_path = ribbon_add.path;
    this.route_step_verify_insert_after_finished = true;
    return;
}

fn route_step_verify_insert_after_complete(me) { this.done || this.route_step_verify_insert_after_finished == true }


fn route_step_verify_ribbon_add_action(me) {
    if this.done { return; }
    this.waiting_for = "the ribbon-add point projection";
    let children = query("QueryUsdPrim", #{
        doc_id: this.doc, path: ALTERNATE_ROUTE, children: true,
    });
    if children == () || children.ok == false || children.children == ()
        || !children.children.contains(this.ribbon_added_path) { return; }
    let inserted_index = child_index(children.children, this.ribbon_added_path);
    let last_route_point_index = child_index(children.children, this.insert_after_path);
    let position = point_position(this.doc, this.ribbon_added_path);
    this.checks.push(t_true(inserted_index > last_route_point_index
        && position != () && position[0] == 14.0 && position[2] == -25.0,
        "the ribbon action appends after the route and preserves its pointer coordinates"));
    this.route_step_verify_ribbon_add_finished = true;
    return;
}

fn route_step_verify_ribbon_add_complete(me) { this.done || this.route_step_verify_ribbon_add_finished == true }


fn route_step_application_projection_admission_action(me) {
    if this.done { return; }
    this.waiting_for = "application/Repl reference admission";
    let primary_program = program_editor::list(this.doc, ROUTE);
    if primary_program == () || !primary_program.ok
        || primary_program.programs.len() == 0 { return; }
    let possessed_pointer = context(
        this.doc, ALTERNATE_ROUTE + "/P1/Dome", (),
    );
    possessed_pointer.route_path = ();
    possessed_pointer.selected_path = primary_program.programs[0]["path"];
    possessed_pointer.controlled_path = SUBJECT_PATH;
    possessed_pointer.target_path = ALTERNATE_ROUTE + "/P1";
    possessed_pointer.hit_path = ALTERNATE_ROUTE + "/P1/Dome";
    possessed_pointer.prim_paths = [
        ALTERNATE_ROUTE + "/P1/Dome", ALTERNATE_ROUTE + "/P1",
        ALTERNATE_ROUTE, ROOT,
    ];
    possessed_pointer.button = "secondary";
    possessed_pointer.pointer_intents = ["route.context"];
    possessed_pointer.pointer_policy = #{ right: "context" };
    let possessed_action = waypoint_editor::route_pointer_action(possessed_pointer);
    this.checks.push(t_true(possessed_action.handled == true
        && possessed_action.path == ALTERNATE_ROUTE + "/P1",
        "a possessed right-click follows the hit waypoint instead of the previously selected route"));
    let generation = route_revision(this.doc);
    if generation == () {
        this.checks.push(t_true(false,
            "Application/Repl projection admission requires the current document generation"));
        this = finish(this);
        return;
    }
    let code = "let edit = cmd(\"ApplyUsdOps\", #{ doc_id: "
        + this.doc.to_string()
        + ", parent_gen: " + generation.to_string()
        + ", label: \"Application Repl projection admission\", ops: [#{ AddPrim: #{"
        + " edit_target: \"@runtime@\", parent_path: \"/RouteLifecycle\","
        + " name: \"ApplicationAdmissionMarker\", type_name: \"Xform\","
        + " reference: \"lunco://vessels/markers/predicted_landing.usda\","
        + " reference_prim_path: \"\" }}] });"
        + " let runtime = execution_context(); let clocks = clock_snapshot();"
        + " emit(\"" + APPLICATION_ADMISSION_EVENT + "\", #{ accepted: edit.ok,"
        + " scope: runtime.scope, cycle: runtime.cycle, phase: runtime.phase,"
        + " clock: runtime.clock, sequence: runtime.sequence, tick: clocks.sim_tick,"
        + " entity_indexed: find_path(\"" + APPLICATION_ADMISSION_PATH + "\") >= 0 });";
    this.application_admission_pending = true;
    let queued = cmd("RunRhai", #{ code: code });
    this.checks.push(t_true(queued != () && queued.ok == true,
        "the Application/Repl script is admitted through the public command queue"));
    if queued == () || queued.ok != true { this = finish(this); return; }
    this.route_step_application_projection_admission_finished = true;
    return;
}

fn route_step_application_projection_admission_complete(me) { this.done || this.route_step_application_projection_admission_finished == true }


fn application_projection_tick_is_admitted(me) {
    this.done || (this.application_admission_received
        && sim_tick() > this.application_admission_event_tick)
}

fn route_step_application_projection_verdict_action(me) {
    if this.done { return; }
    this.waiting_for = "the application/Repl projection and event verdict";
    if !this.application_admission_first_tick_checked {
        let projected = find_path(APPLICATION_ADMISSION_PATH) >= 0;
        this.checks.push(t_true(projected,
            "the first simulation tick after an Application/Repl USD edit sees its referenced prim projected"));
        this.application_admission_first_tick_checked = true;
        if !projected { this = finish(this); return; }
        return;
    }
    if !this.application_admission_received { return; }
    this.checks.push(t_true(this.application_admission_accepted,
        "the Application/Repl USD edit is accepted by the typed document command"));
    this.checks.push(t_true(this.application_admission_context_ok,
        "the edit and its event retain Application/Repl evaluation context"));
    this.checks.push(t_true(this.application_admission_event_tick_matches,
        "the application-produced event tick matches its producer boundary (payload="
            + this.application_admission_payload_tick.to_string() + ", event="
            + this.application_admission_event_tick.to_string() + ")"));
    this.checks.push(t_true(this.application_admission_entity_unindexed,
        "entity lookup has not indexed the referenced prim immediately after the Application/Repl document edit"));
    this.checks.push(t_true(this.application_admission_pause_observed,
        "pending Application/Repl projection pauses simulation admission"));
    this = finish(this);
}

fn route_step_application_projection_verdict_complete(me) { this.done || this.route_step_application_projection_verdict_finished == true }


fn route_lifecycle_steps() {
    seq([
        once(Fn("record_first_behavior_tick")),

        step(Fn("route_step_route_fixture_action"), Fn("route_step_route_fixture_complete")),
        step(Fn("route_step_hud_projection_action"), Fn("route_step_hud_projection_complete")),
        step(Fn("route_step_switch_program_source_action"), Fn("route_step_switch_program_source_complete")),
        step(Fn("route_step_replacement_program_ready_action"), Fn("route_step_replacement_program_ready_complete")),
        step(Fn("route_step_restore_program_source_action"), Fn("route_step_restore_program_source_complete")),
        wait_until(Fn("reference_admission_projection_checked")),
        step(Fn("route_step_transient_reference_edit_action"), Fn("route_step_transient_reference_edit_complete")),
        step(Fn("route_step_append_route_point_action"), Fn("route_step_append_route_point_complete")),
        step(Fn("route_step_route_point_policy_action"), Fn("route_step_route_point_policy_complete")),
        step(Fn("route_step_context_selection_action"), Fn("route_step_context_selection_complete")),
        step(Fn("route_step_move_preview_action"), Fn("route_step_move_preview_complete")),
        step(Fn("route_step_commit_move_action"), Fn("route_step_commit_move_complete")),
        step(Fn("route_step_move_projection_action"), Fn("route_step_move_projection_complete")),
        step(Fn("route_step_delete_appended_point_action"), Fn("route_step_delete_appended_point_complete")),
        step(Fn("route_step_move_referenced_point_action"), Fn("route_step_move_referenced_point_complete")),
        step(Fn("route_step_undo_referenced_move_action"), Fn("route_step_undo_referenced_move_complete")),
        step(Fn("route_step_redo_referenced_move_action"), Fn("route_step_redo_referenced_move_complete")),
        step(Fn("route_step_delete_referenced_point_action"), Fn("route_step_delete_referenced_point_complete")),
        step(Fn("route_step_undo_referenced_delete_action"), Fn("route_step_undo_referenced_delete_complete")),
        step(Fn("route_step_redo_referenced_delete_action"), Fn("route_step_redo_referenced_delete_complete")),
        step(Fn("route_step_delete_remaining_referenced_point_action"), Fn("route_step_delete_remaining_referenced_point_complete")),
        step(Fn("route_step_one_point_route_action"), Fn("route_step_one_point_route_complete")),
        step(Fn("route_step_empty_route_action"), Fn("route_step_empty_route_complete")),
        wait_until(Fn("empty_route_program_is_selected_and_current")),
        step(Fn("route_step_readd_empty_route_action"), Fn("route_step_readd_empty_route_complete")),
        step(Fn("route_step_first_new_point_action"), Fn("route_step_first_new_point_complete")),
        step(Fn("route_step_modifier_noop_action"), Fn("route_step_modifier_noop_complete")),
        step(Fn("route_step_acquire_subject_action"), Fn("route_step_acquire_subject_complete")),
        step(Fn("route_step_start_route_action"), Fn("route_step_start_route_complete")),
        step(Fn("route_step_edit_active_route_action"), Fn("route_step_edit_active_route_complete")),
        step(Fn("route_step_route_point_reached_action"), Fn("route_step_route_point_reached_complete")),
        step(Fn("route_step_manual_route_pause_action"), Fn("route_step_manual_route_pause_complete")),
        step(Fn("route_step_release_subject_action"), Fn("route_step_release_subject_complete")),
        step(Fn("route_step_free_avatar_input_action"), Fn("route_step_free_avatar_input_complete")),
        step(Fn("route_step_route_after_free_input_action"), Fn("route_step_route_after_free_input_complete")),
        step(Fn("route_step_hud_route_toggle_action"), Fn("route_step_hud_route_toggle_complete")),
        step(Fn("route_step_hud_route_stopped_action"), Fn("route_step_hud_route_stopped_complete")),
        step(Fn("route_step_hud_route_started_again_action"), Fn("route_step_hud_route_started_again_complete")),
        step(Fn("route_step_generic_point_delete_action"), Fn("route_step_generic_point_delete_complete")),
        step(Fn("route_step_select_alternate_program_action"), Fn("route_step_select_alternate_program_complete")),
        step(Fn("route_step_add_alternate_point_action"), Fn("route_step_add_alternate_point_complete")),
        step(Fn("route_step_alternate_route_edit_action"), Fn("route_step_alternate_route_edit_complete")),
        step(Fn("route_step_verify_insert_before_action"), Fn("route_step_verify_insert_before_complete")),
        step(Fn("route_step_verify_insert_after_action"), Fn("route_step_verify_insert_after_complete")),
        step(Fn("route_step_verify_ribbon_add_action"), Fn("route_step_verify_ribbon_add_complete")),
        step(Fn("route_step_application_projection_admission_action"), Fn("route_step_application_projection_admission_complete")),
        wait_until(Fn("application_projection_tick_is_admitted")),
        step(Fn("route_step_application_projection_verdict_action"), Fn("route_step_application_projection_verdict_complete")),

    ])
}

fn task(me, ctx) {
    par_race([
        force_ok(reactive_seq([
            check(Fn("route_lifecycle_active")),
            route_lifecycle_steps(),
        ])),
        seq([
            wait(45.0),
            once(Fn("route_lifecycle_timeout")),
        ]),
    ])
}

fn on_stop(me, ctx) {
    if !this.done {
        this.checks.push(t_true(false, "route lifecycle test stopped before its verdict"));
        this = finish(this);
    }
}
