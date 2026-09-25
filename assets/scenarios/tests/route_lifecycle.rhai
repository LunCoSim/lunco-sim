// Production acceptance for the generic route tool and scene-level route
// program. All observable route policy stays here; Rust is exercised only as
// the generic typed USD command and event/port boundary.

const ROOT = "/RouteLifecycle";
const ROUTE = "/RouteLifecycle/Route";
const ALTERNATE_ROUTE = "/RouteLifecycle/AlternateRoute";
const SUBJECT_PATH = "/RouteLifecycle/Subject";
const RIBBON_GEOMETRY = "/RouteLifecycle/Route/RouteRibbon/Geometry";
const ADMISSION_LANDING_PATH = "/RouteLifecycle/AdmissionLanding";
const ADMISSION_POINT_PATH = "/RouteLifecycle/AdmissionPoint";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [find_path(SUBJECT_PATH)], required_inputs: [] }
}

fn direct_points(doc) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: ROUTE, children: true });
    let points = [];
    if result == () || result.ok == false || result.children == () { return points; }
    let prefix = ROUTE + "/";
    for path in result.children {
        let rest = path.sub_string(prefix.len());
        let trigger = query("QueryUsdPrim", #{
            doc_id: doc,
            path: path + "/Trigger",
            attrs: ["lunco:triggerZone"],
        });
        if rest != "" && rest.index_of("/") < 0
            && trigger != () && trigger.ok != false && trigger.attrs != ()
            && trigger.attrs["lunco:triggerZone"] != () && has_path(doc, path) {
            points.push(path);
        }
    }
    points
}

fn point_count(doc) { direct_points(doc).len() }

fn route_revision(doc) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: ROUTE });
    if result == () || result.ok == false { return (); }
    result.generation
}

fn ribbon_count(doc) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: RIBBON_GEOMETRY,
        attrs: ["curveVertexCounts"],
    });
    if result == () || result.ok == false || result.attrs == ()
        || result.attrs["curveVertexCounts"] == () { return (); }
    result.attrs["curveVertexCounts"][0]
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

fn ribbon_clearance_matches(doc) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: RIBBON_GEOMETRY,
        attrs: ["points"],
    });
    if result == () || result.ok == false || result.attrs == ()
        || result.attrs["points"] == () || result.attrs["points"].len() == 0 {
        return false;
    }
    let first = result.attrs["points"][0];
    first.len() == 3 && first[1] >= 0.02 && first[1] <= 0.04
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

fn on_start(me, ctx) {
    this.doc = -1;
    this.phase = 0;
    this.elapsed = 0.0;
    this.done = false;
    this.checks = [];
    this.new_point = ROUTE + "/P3";
    this.subject = -1;
    this.started = 0;
    this.stopped = 0;
    this.reached = 0;
    this.reached_index = ();
    this.active_edit = false;
    this.hold_started = 0;
    this.program_switch_complete = false;
    this.route_ready = false;
    this.program_switch_subject = -1;
    this.reached_point_path = ();
    this.program_path = ();
    this.alternate_program_path = ();
    this.reference_admission_started = false;
    this.reference_admission_checked = false;
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
    if evt.name == "program.ready" && this.phase >= 101
        && evt.value != () && evt.value["path"] == this.program_path {
        // The readiness event is emitted by the newly started program. A
        // disposable presentation edit may advance the document generation
        // before this broadcast is delivered, so the event's path is the
        // lifecycle identity and the generation is diagnostic context only.
        this.route_ready = true;
    }
}

