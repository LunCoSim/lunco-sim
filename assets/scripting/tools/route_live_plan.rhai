// The active program owns its ordered, session-local waypoint plan. USD is
// only the initial definition; mutations are native pose, sensor and view
// operations. The editor can separately edit the authored definition.
import "lunco://scripting/tools/waypoint_editor" as waypoint_editor;

fn fail(message) { #{ok:false, error:message} }

fn local_position(owner, position) {
    let origin = world_pos3(owner);
    let rotation = world_rotation_quat(owner);
    if origin == () || rotation == () { return (); }
    vec3_array(qrot(quat_inverse(rotation), vsub(vec3_from(position), origin)))
}

fn world_position(owner, position) {
    let origin = world_pos3(owner);
    let rotation = world_rotation_quat(owner);
    if origin == () || rotation == () { return (); }
    vec3_array(vadd(origin, qrot(rotation, vec3_from(position))))
}

fn track(id, write) {
    if execution_context().clock == "simulation" {
        if write { track_entity_write(id); } else { track_entity_read(id); }
    }
}

fn seed(doc, route) {
    let plan = waypoint_editor::route_positions(doc, route);
    if plan.ok != true { return plan; }
    plan.route_path = route;
    plan.owner = find_path_in_document(doc, route);
    if plan.owner < 0 { return fail("live route owner is not mounted"); }
    track(plan.owner,false);
    plan.bindings = #{};
    plan.marker_ids = #{};
    plan.originals = #{};
    plan.next_key = 0;
    for i in 0..plan.paths.len() {
        let path = plan.paths[i];
        let target = find_path_in_document(doc, path);
        let visual = find_path_in_document(doc, path + "/Dome");
        let sensor = find_path_in_document(doc, path + "/Trigger");
        if target < 0 || visual < 0 || sensor < 0 {
            return fail("live route point is not admitted: " + path);
        }
        let source = query("QueryUsdPrim", #{doc_id:doc, path:path, attrs:["primvars:displayColor"]});
        let style = waypoint_editor::record_color(source);
        if style.ok != true { return style; }
        track(target,false);
        let position = world_pos(target);
        if position == () { return fail("live waypoint pose unavailable: " + path); }
        let local = local_position(plan.owner,position);
        if local == () { return fail("live waypoint frame unavailable: " + path); }
        let moved = !waypoint_editor::vector_equal(plan.points[i],local);
        if moved { plan.points[i] = local; }
        let binding = #{point_id:target, visual_id:visual, sensor_id:sensor, native_key:(), color:style.color};
        plan.bindings[path] = binding;
        plan.marker_ids[path] = visual;
        plan.originals[path] = #{binding:binding, moved:moved, retired:false};
    }
    plan
}

fn marker_request(plan, key, position, color) {
    // Same unlit dome and overlap dimensions as the reusable authored marker.
    // Live-created points are native scene objects, not composed references.
    cmd("SetRuntimeMarker", #{owner:plan.owner, key:key, marker:#{
        center:position, radius:1.5, color:color, opacity:0.42, sensor_radius:2.5}})
}

fn result_or_pending(plan, edit, receipt) {
    if receipt == () { return fail("live route action returned no result"); }
    if receipt.status == "pending" && receipt.id != () {
        return #{ok:true, pending:true, plan:plan, edit:edit, command_id:receipt.id};
    }
    if receipt.ok != true { return fail(receipt.error); }
    complete(plan, edit, receipt)
}

fn complete(plan, edit, receipt) {
    let action = edit.action;
    let key = edit.key;
    if action == "add" {
        let marker = if receipt.data == () { () } else { receipt.data.marker };
        if marker == () || marker.marker_id == () || marker.visual_id == () || marker.sensor_id == () {
            return fail("native route point admission returned no marker identities");
        }
        let binding = #{point_id:marker.marker_id.to_int(), visual_id:marker.visual_id.to_int(),
            sensor_id:marker.sensor_id.to_int(), native_key:edit.native_key, color:()};
        plan.bindings[key] = binding;
        plan.marker_ids[key] = binding.visual_id;
        plan.paths.insert(edit.index, key);
        plan.points.insert(edit.index, edit.local);
        plan.marker_colors.insert(edit.index, plan.color);
        plan.next_key += 1;
    } else if action == "move" {
        plan.points[edit.index] = edit.local;
        if plan.originals.contains(key) { plan.originals[key].moved = true; }
    } else if action == "delete" {
        plan.paths.remove(edit.index);
        plan.points.remove(edit.index);
        plan.marker_colors.remove(edit.index);
        plan.bindings.remove(key);
        plan.marker_ids.remove(key);
        if plan.originals.contains(key) { plan.originals[key].retired = true; }
    }
    #{ok:true, pending:false, plan:plan, key:key, action:action}
}

