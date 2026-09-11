// Generic, Rhai-first authoring facades for human and AI model builders.
//
// This library deliberately composes the existing USD reader, assembly plans,
// audit reports, and document owner. It does not create a second USD writer,
// infer identities from names, or encode a vehicle. The returned plans are
// dry until the caller submits `ops` through assembly_edit::batch or review.

import "assembly_edit" as assembly_edit;
import "assembly_builder" as assembly_builder;
import "assembly_audit" as assembly_audit;

fn failure(kind, message) {
    #{ ok: false, kind: kind, error: message, errors: [message], ops: [] }
}

fn query_ok(record) {
    record != () && record.ok != false
}

fn absolute_prim(path, label) {
    if path == () || type_of(path) != "string" || path == ""
        || !path.starts_with("/") || path.ends_with("/") && path != "/" {
        return failure("validation", label + " must be an absolute USD prim path");
    }
    #{ ok: true, value: path }
}

fn clean_asset_path(asset, label) {
    if asset == () || type_of(asset) != "string" || asset == ""
        || !asset.starts_with("lunco://") || asset.contains("@")
        || asset.contains(" ") {
        return failure("validation", label + " must be a non-empty lunco:// asset identity");
    }
    #{ ok: true, value: asset }
}

fn attr_record(attributes, name) {
    if attributes == () { return (); }
    for attribute in attributes {
        if attribute.name == name { return attribute; }
    }
    ()
}

fn attr_value(attributes, name) {
    let record = attr_record(attributes, name);
    if record == () { return (); }
    record.value
}

fn metadata_value(prim, metadata_name) {
    if prim == () || prim.metadata == () { return (); }
    let metadata = prim.metadata[metadata_name];
    if metadata == () { return (); }
    if metadata.composed != () && metadata.composed.present == true {
        return metadata.composed.value;
    }
    if metadata.present == true { return metadata.value; }
    ()
}

fn local_attribute_name(path, name) {
    let parts = path.split("/");
    let leaf = parts[parts.len() - 1];
    let prefix = leaf + ".";
    if name.starts_with(prefix) { return name.sub_string(prefix.len()); }
    name
}

fn normalized_attributes(path, attributes) {
    let result = [];
    if attributes == () { return result; }
    for attribute in attributes {
        result.push(#{
            name: local_attribute_name(path, attribute.name),
            type: attribute.type,
            value: attribute.value,
        });
    }
    result
}

fn document_default_prim(description) {
    if description == () || description.metadata == ()
        || description.metadata.defaultPrim == () {
        return ();
    }
    let composed = description.metadata.defaultPrim.composed;
    if composed == () || composed.present != true { return (); }
    let value = composed.value;
    if value == () || value == "" { return (); }
    if value.starts_with("/") { value } else { "/" + value }
}

fn source_domain(attributes, port_name) {
    if port_name.starts_with("connectors:") { return "modelica"; }
    let source_asset = attr_value(attributes, "info:sourceAsset");
    if source_asset != () && source_asset.contains(".rhai") { return "rhai"; }
    if source_asset != () && source_asset.contains(".mo") { return "modelica"; }
    let implementation = attr_value(attributes, "info:implementationSource");
    if implementation != () && implementation.contains("sourceCode")
        && attr_value(attributes, "info:sourceCode") != () {
        return "rhai";
    }
    "usd"
}

fn port_direction(name) {
    if name.starts_with("outputs:") { return "output"; }
    if name.starts_with("inputs:") { return "input"; }
    if name.starts_with("connectors:") { return "connector"; }
    "property"
}

fn is_port(name) {
    name.starts_with("inputs:") || name.starts_with("outputs:")
        || name.starts_with("connectors:")
}