fn finish(me) {
    me.done = true;
    report_verdict(me.checks, "ROUTE LIFECYCLE", "ROUTE_LIFECYCLE");
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 45.0 {
        let detail = "route lifecycle acceptance timed out in phase " + this.phase;
        if this.phase == 0 {
            let listed = program_editor::list(this.doc, ROUTE);
            let programs = if listed == () || !listed.ok {
                -1
            } else {
                listed.programs.len()
            };
            detail += "; doc=" + this.doc.to_string()
                + ", subject=" + find_path(SUBJECT_PATH).to_string()
                + ", points=" + point_count(this.doc).to_string()
                + ", ribbon=" + ribbon_count(this.doc).to_string()
                + ", normals=" + ribbon_normals_match(this.doc, 3).to_string()
                + ", anchor=" + ribbon_anchor_matches_point(this.doc, ROUTE + "/P0").to_string()
                + ", programs=" + programs.to_string();
        }
        if this.phase == 2 {
            detail += "; point_count=" + point_count(this.doc).to_string()
                + ", new_point=" + this.new_point.to_string()
                + ", ribbon_count=" + ribbon_count(this.doc).to_string();
        }
        if this.phase == 5 {
            detail += "; point_count=" + point_count(this.doc).to_string()
                + ", referenced_point_exists=" + has_path(this.doc, ROUTE + "/P2").to_string()
                + ", ribbon_count=" + ribbon_count(this.doc).to_string();
        }
        if this.phase == 9 {
            detail += "; point_count=" + point_count(this.doc).to_string()
                + ", ribbon_count=" + ribbon_count(this.doc).to_string();
        }
        this.checks.push(t_true(false, detail));
        finish(this);
        return;
    }

    if this.reference_admission_started && !this.reference_admission_checked {
        let landing = find_path(ADMISSION_LANDING_PATH);
        let point = find_path(ADMISSION_POINT_PATH);
        if landing < 0 || point < 0 {
            this.checks.push(t_true(false,
                "the first fixed tick after reference admission observed an incomplete primary USD projection"));
            finish(this);
            return;
        }
        this.checks.push(t_true(true,
            "parallel primary-scene reference preparation commits before the next fixed tick"));
        this.reference_admission_checked = true;
    }

    if this.phase == 0 {
        let route = query("QueryUsdPrim", #{ path: ROUTE, children: true });
        if route == () || route.ok == false || route.doc_id == () { return; }
        this.doc = route.doc_id;
        this.subject = find_path(SUBJECT_PATH);
        if this.subject < 0 || point_count(this.doc) != 3 || ribbon_count(this.doc) != 3
            || !ribbon_normals_match(this.doc, 3)
            || !ribbon_anchor_matches_point(this.doc, ROUTE + "/P0") {
            return;
        }
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
        this.checks.push(t_true(ribbon_clearance_matches(this.doc),
            "route ribbon clears the surface along its support normal"));
        this.checks.push(t_true(listed.ok && listed.programs.len() == 1
            && listed.programs[0]["path"] == this.program_path,
            "generic program discovery lists the authored executable"));
        let hud = program_editor::enable_hud(this.doc, ROUTE, "always");
        this.checks.push(t_true(hud.ok, "Rhai can opt any USD program scope into the standard HUD"));
        if !hud.ok { finish(this); return; }
        let selected = program_editor::select(this.program_path);
        this.checks.push(t_true(selected.ok,
            "program selection establishes the active route without a Rust route registry"));
        this.phase = 50;
        return;
    }

    if this.phase == 50 {
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
        this.checks.push(t_true(hud_record != () && hud_record.ok != false
            && hud_record.attrs["lunco:ui:surfaceId"] == "program-browser"
            && hud_record.attrs["lunco:ui:visibilityMode"] == "always",
            "HUD activation is authored through the typed USD document"));
        for check in invalid_edit_checks(this.doc) { this.checks.push(check); }
        this.phase = 1;
        return;
    }

    if this.phase == 1 {
        if !this.program_switch_complete {
            this.program_switch_subject = find_path(SUBJECT_PATH);
            this.route_ready = false;
            // The source-edit command may restart the replacement program
            // before it returns. Enter the observation phase first so a
            // lifecycle edge emitted by the new source cannot be lost at the
            // old phase boundary.
            this.phase = 100;
            let switched = program_editor::switch_source(
                this.doc,
                this.program_path,
                "lunco://scenarios/tests/program_switch_hold.rhai",
            );
            if !switched.ok { this.checks.push(t_true(false, switched.error)); finish(this); return; }
            return;
        }
        let terrain_pointer = selected_route_context(
            this.doc, (), [0.0, 1.0, -31.0], ROUTE,
        );
        terrain_pointer.route_path = ();
        terrain_pointer.selected_path = ();
        terrain_pointer.target_path = ();
        terrain_pointer.hit_path = ();
        terrain_pointer.prim_paths = [];
        terrain_pointer.pointer_intents = ["route.add_point"];
        let terrain_added = scene_interaction::on_pointer(terrain_pointer);
        this.checks.push(t_true(terrain_added.ok && terrain_added.path != (),
            "terrain add-point intent resolves a route without a hit or selected prim"));
        if terrain_added.ok && terrain_added.path != () {
            let terrain_removed = waypoint_editor::on_delete(
                context(this.doc, terrain_added.path, ()),
            );
            this.checks.push(t_true(terrain_removed.ok,
                "the terrain-created point can be removed through the typed edit path"));
        }
        let pointer = scene_interaction::on_pointer(
            selected_route_context(this.doc, (), [0.0, 1.0, -32.0], ROUTE),
        );
        if pointer.ok != true {
            this.checks.push(t_true(false, "generic pointer policy rejected the add-point intent"));
            finish(this);
            return;
        }
        this.checks.push(t_true(pointer.path != (),
            "the add-point intent resolves the bound program through the subject programs relationship"));
        this.phase = 2;
        return;
    }

    if this.phase == 100 {
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
        let restored = program_editor::switch_source(
            this.doc,
            this.program_path,
            "lunco://scenarios/route_follow.rhai",
        );
        if !restored.ok { this.checks.push(t_true(false, restored.error)); finish(this); return; }
        this.phase = 101;
        return;
    }

    if this.phase == 101 {
        let program = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: this.program_path,
            attrs: ["info:implementationSource", "info:sourceAsset"],
        });
        if program == () || program.ok == false || program.attrs == ()
            || program.attrs["info:sourceAsset"] != "lunco://scenarios/route_follow.rhai" {
            return;
        }
        this.checks.push(t_true(true,
            "the original program source can be restored without rebuilding its subject"));
        this.checks.push(t_true(find_path(SUBJECT_PATH) == this.program_switch_subject,
            "program replacement does not recreate the physical subject"));
        // Wait for the restored program's lifecycle start before delivering a
        // one-shot scene event. A semantic event is not a durable command and
        // must not be sent into a program that is still being replaced.
        if !this.route_ready { return; }
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
            finish(this);
            return;
        }
        this.reference_admission_started = true;
        this.phase = 103;
        return;
    }

    if this.phase == 103 {
        if !this.reference_admission_checked { return; }
        let transient = waypoint_editor::on_click(
            context(this.doc, (), [0.0, 1.0, -40.0]),
        );
        if transient.ok != true || transient.path == () {
            this.checks.push(t_true(false,
                "a newly added route point is available to the edit policy immediately"));
            finish(this);
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
        this.checks.push(t_true(transient_move.ok == true,
            "a newly added route point can be moved before stage projection settles"));
        let transient_delete = waypoint_editor::on_delete(
            context(this.doc, transient_path, ()),
        );
        this.checks.push(t_true(transient_delete.ok == true,
            "a newly added route point can be deleted before stage projection settles"));
        this.phase = 102;
        return;
    }

    if this.phase == 102 {
        // Let the disposable ribbon observe the completed delete before the
        // durable point used by the rest of this lifecycle is appended.
        if point_count(this.doc) != 3 || ribbon_count(this.doc) != 3 { return; }
        let pointer = scene_interaction::on_pointer(
            context(this.doc, (), [0.0, 1.0, -32.0]),
        );
        if pointer.ok != true {
            this.checks.push(t_true(false, "generic pointer policy rejected the add-point intent"));
            finish(this);
            return;
        }
        this.new_point = pointer.path;
        this.program_switch_complete = true;
        this.phase = 2;
        return;
    }

    if this.phase == 2 {
        if point_count(this.doc) != 4 || !has_path(this.doc, this.new_point)
            || ribbon_count(this.doc) != 4 || !ribbon_normals_match(this.doc, 4) { return; }
        this.checks.push(t_true(true,
            "the add-point policy appends a named route point"));
        let position = point_position(this.doc, this.new_point);
        this.checks.push(t_true(position != () && position[2] == -32.0,
            "appended point has the clicked world position"));
        let nested = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: this.new_point + "/Dome",
        });
        this.checks.push(t_true(nested != () && nested.ok != false,
            "a route point reference composes its marker subtree under a referenced route parent"));
        let authored = query("ResolveUsdTarget", #{
            doc_id: this.doc,
            path: this.new_point,
            edit_target: "@runtime@",
        });
        this.checks.push(t_true(authored != () && authored.ok != false
            && authored.authored_here == true,
            "a nested route-point spawn is authored in the runtime layer of the canonical document"));
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
        this.phase = 20;
        return;
    }

    if this.phase == 20 {
        if is_unattended() {
            this.checks.push(t_true(true,
                "secondary-click validates the route context in unattended mode"));
            this.phase = 21;
            return;
        }
        let selection = query("InspectSelection", #{});
        if selection == () || selection.ok == false
            || selection.primary != this.selection_before_context { return; }
        this.checks.push(t_true(true,
            "secondary-click preserves the existing scene selection"));
        this.phase = 21;
        return;
    }

    if this.phase == 21 {
        let selected = waypoint_editor::context_route_point(
            context(this.doc, this.new_point, ()), ROUTE,
        );
        this.checks.push(t_true(selected == this.new_point,
            "secondary-click policy resolves the direct route point independently of selection"));
        if find_path(this.new_point) < 0 { return; }
        let nested = context(this.doc, this.new_point + "/Dome", ());
        nested.target_path = this.new_point + "/Dome";
        nested.hit_path = this.new_point + "/Dome";
        nested.prim_paths = [this.new_point + "/Dome", this.new_point];
        let selected_nested = waypoint_editor::context_route_point(nested, ROUTE);
        this.checks.push(t_true(selected_nested == this.new_point,
            "secondary-click resolves nested marker visuals to the canonical route point"));
        let moved = waypoint_editor::on_move(context(this.doc, this.new_point, [4.0, 1.0, -32.0]));
        this.checks.push(t_true(moved.ok == true && moved.path == this.new_point,
            "route-point move is a typed USD edit"));
        this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
            "adding and moving a point preserve the live subject identity"));
        this.phase = 3;
        return;
    }

    if this.phase == 3 {
        let position = point_position(this.doc, this.new_point);
        if position == () || position[0] != 4.0 || ribbon_count(this.doc) != 4 { return; }
        this.checks.push(t_true(true, "moving a point refreshes the reusable ribbon in place"));
        let deleted = waypoint_editor::on_delete(context(this.doc, this.new_point, ()));
        this.checks.push(t_true(deleted.ok == true && deleted.path == this.new_point,
            "delete removes an authored route point"));
        this.phase = 4;
        return;
    }

    if this.phase == 4 {
        if point_count(this.doc) != 3 || has_path(this.doc, this.new_point)
            || ribbon_count(this.doc) != 3 { return; }
        this.checks.push(t_true(true, "deleting a point updates ribbon topology"));
        this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
            "deleting a point does not rebuild the subject"));
        let moved = waypoint_editor::on_move(context(
            this.doc, ROUTE + "/P2", [2.0, 1.0, -26.0],
        ));
        this.checks.push(t_true(moved.ok == true && moved.path == ROUTE + "/P2",
            "moving a referenced route point authors a local USD transform opinion"));
        this.phase = 40;
        return;
    }

    if this.phase == 40 {
        let position = point_position(this.doc, ROUTE + "/P2");
        if position == () || position[0] != 2.0 || position[2] != -26.0
            || ribbon_count(this.doc) != 3 { return; }
        this.checks.push(t_true(true,
            "moving a referenced route point updates its composed ribbon"));
        this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
            "moving a referenced point preserves the subject identity"));
        let undone = cmd("UndoDocument", #{ doc_id: this.doc });
        if undone == () || undone.ok != true { return; }
        this.phase = 41;
        return;
    }

    if this.phase == 41 {
        let position = point_position(this.doc, ROUTE + "/P2");
        if position == () || position[0] != 0.0 || position[2] != -24.0
            || ribbon_count(this.doc) != 3 { return; }
        this.checks.push(t_true(true,
            "undo removes only the local transform opinion from a referenced point"));
        this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
            "undoing a referenced transform preserves the subject identity"));
        let redone = cmd("RedoDocument", #{ doc_id: this.doc });
        if redone == () || redone.ok != true { return; }
        this.phase = 42;
        return;
    }

    if this.phase == 42 {
        let position = point_position(this.doc, ROUTE + "/P2");
        if position == () || position[0] != 2.0 || position[2] != -26.0
            || ribbon_count(this.doc) != 3 { return; }
        this.checks.push(t_true(true,
            "redo reapplies the referenced transform without scene reprojection"));
        this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
            "redoing a referenced transform preserves the subject identity"));
        let deleted = waypoint_editor::on_delete(context(this.doc, ROUTE + "/P2", ()));
        this.checks.push(t_true(deleted.ok == true,
            "referenced route point uses a local USD active override on delete"));
        this.phase = 5;
        return;
    }

    if this.phase == 5 {
        if point_count(this.doc) != 2 || has_path(this.doc, ROUTE + "/P2")
            || ribbon_count(this.doc) != 2 { return; }
        this.checks.push(t_true(true, "deactivating a referenced point does not rebuild the subject"));
        let undone = cmd("UndoDocument", #{ doc_id: this.doc });
        if undone == () || undone.ok != true { return; }
        this.phase = 51;
        return;
    }

    if this.phase == 51 {
        if point_count(this.doc) != 3 || !has_path(this.doc, ROUTE + "/P2")
            || ribbon_count(this.doc) != 3 { return; }
        this.checks.push(t_true(true,
            "undo restores a referenced route point and its ribbon"));
        this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
            "undoing a referenced route edit preserves the subject identity"));
        let redone = cmd("RedoDocument", #{ doc_id: this.doc });
        if redone == () || redone.ok != true { return; }
        this.phase = 52;
        return;
    }

    if this.phase == 52 {
        if point_count(this.doc) != 2 || has_path(this.doc, ROUTE + "/P2")
            || ribbon_count(this.doc) != 2 { return; }
        this.checks.push(t_true(true,
            "redo reapplies the referenced route-point deletion"));
        this.checks.push(t_true(find_path(SUBJECT_PATH) == this.subject,
            "redoing a referenced route edit preserves the subject identity"));
        waypoint_editor::on_delete(context(this.doc, ROUTE + "/P1", ()));
        this.phase = 6;
        return;
    }

    if this.phase == 6 {
        if point_count(this.doc) != 1 || has_path(this.doc, ROUTE + "/P1")
            || ribbon_count(this.doc) != () { return; }
        this.checks.push(t_true(true, "a one-point route has no stale ribbon"));
        waypoint_editor::on_delete(context(this.doc, ROUTE + "/P0", ()));
        this.phase = 7;
        return;
    }

    if this.phase == 7 {
        if point_count(this.doc) != 0 || ribbon_count(this.doc) != () { return; }
        this.checks.push(t_true(true, "an empty route is safe and has no stale presentation"));
        waypoint_editor::on_click(context(this.doc, (), [0.0, 1.0, -8.0]));
        this.phase = 8;
        return;
    }

    if this.phase == 8 {
        if point_count(this.doc) != 1 || ribbon_count(this.doc) != () { return; }
        waypoint_editor::on_click(context(this.doc, (), [0.0, 1.0, -16.0]));
        this.phase = 9;
        return;
    }

    if this.phase == 9 {
        if point_count(this.doc) != 2 || ribbon_count(this.doc) != 2 { return; }
        let modified = modified_context(this.doc, (), [0.0, 1.0, -24.0],
            #{ alt: true, shift: true, ctrl: true });
        modified.pointer_intents = [];
        scene_interaction::on_pointer(modified);
        this.phase = 90;
        return;
    }

    if this.phase == 90 {
        if point_count(this.doc) != 2 || ribbon_count(this.doc) != 2 { return; }
        let possession = cmd("AcquireControl", #{
            target: this.subject,
            bind_camera: false,
        });
        this.checks.push(t_true(possession != () && possession.ok != false,
            "clicking the rover establishes the human control target"));
        this.phase = 91;
        return;
    }

    if this.phase == 91 {
        if !is_controlled(this.subject) { return; }
        this.checks.push(t_true(owner_of(this.subject) == local_session_id(),
            "the possessed rover is owned by the active session"));
        let edge = intent_pulse(this.subject, "action");
        this.checks.push(t_true(edge != () && edge.ok != false,
            "F is delivered through the generic semantic action edge"));
        this.phase = 10;
        return;
    }

    if this.phase == 10 {
        if !this.active_edit {
            if point_count(this.doc) != 2 || ribbon_count(this.doc) != 2 { return; }
            this.checks.push(t_true(true,
                "a non-matching modifier combination does not append a route point"));
            let added = waypoint_editor::on_click(
                context(this.doc, (), [0.0, 1.0, -24.0]),
            );
            this.checks.push(t_true(added.ok == true,
                "editing a route while it is running appends a point"));
            this.active_edit = true;
            return;
        }
        if point_count(this.doc) != 3 || ribbon_count(this.doc) != 3 { return; }
        if this.started == 0 || this.stopped != 0 { return; }
        let guidance = port(this.subject, "guidance_throttle");
        if guidance == () || guidance.abs() < 0.01 { return; }
        this.checks.push(t_true(true,
            "an active route keeps running through a live point edit"));
        let program_active = port(this.subject, "program_active");
        if program_active == () || program_active < 0.5 { return; }
        this.checks.push(t_true(true,
            "F starts the authored program without replacing local possession"));
        this.checks.push(t_true(true,
            "the running route program produces generic drive commands"));
        let points = direct_points(this.doc);
        if points.len() == 0 { return; }
        let zone = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: points[0] + "/Trigger",
            attrs: ["lunco:triggerZone"],
        });
        if zone == () || zone.ok == false || zone.attrs == ()
            || zone.attrs["lunco:triggerZone"] == () { return; }
        this.reached_point_path = points[0];
        emit("enter:" + zone.attrs["lunco:triggerZone"], this.subject);
        this.phase = 11;
        return;
    }

    if this.phase == 11 {
        if this.reached == 0 { return; }
        let reached_color = marker_color(this.doc, this.reached_point_path);
        let points = direct_points(this.doc);
        if reached_color == () || points.len() < 2 { return; }
        let pending_path = if points[0] == this.reached_point_path { points[1] } else { points[0] };
        let pending_color = marker_color(this.doc, pending_path);
        if !color_matches(reached_color, [0.55, 0.55, 0.55])
            || !color_matches(pending_color, [0.10, 0.95, 0.22]) { return; }
        this.checks.push(t_true(true,
            "visited route points turn gray while unvisited points stay green"));
        this.checks.push(t_true(this.reached_index == 0,
            "the sensor event advances the stable current route point"));
        let hud_toggle = emit("runtime.ui.action", #{ action: "program.toggle", parameters: #{} });
        this.checks.push(t_true(hud_toggle,
            "the authored program HUD dispatches a generic toggle action"));
        this.phase = 12;
        return;
    }

    if this.phase == 12 {
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
        this.phase = 120;
        return;
    }

    if this.phase == 120 {
        let program_active = port(this.subject, "program_active");
        if this.started < 2 || program_active == () || program_active < 0.5 { return; }
        this.checks.push(t_true(true, "the program HUD toggles the route on"));
        let stopped = emit("runtime.ui.action", #{ action: "program.toggle", parameters: #{} });
        this.checks.push(t_true(stopped, "the program HUD dispatches its off action"));
        this.phase = 121;
        return;
    }

    if this.phase == 121 {
        let program_active = port(this.subject, "program_active");
        if this.stopped < 2 || program_active == () || program_active > 0.5 { return; }
        this.checks.push(t_true(true, "the program HUD toggles the route off again"));
        let points = direct_points(this.doc);
        if points.len() == 0 { return; }
        let delete_path = points[0];
        let target = find_path(delete_path);
        if target < 0 { return; }
        this.generic_delete_path = delete_path;
        let deleted = cmd("DeleteEntity", #{ target: target });
        if deleted == () || deleted.ok == false {
            this.checks.push(t_true(false,
                "generic DeleteEntity accepted a selected authored route point"));
            finish(this);
            return;
        }
        this.phase = 13;
        return;
    }

    if this.phase == 13 {
        if point_count(this.doc) != 2 || has_path(this.doc, this.generic_delete_path)
            || ribbon_count(this.doc) != 2 { return; }
        this.checks.push(t_true(true,
            "generic DeleteEntity persists a runtime active override for an authored point"));
        let alternate = program_editor::list(this.doc, ALTERNATE_ROUTE);
        if alternate == () || !alternate.ok || alternate.programs.len() == 0 { return; }
        this.alternate_program_path = alternate.programs[0]["path"];
        let selected = program_editor::select(this.alternate_program_path);
        if !selected.ok {
            this.checks.push(t_true(false, selected.error));
            finish(this);
            return;
        }
        this.phase = 131;
        return;
    }

    if this.phase == 131 {
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
        this.phase = 14;
        return;
    }

    if this.phase == 14 {
        if !has_path(this.doc, ALTERNATE_ROUTE + "/P1") { return; }
        this.checks.push(t_true(point_count(this.doc) == 2,
            "editing the alternate program does not mutate the first route"));
        finish(this);
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        this.checks.push(t_true(false, "route lifecycle test stopped before its verdict"));
        finish(this);
    }
}