// Route operations are admitted by their actual program on its simulation
// clock. A pending native receipt is polled only until its terminal result.
fn edit(plan, request) {
    let action = request.action;
    let key = request.key;
    let index = -1;
    for i in 0..plan.paths.len() { if plan.paths[i] == key { index = i; break; } }
    track(plan.owner, false);
    if action == "color" {
        if request.color != () && !waypoint_editor::valid_color(request.color) { return fail("invalid live route color"); }
        if key == plan.route_path || key == () { plan.color = request.color; }
        else {
            if index < 0 { return fail("live color target is not in the route"); }
            plan.bindings[key].color = request.color;
        }
        for i in 0..plan.paths.len() {
            let binding = plan.bindings[plan.paths[i]];
            plan.marker_colors[i] = if binding.color != () { binding.color } else {plan.color};
        }
        return #{ok:true,pending:false,plan:plan,key:key,action:action};
    }
    if action == "add" || action == "before" || action == "after" {
        if plan.paths.len() >= 4096 { return fail("live route point budget exceeded"); }
        let position = request.position;
        let insertion = plan.paths.len();
        if action != "add" {
            if index < 0 { return fail("live insertion target is not in the route"); }
            let local = waypoint_editor::insertion_position(plan,key,action=="before");
            if local == () { return fail("live insertion needs an adjacent waypoint"); }
            position = world_position(plan.owner,local);
            insertion = if action=="before" {index} else {index+1};
        }
        if position == () { return fail("live waypoint has no active-frame position"); }
        let local = local_position(plan.owner,position);
        if local == () { return fail("live waypoint frame is unavailable"); }
        let native_key = "route-point-" + plan.next_key.to_string();
        let point_key = "live:" + plan.next_key.to_string();
        let receipt = marker_request(plan,native_key,position,[0.10,0.95,0.22]);
        return result_or_pending(plan,#{action:"add", key:point_key, native_key:native_key,
            index:insertion, local:local},receipt);
    }
    if index < 0 { return fail("live waypoint is no longer in the active plan"); }
    let binding = plan.bindings[key];
    if action == "move" {
        let local = local_position(plan.owner,request.position);
        if local == () { return fail("live waypoint frame is unavailable"); }
        let receipt = if binding.native_key != () {
            marker_request(plan,binding.native_key,request.position,[0.10,0.95,0.22])
        } else {
            track(binding.point_id,true);
            cmd("MoveEntity",#{entity_id:binding.point_id,translation:request.position,intent:"Interactive"})
        };
        return result_or_pending(plan,#{action:action,key:key,index:index,local:local},receipt);
    }
    if action == "delete" {
        let receipt = if binding.native_key != () {
            cmd("SetRuntimeMarker",#{owner:plan.owner,key:binding.native_key,marker:()})
        } else {
            track(binding.point_id,true);
            track(binding.sensor_id,true);
            let visibility = #{property:"visibility",value:#{Bool:#{value:false}}};
            let view = #{target:binding.point_id,writes:[visibility]};
            cmd("SetSceneParticipation",#{colliders:[#{target:binding.sensor_id,enabled:false}],views:[view]})
        };
        return result_or_pending(plan,#{action:action,key:key,index:index},receipt);
    }
    fail("unknown live route action: " + action.to_string())
}

fn finish_pending(pending) {
    let receipt = command_result(pending.command_id);
    if receipt == () || receipt.status == "pending" { return pending; }
    if receipt.ok != true { return fail(receipt.error); }
    complete(pending.plan,pending.edit,receipt)
}

// Authoring or ending the program discards the active plan. Restore only the
// authored points touched by it, using current composed definitions (which
// may have changed or removed a point). Native children retire at scene loss.
fn reset(doc, route, plan) {
    if plan == () { return #{ok:true}; }
    if find_path_in_document(doc,route) != plan.owner { return #{ok:true}; }
    let source = waypoint_editor::route_positions(doc,route);
    if source.ok != true { return source; }
    for key in plan.bindings.keys() {
        let binding = plan.bindings[key];
        if binding.native_key == () { continue; }
        let retired = cmd("SetRuntimeMarker",#{owner:plan.owner,key:binding.native_key,marker:()});
        if retired == () || retired.ok != true { return fail("cannot discard live route marker: " + key); }
    }
    for i in 0..source.paths.len() {
        let key = source.paths[i];
        if !plan.originals.contains(key) { continue; }
        let original = plan.originals[key];
        if !original.moved && !original.retired { continue; }
        let point = find_path_in_document(doc,key);
        let sensor = find_path_in_document(doc,key+"/Trigger");
        if point < 0 || sensor < 0 { return fail("cannot restore authored waypoint " + key); }
        if original.moved {
            let position = world_position(plan.owner,source.points[i]);
            if position == () { return fail("authored waypoint frame unavailable"); }
            track(point,true);
            let moved = cmd("MoveEntity",#{entity_id:point,translation:position,intent:"Interactive"});
            if moved == () || moved.ok != true { return fail("cannot restore authored waypoint pose: " + key); }
        }
        if original.retired {
            let authored = query("QueryUsdPrim",#{doc_id:doc,path:key+"/Trigger",attrs:["physics:collisionEnabled"]});
            if authored == () || authored.attrs == () || authored.attrs["physics:collisionEnabled"] == () {
                return fail("authored waypoint sensor state unavailable: " + key);
            }
            track(point,true); track(sensor,true);
            let view = #{target:point,writes:[#{property:"visibility",value:"Clear"}]};
            let restored = cmd("SetSceneParticipation",#{colliders:[#{target:sensor,enabled:authored.attrs["physics:collisionEnabled"]}],views:[view]});
            if restored == () || restored.ok != true { return fail("cannot restore authored waypoint participation: " + key); }
        }
    }
    #{ok:true}
}