fn node_ports(path, attributes, connections) {
    let ports = [];
    if attributes == () { return ports; }
    for attribute in attributes {
        if !is_port(attribute.name) { continue; }
        let sources = [];
        if connections != () && connections[attribute.name] != () {
            sources = connections[attribute.name];
        }
        ports.push(#{
            id: path + "." + attribute.name,
            path: path,
            name: attribute.name,
            direction: port_direction(attribute.name),
            domain: source_domain(attributes, attribute.name),
            type_name: attribute.type,
            value: attribute.value,
            connections: sources,
        });
    }
    ports
}

fn has_schema(schemas, expected) {
    if schemas == () { return false; }
    for schema in schemas {
        if schema == expected { return true; }
    }
    false
}

fn node_actions(path, type_name, target, root) {
    let resolved = query_ok(target) && target.status == "resolved";
    let editable = resolved && target.edit_scope != "composed_read_only";
    [
        #{ action: "inspect", enabled: true },
        #{ action: "select", enabled: true },
        #{ action: "edit_properties", enabled: editable },
        #{ action: "transform", enabled: editable && type_name == "Xform" },
        #{ action: "remove", enabled: editable && path != root },
        #{ action: "publish_component", enabled: editable && path == root },
    ]
}

fn read_node(doc, path, edit_target, root) {
    let query_record = query("QueryUsdPrim", #{
        doc_id: doc,
        path: path,
        children: true,
        collision_bounds: true,
        topology: true,
        relationships: true,
        connections: true,
        schemas: true,
    });
    if !query_ok(query_record) {
        return failure("model_context", path + ": composed USD read failed");
    }

    let inspection = assembly_edit::inspect(doc, path);
    if !query_ok(inspection) || inspection.prim == () || inspection.prim.exists != true {
        return failure("model_context", path + ": complete USD inspection failed");
    }
    let attributes = normalized_attributes(path, inspection.prim.attributes);
    let kind = metadata_value(inspection.prim, "kind");
    let variants = metadata_value(inspection.prim, "variantSelections");
    let references = inspection.prim.references;
    let target = assembly_edit::resolve_target(doc, path, edit_target);
    if !query_ok(target) {
        return failure("model_context", path + ": edit target resolution failed");
    }

    let ports = node_ports(path, attributes, query_record.connections);
    let mount_attributes = #{};
    if attributes != () {
        for attribute in attributes {
            if attribute.name.starts_with("lunco:mount:") {
                mount_attributes[attribute.name] = attribute.value;
            }
        }
    }
    let mount_relationships = #{};
    if query_record.relationships != () {
        for name in query_record.relationships.keys() {
            if name.starts_with("lunco:mount:") {
                mount_relationships[name] = query_record.relationships[name];
            }
        }
    }
    let has_mount = mount_attributes.len() > 0 || mount_relationships.len() > 0;
    let topology_body = query_record.topology != ()
        && query_record.topology.selection != ()
        && query_record.topology.selection.body_owner == path;
    let body = has_schema(query_record.api_schemas, "PhysicsRigidBodyAPI")
        || attr_value(attributes, "physics:mass") != ()
        || topology_body;
    let frame = query_record.type_name == "Xform"
        && attr_value(attributes, "xformOpOrder") != ();
    let component = kind == "component";

    #{
        ok: true,
        path: path,
        type_name: query_record.type_name,
        active: inspection.prim.active,
        kind: kind,
        children: query_record.children,
        attributes: attributes,
        relationships: query_record.relationships,
        connections: query_record.connections,
        api_schemas: query_record.api_schemas,
        references: references,
        variants: variants,
        collision_bounds: query_record.collision_bounds,
        topology: query_record.topology,
        world_position: query_record.world_position,
        spawned: query_record.spawned,
        edit_target: target,
        ports: ports,
        component: component,
        frame: frame,
        body: body,
        collider: has_schema(query_record.api_schemas, "PhysicsCollisionAPI"),
        mount: if has_mount { #{
            attributes: mount_attributes,
            relationships: mount_relationships,
        }} else { () },
        actions: node_actions(path, query_record.type_name, target, root),
    }
}

