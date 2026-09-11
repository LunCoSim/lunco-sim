// Rhai production test for the generic model-authoring facades.

const ROOT = "/ModelAuthoring";
const CHASSIS = ROOT + "/Chassis";
const BODY = CHASSIS + "/Body";
const CONTROLLER = ROOT + "/Controller";
const MODEL = ROOT + "/MotorModel";
const SINK = ROOT + "/DriveInput";

fn query_ok(record) {
    record != () && record.ok != false
}

fn find_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return -1; }
    for item in docs.open_documents {
        if item.kind != "usd" { continue; }
        let root = query("QueryUsdPrim", #{
            doc_id: item.doc_id, path: ROOT, children: false,
        });
        if query_ok(root) { return item.doc_id; }
    }
    -1
}

fn has_port(graph, id) {
    for endpoint in graph.endpoints {
        if endpoint.id == id { return true; }
    }
    false
}

fn endpoint_domain(graph, id) {
    for endpoint in graph.endpoints {
        if endpoint == () { continue; }
        if endpoint.id == id { return endpoint.domain; }
    }
    ""
}

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([t_true(false, message)], "MODEL AUTHORING", "MODEL_AUTHORING");
    }
}

fn on_start(me) {
    this.elapsed = 0.0;
    this.doc = -1;
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.doc < 0 {
        this.doc = find_document();
        if this.doc < 0 {
            if this.elapsed > 10.0 { fail(this, "model-authoring fixture was not discoverable"); }
            return;
        }
    }

    let description = assembly_edit::describe(this.doc);
    if !query_ok(description) || description.generation == () {
        if this.elapsed > 10.0 { fail(this, "model-authoring document generation was unavailable"); }
        return;
    }

    let context = model_authoring::model_context(this.doc, ROOT, "@root@");
    if !query_ok(context) {
        fail(this, "model_context failed: " + context.error);
        return;
    }
    let graph = model_authoring::port_graph(this.doc, ROOT, "@root@");
    if !query_ok(graph) {
        fail(this, "port_graph failed: " + graph.error);
        return;
    }
    let wiring = model_authoring::wiring_plan(
        this.doc, "@root@", ROOT,
        [#{
            sink_path: SINK,
            sink_name: "inputs:throttle",
            sources: [CONTROLLER + ".outputs:throttle"],
        }],
        context.generation,
    );
    let bad_wiring = model_authoring::wiring_plan(
        this.doc, "@root@", ROOT,
        [#{
            sink_path: SINK,
            sink_name: "inputs:throttle",
            sources: [CONTROLLER + ".outputs:missing"],
        }],
        context.generation,
    );
    let readiness = model_authoring::readiness_report(
        this.doc, ROOT, "@root@", #{
            topology: [#{
                path: CHASSIS,
                expected_type: "Xform",
                children: [BODY],
            }],
            connections: [#{
                sink_path: SINK,
                sink_name: "inputs:throttle",
                sources: [CONTROLLER + ".outputs:throttle"],
            }],
            controls: [#{ path: CONTROLLER }],
            runtime: #{ required: false },
        },
    );
    let bad_readiness = model_authoring::readiness_report(
        this.doc, ROOT, "@root@", #{
            controls: [#{ path: ROOT + "/MissingControl" }],
        },
    );
    let recipe = model_authoring::scene_recipe(
        this.doc, "@root@", #{
            root: ROOT,
            references: [#{
                name: "Payload",
                type_name: "Xform",
                asset: "lunco://components/test/payload.usda",
            }],
            terrain: [],
            cameras: [#{
                parent: ROOT,
                name: "PreviewCamera",
                translation: [3.0, 2.0, 3.0],
            }],
            initial_state: [#{
                path: CHASSIS,
                name: "inputs:throttle",
                type_name: "float",
                value: "0.0",
            }],
            routes: [#{ path: CONTROLLER }],
            programs: [#{ path: CONTROLLER, spec: #{} }],
        },
        context.generation,
    );
    let bad_recipe = model_authoring::scene_recipe(
        this.doc, "@root@", #{
            root: ROOT,
            references: [#{ name: "Broken", type_name: "Xform", asset: "not-an-asset" }],
        },
        context.generation,
    );
    let bad_scene_identity = model_authoring::scene_recipe(
        this.doc, "@root@", #{
            root: ROOT,
            cameras: [#{ path: ROOT + "/MissingCamera" }],
        },
        context.generation,
    );
    let publication = model_authoring::publish_component(
        this.doc, ROOT, "@root@", "target/scene-tests/model_authoring.usda",
        #{ source: "model-authoring-fixture", revision: "test" },
    );
    let bad_publication = model_authoring::publish_component(
        this.doc, ROOT, "@root@", "target/scene-tests/model_authoring.usda", (),
    );

    this.done = true;
    report_verdict([
        t_true(context.generation == description.generation,
            "model_context generation is not current"),
        t_true(context.paths.contains(ROOT) && context.paths.contains(BODY),
            "model_context did not return the complete path set"),
        t_true(context.components.contains(ROOT) && context.components.contains(CHASSIS),
            "model_context did not return standard component kinds"),
        t_true(context.bodies.contains(CHASSIS) && context.colliders.contains(BODY),
            "model_context did not return body and collider facts"),
        t_true(has_port(graph, CONTROLLER + ".outputs:throttle")
            && has_port(graph, MODEL + ".connectors:torque")
            && has_port(graph, SINK + ".inputs:throttle"),
            "port_graph did not discover USD/Modelica/Rhai endpoints"),
        t_true(endpoint_domain(graph, CONTROLLER + ".outputs:throttle") == "rhai"
            && endpoint_domain(graph, MODEL + ".connectors:torque") == "modelica",
            "port_graph did not classify source domains"),
        t_true(graph.edges.len() == 1 && wiring.ok && wiring.ops.len() == 1,
            "port_graph or wiring_plan did not return the typed connection"),
        t_true(!bad_wiring.ok,
            "wiring_plan accepted a missing source endpoint"),
        t_true(readiness.ok && readiness.checks.len() == 6,
            "readiness_report did not combine the requested checks"),
        t_true(!bad_readiness.ok,
            "readiness_report accepted a missing control"),
        t_true(recipe.ok && recipe.ops.len() == 4 && recipe.routes.len() == 1
            && recipe.programs.len() == 1,
            "scene_recipe did not return the generic scene plan"),
        t_true(!bad_recipe.ok,
            "scene_recipe accepted an invalid asset identity"),
        t_true(!bad_scene_identity.ok,
            "scene_recipe accepted a missing camera identity"),
        t_true(publication.ok && publication.ops.len() == 2
            && publication.save_as.path == "target/scene-tests/model_authoring.usda",
            "publish_component did not return a clean typed publication/save plan"),
        t_true(!bad_publication.ok,
            "publish_component accepted missing provenance"),
    ], "MODEL AUTHORING", "MODEL_AUTHORING");
}