/// Return one complete, composed assembly context. `root` is an exact prim
/// path and `edit_target` is explicit (`@root@` for authored component work or
/// `@runtime@` for a local override). Every path, reference, variant, frame,
/// mount, body, joint, collider, port, generation, and action comes from this
/// one Rhai facade over the shared USD readers.
fn model_context(doc, root, edit_target) {
    if doc == () || edit_target == () {
        return failure("model_context", "model_context requires document id and edit target");
    }
    let valid_root = absolute_prim(root, "model_context root");
    if !valid_root.ok { return valid_root; }
    let description = assembly_edit::describe(doc);
    if !query_ok(description) || description.generation == () {
        return failure("model_context", "document description has no usable generation");
    }
    let root_check = query("QueryUsdPrim", #{
        doc_id: doc, path: root, children: false,
    });
    if !query_ok(root_check) {
        return failure("model_context", root + ": root prim is not composed");
    }

    let queue = [root];
    let visited = [];
    let nodes = [];
    let references = [];
    let variants = [];
    let components = [];
    let frames = [];
    let mounts = [];
    let bodies = [];
    let joints = [];
    let colliders = [];
    let ports = [];
    while queue.len() > 0 {
        let path = queue.pop();
        if visited.contains(path) { continue; }
        visited.push(path);
        let node = read_node(doc, path, edit_target, root);
        if !node.ok { return node; }
        nodes.push(node);
        for child in node.children { queue.push(child); }

        if node.references != () { references.push(#{ path: path, value: node.references }); }
        if node.variants != () { variants.push(#{ path: path, value: node.variants }); }
        if node.component { components.push(path); }
        if node.frame { frames.push(path); }
        if node.mount != () { mounts.push(#{ path: path, value: node.mount }); }
        if node.body { bodies.push(path); }
        if node.collider && !colliders.contains(path) { colliders.push(path); }
        for port in node.ports { ports.push(port); }
        if node.topology != () {
            for joint in node.topology.joints {
                let seen = false;
                for existing in joints {
                    if existing.path == joint.path { seen = true; break; }
                }
                if !seen { joints.push(joint); }
            }
        }
    }

    let default_prim = document_default_prim(description);
    #{
        ok: true,
        kind: "model_context",
        doc_id: doc,
        root: root,
        edit_target: edit_target,
        generation: description.generation,
        dirty: description.dirty,
        origin: description.origin,
        default_prim: default_prim,
        paths: visited,
        nodes: nodes,
        references: references,
        variants: variants,
        components: components,
        frames: frames,
        mounts: mounts,
        bodies: bodies,
        joints: joints,
        colliders: colliders,
        ports: ports,
        actions: nodes.map(|node| #{ path: node.path, actions: node.actions }),
        composition: description.composition,
        diagnostics: description.diagnostics,
    }
}

fn check_record(name, status, detail, errors) {
    #{
        name: name,
        status: status,
        ok: status != "failed",
        detail: detail,
        errors: errors,
    }
}

fn context_port(context, id) {
    for port in context.ports {
        if port.id == id { return port; }
    }
    ()
}

fn control_check(context, controls) {
    let errors = [];
    let records = [];
    if controls == () || controls.len() == 0 {
        return check_record("controls", "not_requested", [], []);
    }
    for control in controls {
        if control.path == () {
            errors.push("control requires an exact path");
            continue;
        }
        let path_exists = context.paths.contains(control.path);
        let inputs = if control.inputs == () { [] } else { control.inputs };
        let missing = [];
        for name in inputs {
            if context_port(context, control.path + "." + name) == () {
                missing.push(control.path + "." + name);
            }
        }
        let ok = path_exists && missing.len() == 0;
        if !path_exists { errors.push(control.path + ": control prim is missing"); }
        for missing_port in missing { errors.push(missing_port + ": control port is missing"); }
        records.push(#{ path: control.path, inputs: inputs, missing: missing, ok: ok });
    }
    check_record("controls", if errors.len() == 0 { "passed" } else { "failed" }, records, errors)
}

fn runtime_check(context, runtime) {
    if runtime == () || runtime.required != true {
        return check_record("runtime", "not_requested", [], []);
    }
    let paths = if runtime.paths == () { context.paths } else { runtime.paths };
    let errors = [];
    let records = [];
    for path in paths {
        let node = ();
        for candidate in context.nodes {
            if candidate.path == path { node = candidate; break; }
        }
        if node == () {
            errors.push(path + ": runtime path is outside the context");
            continue;
        }
        let state = if node.spawned == true { "spawned" } else { "not_spawned" };
        if state != "spawned" { errors.push(path + ": prim is not projected into runtime"); }
        records.push(#{ path: path, state: state, topology: node.topology });
    }
    check_record("runtime", if errors.len() == 0 { "passed" } else { "failed" }, records, errors)
}

/// Combine the generic topology/physicality/mount/wiring/control/runtime
/// checks into one machine-readable preflight. Policies are supplied by the
/// caller as exact manifests; omitted sections are explicitly marked
/// `not_requested`, never treated as silently passing evidence.
fn readiness_report(doc, root, edit_target, policy) {
    if policy == () { return failure("readiness_report", "readiness_report requires an explicit policy map"); }
    let context = model_context(doc, root, edit_target);
    if !context.ok { return context; }
    let checks = [];
    let errors = [];

    let topology = if policy.topology == () || policy.topology.len() == 0 {
        check_record("topology", "passed", #{ paths: context.paths }, [])
    } else {
        let result = assembly_audit::topology_report(doc, policy.topology);
        check_record("topology", if result.ok { "passed" } else { "failed" }, result.records, result.errors)
    };
    checks.push(topology);
    for error in topology.errors { errors.push(error); }

    let physicality = if policy.physicality == () || policy.physicality.len() == 0 {
        check_record("physicality", "not_requested", [], [])
    } else {
        let result = assembly_audit::physicality_report(doc, policy.physicality);
        check_record("physicality", if result.ok { "passed" } else { "failed" }, result.parts, result.errors)
    };
    checks.push(physicality);
    for error in physicality.errors { errors.push(error); }

    let mounts = if policy.mounts == () || policy.mounts.len() == 0 {
        check_record("mounts", "not_requested", [], [])
    } else {
        let result = assembly_audit::mount_contract_report(doc, policy.mounts);
        check_record("mounts", if result.ok { "passed" } else { "failed" }, result.connections, result.errors)
    };
    checks.push(mounts);
    for error in mounts.errors { errors.push(error); }

    let wiring = if policy.connections == () || policy.connections.len() == 0 {
        check_record("connections", "not_requested", [], [])
    } else {
        let result = wiring_plan(doc, edit_target, root, policy.connections, context.generation);
        check_record("connections", if result.ok { "passed" } else { "failed" }, result, if result.ok { [] } else { [result.error] })
    };
    checks.push(wiring);
    for error in wiring.errors { errors.push(error); }

    let controls = control_check(context, policy.controls);
    checks.push(controls);
    for error in controls.errors { errors.push(error); }

    let runtime = runtime_check(context, policy.runtime);
    checks.push(runtime);
    for error in runtime.errors { errors.push(error); }

    #{
        ok: errors.len() == 0,
        kind: "readiness_report",
        doc_id: doc,
        root: root,
        generation: context.generation,
        checks: checks,
        errors: errors,
        context: context,
    }
}

fn recipe_reference(doc, edit_target, parent, spec) {
    if spec == () || spec.name == () || spec.type_name == () || spec.asset == () {
        return failure("scene_recipe", "each scene reference requires parent, name, type_name, and asset");
    }
    let asset = clean_asset_path(spec.asset, parent + "/" + spec.name);
    if !asset.ok { return asset; }
    let target = if spec.reference_prim_path == () { () } else { spec.reference_prim_path };
    assembly_builder::referenced_instance_targeted_plan(
        doc, edit_target, parent, spec.name, spec.type_name, asset.value,
        target, spec.translation, spec.rotation, spec.scale,
        spec.api_schemas, spec.attributes,
    )
}

fn scene_prim(doc, path, label) {
    let valid = absolute_prim(path, label);
    if !valid.ok { return valid; }
    if path == "/" {
        return #{ ok: true, path: path, type_name: "PseudoRoot" };
    }
    let record = query("QueryUsdPrim", #{
        doc_id: doc, path: path, children: false,
    });
    if !query_ok(record) {
        return failure("scene_recipe", label + " " + path + ": composed prim is missing");
    }
    #{ ok: true, path: path, type_name: record.type_name }
}

fn recipe_camera(edit_target, spec) {
    if spec == () { return failure("scene_recipe", "camera entry is missing"); }
    if spec.path != () {
        let valid = absolute_prim(spec.path, "camera path");
        if !valid.ok { return valid; }
        let placement = assembly_builder::place_plan(edit_target, spec.path,
            spec.translation, spec.rotation, spec.scale);
        if !placement.ok { return placement; }
        return #{ ok: true, path: spec.path, ops: placement.ops, created: false };
    }
    if spec.parent == () || spec.name == () || spec.name == "" {
        return failure("scene_recipe", "new camera requires parent and name");
    }
    let parent = absolute_prim(spec.parent, "camera parent");
    if !parent.ok { return parent; }
    if spec.name.contains("/") {
        return failure("scene_recipe", "camera name must be one USD identifier");
    }
    let path = if spec.parent == "/" { "/" + spec.name } else { spec.parent + "/" + spec.name };
    let ops = [#{ AddPrim: #{
        edit_target: edit_target, parent_path: spec.parent, name: spec.name,
        type_name: "Camera", reference: (), reference_prim_path: "",
    }}];
    if spec.translation != () || spec.rotation != () || spec.scale != () {
        let placement = assembly_builder::place_plan(edit_target, path,
            spec.translation, spec.rotation, spec.scale);
        if !placement.ok { return placement; }
        for op in placement.ops { ops.push(op); }
    }
    #{ ok: true, path: path, ops: ops, created: true }
}

/// Build one generic scene recipe. USD changes are typed dry ops; route and
/// program entries are returned as explicit hand-off records for
/// `waypoint_editor` and `assembly_edit::attach_program`, so no scene-specific
/// XML or program policy is hidden in the core.
fn scene_recipe(doc, edit_target, spec, parent_gen) {
    if doc == () || edit_target == () || spec == () || spec.root == () {
        return failure("scene_recipe", "scene_recipe requires document, edit target, root, and recipe");
    }
    let parent = absolute_prim(spec.root, "scene recipe root");
    if !parent.ok { return parent; }
    let root_record = scene_prim(doc, spec.root, "scene recipe root");
    if !root_record.ok { return root_record; }
    let description = assembly_edit::describe(doc);
    if !query_ok(description) || description.generation == () {
        return failure("scene_recipe", "scene recipe could not read document generation");
    }
    if parent_gen != () && parent_gen != description.generation {
        return failure("scene_recipe", "scene recipe rejected stale document generation");
    }
    let ops = [];
    let references = [];
    let reference_specs = if spec.references == () { [] } else { spec.references };
    for reference in reference_specs {
        let reference_parent = if reference.parent == () { spec.root } else { reference.parent };
        let parent_record = scene_prim(doc, reference_parent, "reference parent");
        if !parent_record.ok { return parent_record; }
        let planned = recipe_reference(doc, edit_target, reference_parent, reference);
        if !planned.ok { return planned; }
        for op in planned.ops { ops.push(op); }
        references.push(#{ path: planned.path, asset: planned.asset, spec: reference });
    }

    let terrain = [];
    let terrain_specs = if spec.terrain == () { [] } else { spec.terrain };
    for entry in terrain_specs {
        let terrain_parent = if entry.parent == () { spec.root } else { entry.parent };
        let parent_record = scene_prim(doc, terrain_parent, "terrain parent");
        if !parent_record.ok { return parent_record; }
        let planned = recipe_reference(doc, edit_target, terrain_parent, entry);
        if !planned.ok { return planned; }
        for op in planned.ops { ops.push(op); }
        terrain.push(#{ path: planned.path, asset: planned.asset, spec: entry });
    }

    let cameras = [];
    let camera_specs = if spec.cameras == () { [] } else { spec.cameras };
    for camera in camera_specs {
        if camera.path != () {
            let camera_record = scene_prim(doc, camera.path, "camera");
            if !camera_record.ok { return camera_record; }
            if camera_record.type_name != "Camera" {
                return failure("scene_recipe", camera.path + ": existing camera path is not a Camera prim");
            }
        } else if camera.parent != () {
            let camera_parent = scene_prim(doc, camera.parent, "camera parent");
            if !camera_parent.ok { return camera_parent; }
        }
        let planned = recipe_camera(edit_target, camera);
        if !planned.ok { return planned; }
        for op in planned.ops { ops.push(op); }
        cameras.push(planned);
    }

    let initial_state = [];
    let state_specs = if spec.initial_state == () { [] } else { spec.initial_state };
    for state in state_specs {
        if state.path == () || state.name == () || state.type_name == () || state.value == () {
            return failure("scene_recipe", "initial state entries require path, name, type_name, and value");
        }
        let valid_path = absolute_prim(state.path, "initial state path");
        if !valid_path.ok { return valid_path; }
        let state_record = scene_prim(doc, state.path, "initial state");
        if !state_record.ok { return state_record; }
        let op = #{ SetAttribute: #{
            edit_target: edit_target, path: state.path, name: state.name,
            type_name: state.type_name, value: state.value,
        }};
        ops.push(op);
        initial_state.push(state);
    }

    let routes = if spec.routes == () { [] } else { spec.routes };
    for route in routes {
        if route.path == () {
            return failure("scene_recipe", "route entries require an exact authored program path");
        }
        let valid_path = absolute_prim(route.path, "route path");
        if !valid_path.ok { return valid_path; }
        let route_record = scene_prim(doc, route.path, "route");
        if !route_record.ok { return route_record; }
    }
    let programs = if spec.programs == () { [] } else { spec.programs };
    for program in programs {
        if program.path == () || program.spec == () {
            return failure("scene_recipe", "program entries require path and AttachProgram spec");
        }
        let valid_path = absolute_prim(program.path, "program path");
        if !valid_path.ok { return valid_path; }
        let program_record = scene_prim(doc, program.path, "program");
        if !program_record.ok { return program_record; }
    }
    #{
        ok: true,
        kind: "scene_recipe",
        doc_id: doc,
        root: spec.root,
        parent_generation: description.generation,
        references: references,
        terrain: terrain,
        cameras: cameras,
        initial_state: initial_state,
        routes: routes,
        programs: programs,
        ops: ops,
        apply: #{
            usd: "assembly_edit::batch",
            routes: "waypoint_editor",
            programs: "assembly_edit::attach_program",
        },
    }
}

/// Discover all USD inputs/outputs/connectors under one exact assembly root.
/// Connections are returned as graph edges; endpoint domains distinguish USD,
/// Modelica connector declarations, and source-code-backed Rhai programs.
fn port_graph(doc, root, edit_target) {
    let context = model_context(doc, root, edit_target);
    if !context.ok { return context; }
    let edges = [];
    for port in context.ports {
        for source in port.connections {
            edges.push(#{
                source: source,
                sink: port.id,
                sink_domain: port.domain,
                kind: "usd_connection",
            });
        }
    }
    #{
        ok: true,
        kind: "port_graph",
        doc_id: doc,
        root: root,
        generation: context.generation,
        endpoints: context.ports,
        ports: context.ports,
        edges: edges,
    }
}

fn graph_endpoint(graph, id) {
    for endpoint in graph.endpoints {
        if endpoint == () { continue; }
        if endpoint.id == id { return endpoint; }
    }
    ()
}

fn endpoint_id(path, name) {
    if path == () || name == () { return (); }
    path + "." + name
}

/// Build one typed, generation-checked USD connection plan. Each entry is
/// `{sink_path, sink_name, sources:["/Producer.outputs:value"], type_name?}`.
/// The exact endpoint graph is checked before any SetConnection operation is
/// returned; no missing port is turned into a warning or an inferred wire.
fn wiring_plan(doc, edit_target, root, connections, parent_gen) {
    if doc == () || edit_target == () || root == () || connections == () {
        return failure("wiring_plan", "wiring_plan requires document, edit target, root, and connections");
    }
    if connections.len() == 0 {
        return failure("wiring_plan", "wiring_plan requires at least one connection");
    }
    let graph = port_graph(doc, root, edit_target);
    if !graph.ok { return graph; }
    if parent_gen != () && parent_gen != graph.generation {
        return failure("wiring_plan", "wiring plan rejected stale document generation");
    }
    let ops = [];
    let records = [];
    for connection in connections {
        if connection.sink_path == () || connection.sink_name == ()
            || connection.sources == () || connection.sources.len() == 0 {
            return failure("wiring_plan", "each connection requires sink_path, sink_name, and one or more sources");
        }
        let sink_id = endpoint_id(connection.sink_path, connection.sink_name);
        let sink = graph_endpoint(graph, sink_id);
        if sink == () {
            return failure("wiring_plan", sink_id + ": sink endpoint is missing");
        }
        if sink.direction != "input" && sink.direction != "connector" {
            return failure("wiring_plan", sink_id + ": sink must be an input or connector");
        }
        let source_ids = [];
        let source_endpoints = [];
        for source in connection.sources {
            if type_of(source) != "string" || !source.starts_with("/") || !source.contains(".") {
                return failure("wiring_plan", sink_id + ": source must be `/Prim.property` path");
            }
            let parts = source.split(".");
            if parts.len() != 2 {
                return failure("wiring_plan", sink_id + ": source must contain one prim/property separator");
            }
            let source_id = source;
            let endpoint = graph_endpoint(graph, source_id);
            if endpoint == () {
                return failure("wiring_plan", source_id + ": source endpoint is missing");
            }
            if endpoint.direction != "output" && endpoint.direction != "connector" {
                return failure("wiring_plan", source_id + ": source must be an output or connector");
            }
            if connection.type_name != () && endpoint.type_name != ()
                && connection.type_name != endpoint.type_name {
                return failure("wiring_plan", source_id + ": explicit type does not match source endpoint");
            }
            if sink.type_name != () && endpoint.type_name != ()
                && sink.type_name != endpoint.type_name {
                return failure("wiring_plan", sink_id + ": source and sink USD types differ");
            }
            source_ids.push(source_id);
            source_endpoints.push(endpoint);
        }
        let type_name = if connection.type_name == () { sink.type_name } else { connection.type_name };
        if type_name == () || type_name == "" {
            return failure("wiring_plan", sink_id + ": connection type cannot be resolved");
        }
        ops.push(#{ SetConnection: #{
            edit_target: edit_target,
            path: connection.sink_path,
            name: connection.sink_name,
            type_name: type_name,
            sources: source_ids,
        }});
        records.push(#{
            sink: sink,
            sources: source_endpoints,
            type_name: type_name,
        });
    }
    #{
        ok: true,
        kind: "wiring_plan",
        doc_id: doc,
        root: root,
        parent_generation: graph.generation,
        connections: records,
        ops: ops,
    }
}

fn valid_provenance(provenance) {
    if provenance == () || type_of(provenance) != "map"
        || provenance.source == () || type_of(provenance.source) != "string"
        || provenance.source == "" {
        return failure("publish_component", "provenance requires a non-empty source");
    }
    if provenance.revision != () && type_of(provenance.revision) != "string" {
        return failure("publish_component", "provenance revision must be a string when supplied");
    }
    #{ ok: true, value: provenance }
}

fn reference_items(reference_info) {
    if reference_info == () { return []; }
    if reference_info.composed != () && reference_info.composed.items != () {
        return reference_info.composed.items;
    }
    if reference_info.authored != () && reference_info.authored.root != ()
        && reference_info.authored.root.items != () {
        return reference_info.authored.root.items;
    }
    []
}

/// Validate a standalone, top-level reusable component and return the typed
/// publication plan. The caller submits `ops` through the normal review/batch
/// boundary, then executes the explicit `save_as` command. Provenance is an
/// input manifest, not an invented `lunco:*` metadata field; a package may
/// persist it beside the USD file or in standard `assetInfo` authored by its
/// source tool.
fn publish_component(doc, root, edit_target, output, provenance) {
    if doc == () || edit_target == () || root == () || output == () {
        return failure("publish_component", "publish_component requires document, root, edit target, output, and provenance");
    }
    let valid_root = absolute_prim(root, "component root");
    if !valid_root.ok { return valid_root; }
    if root == "/" || root.split("/").len() != 2 {
        return failure("publish_component", "component root must be a top-level prim for standalone USD publication");
    }
    if type_of(output) != "string" || output == "" || output.contains("..") {
        return failure("publish_component", "output must be a non-empty safe Save-As path");
    }
    let provenance_result = valid_provenance(provenance);
    if !provenance_result.ok { return provenance_result; }
    let description = assembly_edit::describe(doc);
    if !query_ok(description) || description.generation == () {
        return failure("publish_component", "document generation is unavailable");
    }
    let inspection = assembly_edit::inspect(doc, root);
    if !query_ok(inspection) || inspection.prim == () || inspection.prim.exists != true {
        return failure("publish_component", root + ": component prim is missing");
    }
    if inspection.prim.type != "Xform" {
        return failure("publish_component", root + ": reusable component root must be an Xform");
    }
    let kind = metadata_value(inspection.prim, "kind");
    if kind != "component" {
        return failure("publish_component", root + ": standard USD kind must be `component`");
    }
    if document_default_prim(description) != root {
        return failure("publish_component", "document defaultPrim must select the component root");
    }
    let resolved = assembly_edit::resolve_target(doc, root, edit_target);
    if !query_ok(resolved) || resolved.edit_scope == "composed_read_only" {
        return failure("publish_component", root + ": publication target is not editable");
    }
    let full = query("QueryUsdPrim", #{
        doc_id: doc, path: root, children: false, schemas: true,
    });
    if !query_ok(full) { return failure("publish_component", root + ": schema read failed"); }
    let refs = reference_items(inspection.prim.references);
    for reference in refs {
        if reference.asset_path == () || reference.asset_path == "" {
            return failure("publish_component", root + ": a reference has no asset identity");
        }
    }
    let ops = [
        #{ SetDefaultPrim: #{ edit_target: edit_target, default_prim: root }},
        #{ SetPrimKind: #{ edit_target: edit_target, path: root, kind: "component" }},
    ];
    #{
        ok: true,
        kind: "publish_component",
        doc_id: doc,
        root: root,
        parent_generation: description.generation,
        api_schemas: full.api_schemas,
        references: refs,
        provenance: provenance_result.value,
        ops: ops,
        save_as: #{ command: "SaveAsDocument", doc_id: doc, path: output },
        apply: #{
            review: "assembly_edit::propose -> review_session -> commit_proposal",
            batch: "assembly_edit::batch",
            save: "assembly_edit::save_as_document",
        },
    }
}
