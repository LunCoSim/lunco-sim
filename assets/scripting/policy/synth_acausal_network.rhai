// The shipped generated Modelica schema.
//
// Rust reads the composed USD graph once and supplies these facts. This policy
// owns the generated source, hierarchy, visual annotations, and layout. The
// Rust seam validates that the returned source represents only the composed
// members, boundary, units, and placements.

// Presentation policy is intentionally tunable here. These are diagram
// coordinates, extents, and typography choices—not Rust/runtime behavior.
const UNIT_POSITION_SCALE = 2;
const HUB_X = -170;
const BUS_Y = 0;
const BRANCH_FIRST_X = 0;
const BRANCH_LEFT_X = -320;
const BRANCH_LEFT_FIRST_ROW_Y = -80;
const BRANCH_RIGHT_FIRST_ROW_Y = 80;
const BRANCH_COLUMN_SPACING = 180;
const BRANCH_ROW_SPACING = 120;
const BUS_RAIL_MARGIN = 35;

const MEMBER_MIN_HALF_WIDTH = 180;
const MEMBER_MIN_HALF_HEIGHT = 130;
const MEMBER_EXTENT_MARGIN_X = 70;
const MEMBER_EXTENT_MARGIN_Y = 80;
const UNIT_MIN_HALF_WIDTH = 240;
const UNIT_MIN_HALF_HEIGHT = 120;
const UNIT_EXTENT_MARGIN_X = 130;
const UNIT_EXTENT_MARGIN_Y = 75;

const COMPONENT_HALF_WIDTH = 55;
const COMPONENT_HALF_HEIGHT = 35;
const UNIT_HALF_WIDTH = 110;
const UNIT_HALF_HEIGHT = 55;

const ICON_COORDINATE_HALF_WIDTH = 100;
const ICON_COORDINATE_HALF_HEIGHT = 70;
const ICON_BOX_HALF_WIDTH = 92;
const ICON_BOX_HALF_HEIGHT = 58;
const ICON_CORNER_RADIUS_TEXT = "10";
const DIAGRAM_TITLE_INSET_X = 20;
const DIAGRAM_TITLE_TOP_INSET = 34;
const DIAGRAM_TITLE_BOTTOM_INSET = 12;
const DIAGRAM_TITLE_FONT_SIZE_TEXT = "22";
const DIAGRAM_BUS_FONT_SIZE_TEXT = "18";
const ICON_TEXT_COLOR = "{255,255,255}";
const UNIT_ICON_LINE_COLOR = "{60,90,145}";
const UNIT_ICON_FILL_COLOR = "{75,130,190}";
const ROOT_ICON_LINE_COLOR = "{70,85,130}";
const ROOT_ICON_FILL_COLOR = "{105,120,180}";
const DIAGRAM_TITLE_COLOR = "{165,175,205}";

fn string_literal(value) {
    "\"" + value + "\""
}

// Rhai hook functions resolve the long-lived presentation constants above.
// Keep the bus style behind named policy helpers so the route and the
// diagram decoration cannot drift apart.
fn power_rail_x() { HUB_X / 2 }
fn power_rail_color() { DIAGRAM_TITLE_COLOR }
fn power_wire_color() { DIAGRAM_TITLE_COLOR }
fn power_line_thickness_text() { "2" }

fn rect(x1, y1, x2, y2) {
    "{{" + x1.to_string() + "," + y1.to_string() + "},{"
        + x2.to_string() + "," + y2.to_string() + "}}"
}

fn position(layout, path) {
    for item in layout.members {
        if item.path == path {
            let result = #{};
            result.x = item.x;
            result.y = item.y;
            return result;
        }
    }
    throw "generated Modelica layout is missing member position: " + path
}

fn unit_position(layout, name) {
    for item in layout.units {
        if item.name == name {
            let result = #{};
            result.x = item.x;
            result.y = item.y;
            return result;
        }
    }
    throw "generated Modelica layout is missing unit position: " + name
}

fn has_member(members, path) {
    for member in members {
        if member == path { return true; }
    }
    false
}

fn component_for(net, path) {
    for component in net.components {
        if component.path == path { return component; }
    }
    throw "generated Modelica facts are missing component: " + path
}

fn path_leaf(path) {
    let leaf = path;
    for segment in path.split("/") {
        if segment != "" { leaf = segment; }
    }
    leaf
}

fn connection_degree(net, path, members) {
    let degree = 0;
    for edge in net.connections {
        if !members.contains(edge.left_path) || !members.contains(edge.right_path) {
            continue;
        }
        if edge.left_path == path { degree += 1; }
        if edge.right_path == path { degree += 1; }
    }
    degree
}

// Acausal flow has no compile-time source or sink: Pin.i can reverse sign at
// runtime. The topology role is therefore an explicit USD presentation fact,
// not an inference from a connector name or a Modelica class string. It only
// chooses a readable side of the visual bus; runtime Pin.i sign controls
// animated direction, never the diagram's geometry.
fn topology_role(net, path) {
    let component = component_for(net, path);
    if component.contains("topology_role") {
        component.topology_role
    } else {
        "neutral"
    }
}

fn hub_role_priority(net, path) {
    if topology_role(net, path) == "storage" { 1 } else { 0 }
}

fn stable_path_precedes(path, best) {
    if best == "" { return true; }
    path < best
}

fn should_replace_hub(path, degree, role_priority, best, best_degree, best_role_priority) {
    if degree != best_degree {
        return degree > best_degree;
    }
    if role_priority != best_role_priority {
        return role_priority > best_role_priority;
    }
    stable_path_precedes(path, best)
}

fn topology_hub(net, members) {
    let best = "";
    let best_degree = -1;
    let best_role_priority = -1;
    for path in members {
        let degree = connection_degree(net, path, members);
        let role_priority = hub_role_priority(net, path);
        let replace = should_replace_hub(
            path, degree, role_priority, best, best_degree, best_role_priority
        );
        if replace {
            best = path;
            best_degree = degree;
            best_role_priority = role_priority;
        }
    }
    if best == "" {
        throw "generated acausal unit has no topology members"
    }
    best
}

// Use a deterministic near-square bank. This keeps repeated components of
// the same type readable without giving the diagram a height proportional to
// the number of motors, thrusters, or other common-pin members.
fn branch_column_count(count) {
    if count <= 1 { 1 }
    else if count <= 4 { 2 }
    else if count <= 9 { 3 }
    else if count <= 16 { 4 }
    else { 5 }
}

fn branch_placement(path, side, index, count) {
    let columns = branch_column_count(count);
    let column = index % columns;
    let row = index / columns;
    let x = if side == "left" {
        BRANCH_LEFT_X - column * BRANCH_COLUMN_SPACING
    } else {
        BRANCH_FIRST_X + column * BRANCH_COLUMN_SPACING
    };
    let y = if side == "left" {
        BRANCH_LEFT_FIRST_ROW_Y - row * BRANCH_ROW_SPACING
    } else {
        BRANCH_RIGHT_FIRST_ROW_Y + row * BRANCH_ROW_SPACING
    };
    #{
        path: path,
        x: x,
        y: y
    }
}

fn layout_groups(net, units) {
    let groups = [];
    if units.len == 0 {
        let all = [];
        for component in net.components { all.push(component.path); }
        groups.push(all);
    } else {
        for unit in units { groups.push(unit.components); }
    }
    groups
}

fn classify_members(net, members, hub) {
    let result = #{ sources: [], loads: [], neutral: [] };
    for path in members {
        if path == hub { continue; }
        let role = topology_role(net, path);
        if role == "source" {
            result.sources.push(path);
        } else if role == "load" {
            result.loads.push(path);
        } else {
            result.neutral.push(path);
        }
    }
    result
}

fn group_member_layout(net, members) {
    let placements = [];
    let hub = topology_hub(net, members);
    placements.push(#{ path: hub, x: HUB_X, y: BUS_Y });
    let classified = classify_members(net, members, hub);
    // Explicit source/load roles form opposite banks of the bus. This is a
    // semantic layout policy, not an electrical direction assumption: the
    // generated equations remain the authored acausal connect graph.
    let left_count = 0;
    let left_total = classified.sources.len + classified.neutral.len;
    for path in classified.sources {
        placements.push(branch_placement(path, "left", left_count, left_total));
        left_count += 1;
    }
    let right_count = 0;
    let right_total = classified.loads.len + classified.neutral.len;
    for path in classified.loads {
        placements.push(branch_placement(path, "right", right_count, right_total));
        right_count += 1;
    }
    // Components without a side preference remain fully supported. Pack them
    // onto the shorter bank, with a deterministic left tie, so a new Modelica
    // domain does not need a Rust layout branch merely because it has neutral
    // components.
    for path in classified.neutral {
        if left_count <= right_count {
            placements.push(branch_placement(path, "left", left_count, left_total));
            left_count += 1;
        } else {
            placements.push(branch_placement(path, "right", right_count, right_total));
            right_count += 1;
        }
    }
    placements
}

fn visual_layout(net, units) {
    let layout = #{ units: [], members: [] };
    for unit in units {
        // `source_name` is the partition identity from the Rust facts. `name`
        // is the readable Modelica class name presented to the user.
        let item = unit_position(net.layout, unit.source_name);
        layout.units.push(#{
            name: unit.name,
            x: item.x * UNIT_POSITION_SCALE,
            y: item.y * UNIT_POSITION_SCALE
        });
    }
    // Unit positions belong to the generated root diagram. Member positions
    // belong to each unit's own diagram, so every group gets the same local
    // coordinate system; root-level spacing never leaks into a child class.
    for members in layout_groups(net, units) {
        for placement in group_member_layout(net, members) {
            layout.members.push(placement);
        }
    }
    layout
}

fn member_extent(layout, members) {
    let half_width = MEMBER_MIN_HALF_WIDTH;
    let half_height = MEMBER_MIN_HALF_HEIGHT;
    for path in members {
        let p = position(layout, path);
        if p.x + MEMBER_EXTENT_MARGIN_X > half_width {
            half_width = p.x + MEMBER_EXTENT_MARGIN_X;
        }
        if -p.x + MEMBER_EXTENT_MARGIN_X > half_width {
            half_width = -p.x + MEMBER_EXTENT_MARGIN_X;
        }
        if p.y + MEMBER_EXTENT_MARGIN_Y > half_height {
            half_height = p.y + MEMBER_EXTENT_MARGIN_Y;
        }
        if -p.y + MEMBER_EXTENT_MARGIN_Y > half_height {
            half_height = -p.y + MEMBER_EXTENT_MARGIN_Y;
        }
    }
    let result = #{};
    result.width = half_width;
    result.height = half_height;
    result
}

fn unit_extent(layout, units) {
    let half_width = UNIT_MIN_HALF_WIDTH;
    let half_height = UNIT_MIN_HALF_HEIGHT;
    for unit in units {
        let p = unit_position(layout, unit.name);
        if p.x + UNIT_EXTENT_MARGIN_X > half_width {
            half_width = p.x + UNIT_EXTENT_MARGIN_X;
        }
        if -p.x + UNIT_EXTENT_MARGIN_X > half_width {
            half_width = -p.x + UNIT_EXTENT_MARGIN_X;
        }
        if p.y + UNIT_EXTENT_MARGIN_Y > half_height {
            half_height = p.y + UNIT_EXTENT_MARGIN_Y;
        }
        if -p.y + UNIT_EXTENT_MARGIN_Y > half_height {
            half_height = -p.y + UNIT_EXTENT_MARGIN_Y;
        }
    }
    let result = #{};
    result.width = half_width;
    result.height = half_height;
    result
}

fn compact_icon(line_color, fill_color) {
    let rectangle = "Rectangle(extent=" + rect(
            -ICON_BOX_HALF_WIDTH,
            -ICON_BOX_HALF_HEIGHT,
            ICON_BOX_HALF_WIDTH,
            ICON_BOX_HALF_HEIGHT
        )
        + ", lineColor=" + line_color
        + ", fillColor=" + fill_color
        + ", fillPattern=FillPattern.Solid, radius=" + ICON_CORNER_RADIUS_TEXT + ")";
    let bus = "Line(points={{-52,0},{52,0}}, color={245,250,255}, thickness=2)";
    let left_node = "Ellipse(extent={{-68,-12},{-44,12}}, lineColor="
        + line_color + ", fillColor=" + ICON_TEXT_COLOR + ", fillPattern=FillPattern.Solid)";
    let right_node = "Ellipse(extent={{44,-12},{68,12}}, lineColor="
        + line_color + ", fillColor=" + ICON_TEXT_COLOR + ", fillPattern=FillPattern.Solid)";
    "Icon(coordinateSystem(extent=" + rect(
            -ICON_COORDINATE_HALF_WIDTH,
            -ICON_COORDINATE_HALF_HEIGHT,
            ICON_COORDINATE_HALF_WIDTH,
            ICON_COORDINATE_HALF_HEIGHT
        )
        + "), graphics={" + rectangle + "," + bus + "," + left_node + "," + right_node + "})"
}

fn diagram_annotation(width, height, title) {
    let heading = "Text(extent=" + rect(
            -width + DIAGRAM_TITLE_INSET_X,
            height - DIAGRAM_TITLE_TOP_INSET,
            width - DIAGRAM_TITLE_INSET_X,
            height - DIAGRAM_TITLE_BOTTOM_INSET
        )
        + ", textString=" + string_literal(title)
        + ", textColor=" + DIAGRAM_TITLE_COLOR + ", fontSize=" + DIAGRAM_TITLE_FONT_SIZE_TEXT + ")";
    "Diagram(coordinateSystem(extent=" + rect(-width, -height, width, height)
        + "), graphics={" + heading + "})"
}

fn connect_equation(edge) {
    let line = "  connect(";
    line += edge.left_instance;
    line += ".";
    line += edge.left_connector;
    line += ", ";
    line += edge.right_instance;
    line += ".";
    line += edge.right_connector;
    line += ")";
    line
}

fn branch_feeder_x(x, rail_x) {
    if x < rail_x {
        x + COMPONENT_HALF_WIDTH + 18
    } else {
        x - COMPONENT_HALF_WIDTH - 18
    }
}

fn append_point(line, x, y) {
    line += ",{"
        + x.to_string()
        + ","
        + y.to_string()
        + "}";
    line
}

// Keep common-pin electrical wiring legible. The authored topology remains
// the connect() equation; these are standard Modelica Line waypoints only.
// Every branch visits its own feeder lane and then the central bus, so a
// SolarPanel-to-motor equation cannot render as a direct wire or cut through
// another repeated component.
fn power_bus_annotation(edge, layout) {
    let left = position(layout, edge.left_path);
    let right = position(layout, edge.right_path);
    let rail_x = power_rail_x();
    // Include the horizontal icon stubs as well as the rail crossing. A
    // two-point vertical annotation leaves the renderer's default straight
    // wire visible, making a panel appear directly connected to a motor.
    let left_port_x = if left.x < rail_x {
        left.x + COMPONENT_HALF_WIDTH
    } else {
        left.x - COMPONENT_HALF_WIDTH
    };
    let right_port_x = if right.x < rail_x {
        right.x + COMPONENT_HALF_WIDTH
    } else {
        right.x - COMPONENT_HALF_WIDTH
    };
    let line = " annotation(Line(points={{";
    line += left_port_x.to_string();
    line += ",";
    line += left.y.to_string();
    line += "}";
    if left.x == HUB_X && left.y == BUS_Y {
        line = append_point(line, rail_x, left.y);
    } else {
        let feeder_x = branch_feeder_x(left.x, rail_x);
        line = append_point(line, feeder_x, left.y);
        line = append_point(line, feeder_x, BUS_Y);
        line = append_point(line, rail_x, BUS_Y);
    }
    if right.x == HUB_X && right.y == BUS_Y {
        line = append_point(line, rail_x, right.y);
    } else {
        let feeder_x = branch_feeder_x(right.x, rail_x);
        line = append_point(line, rail_x, BUS_Y);
        line = append_point(line, feeder_x, BUS_Y);
        line = append_point(line, feeder_x, right.y);
    }
    line = append_point(line, right_port_x, right.y);
    line += "}, color=";
    line += power_wire_color();
    line += ", thickness=";
    line += power_line_thickness_text();
    line += "))";
    line
}

fn bus_bounds(layout, members) {
    let min_y = 0;
    let max_y = 0;
    let first = true;
    for path in members {
        let p = position(layout, path);
        if first {
            min_y = p.y;
            max_y = p.y;
            first = false;
        } else {
            if p.y < min_y { min_y = p.y; }
            if p.y > max_y { max_y = p.y; }
        }
    }
    let result = #{};
    result.min_y = min_y - BUS_RAIL_MARGIN;
    result.max_y = max_y + BUS_RAIL_MARGIN;
    result
}

fn unit_bus_graphics(layout, members) {
    let bounds = bus_bounds(layout, members);
    let rail = "Line(points={{";
    rail += power_rail_x().to_string();
    rail += ",";
    rail += bounds.min_y.to_string();
    rail += "},{";
    rail += power_rail_x().to_string();
    rail += ",";
    rail += bounds.max_y.to_string();
    rail += "}}, color=";
    rail += power_rail_color();
    rail += ", thickness=";
    rail += power_line_thickness_text();
    rail += ")";
    let label_y = bounds.max_y + 22;
    let label = "Text(extent=" + rect(
            power_rail_x() - 75,
            label_y,
            power_rail_x() + 75,
            label_y + 20
        )
        + ", textString=\"POWER BUS\", textColor=" + power_rail_color() + ", fontSize=" + DIAGRAM_BUS_FONT_SIZE_TEXT + ")";
    rail + "," + label
}

fn policy_units(net) {
    let result = [];
    let index = 0;
    for unit in net.units {
        let instance = "power_unit_" + (index + 1).to_string();
        if net.units.len == 1 { instance = "power_system"; }
        let class_name = "PowerUnit_" + (index + 1).to_string();
        if net.units.len == 1 { class_name = "PowerSystem"; }
        let rewritten = #{
            name: class_name,
            source_name: unit.name,
            instance: instance,
            components: unit.components,
            inputs: unit.inputs,
            outputs: unit.outputs
        };
        result.push(rewritten);
        index += 1;
    }
    result
}

fn boundary_equation(link) {
    let line = "  ";
    line += link.target_instance;
    line += ".";
    line += link.target_input;
    line += " = ";
    line += link.input;
    line += ";\n";
    line
}

fn causal_equation(link) {
    let line = "  ";
    line += link.target_instance;
    line += ".";
    line += link.target_input;
    line += " = ";
    line += link.source_instance;
    line += ".";
    line += link.source_output;
    line += ";\n";
    line
}

fn output_equation(output) {
    let line = "  ";
    line += output.name;
    line += " = ";
    line += output.source_instance;
    line += ".";
    line += output.source_output;
    line += ";\n";
    line
}

fn member_output_equation(output, instance) {
    let line = "  ";
    line += output.alias;
    line += " = ";
    line += instance;
    line += ".";
    line += output.alias;
    line += ";\n";
    line
}

fn component_declaration(component, layout) {
    let line = "  " + component.class + " " + component.instance;
    if component.constant_modifications.len > 0 {
        line += "(";
        for index in 0..component.constant_modifications.len {
            let modification = component.constant_modifications[index];
            if index > 0 { line += ", "; }
            line += modification.name + " = " + modification.value.to_string();
        }
        line += ")";
    }
    let p = position(layout, component.path);
    let component_extent = rect(
        -COMPONENT_HALF_WIDTH,
        -COMPONENT_HALF_HEIGHT,
        COMPONENT_HALF_WIDTH,
        COMPONENT_HALF_HEIGHT
    );
    let origin = p.x.to_string() + ", " + p.y.to_string();
    let placement = " annotation(Placement(transformation(origin = {"
        + origin + "}, extent = " + component_extent + ")));\n";
    line += placement;
    line
}

fn emit_unit(net, unit, layout) {
    let source = "model " + unit.name + "\n";
    let mutable = source;
    for input in unit.inputs { mutable += "  input Real " + input + ";\n"; }
    for output in unit.outputs { mutable += "  output Real " + output + ";\n"; }
    for member_output in net.member_outputs {
        if has_member(unit.components, member_output.member_path) {
            mutable += "  output Real " + member_output.alias + ";\n";
        }
    }
    for path in unit.components {
        mutable += component_declaration(component_for(net, path), layout);
    }
    mutable += "equation\n";
    for edge in net.connections {
        if has_member(unit.components, edge.left_path) && has_member(unit.components, edge.right_path) {
            mutable += connect_equation(edge);
            mutable += power_bus_annotation(edge, layout);
            mutable += ";\n";
        }
    }
    for link in net.boundary_links {
        if has_member(unit.components, link.target_path) {
            mutable += boundary_equation(link);
        }
    }
    for link in net.causal_links {
        if has_member(unit.components, link.source_path) && has_member(unit.components, link.target_path) {
            mutable += causal_equation(link);
        }
    }
    for boundary in net.boundary_outputs {
        if has_member(unit.components, boundary.source_path) {
            mutable += output_equation(boundary);
        }
    }
    for member_output in net.member_outputs {
        if has_member(unit.components, member_output.member_path) {
            let line = "  " + member_output.alias;
            line += " = ";
            line += member_output.member_instance;
            line += ".";
            line += member_output.output;
            line += ";\n";
            mutable += line;
        }
    }
    let size = member_extent(layout, unit.components);
    mutable += "annotation(" + compact_icon(UNIT_ICON_LINE_COLOR, UNIT_ICON_FILL_COLOR) + ","
        + "Diagram(coordinateSystem(extent=" + rect(-size.width, -size.height, size.width, size.height)
        + "), graphics={" + unit_bus_graphics(layout, unit.components) + "})" + ");\n";
    mutable += "end " + unit.name + ";\n\n";
    mutable
}

fn synthesize(net) {
    let units = policy_units(net);
    let layout = visual_layout(net, units);
    let source = "model " + net.model_name + "\n";
    let mutable = source;
    for input in net.inputs { mutable += "  input Real " + input + ";\n"; }
    for boundary in net.boundary_outputs { mutable += "  output Real " + boundary.name + ";\n"; }
    for member_output in net.member_outputs { mutable += "  output Real " + member_output.alias + ";\n"; }
    for unit in units {
        let p = unit_position(layout, unit.name);
        let unit_extent = rect(
            -UNIT_HALF_WIDTH,
            -UNIT_HALF_HEIGHT,
            UNIT_HALF_WIDTH,
            UNIT_HALF_HEIGHT
        );
        let origin = p.x.to_string() + ", " + p.y.to_string();
        let placement = " annotation(Placement(transformation(origin = {"
            + origin + "}, extent = " + unit_extent + ")));\n";
        mutable += "  " + unit.name + " " + unit.instance + placement;
    }
    mutable += "equation\n";
    for unit in units {
        for input in unit.inputs { mutable += "  " + unit.instance + "." + input + " = " + input + ";\n"; }
        for output in unit.outputs { mutable += "  " + output + " = " + unit.instance + "." + output + ";\n"; }
    }
    for member_output in net.member_outputs {
        for unit in units {
            if has_member(unit.components, member_output.member_path) {
                mutable += member_output_equation(member_output, unit.instance);
            }
        }
    }
    let size = unit_extent(layout, units);
    let title = path_leaf(net.root) + " network | " + units.len.to_string() + " unit(s)";
    mutable += "annotation(" + compact_icon(ROOT_ICON_LINE_COLOR, ROOT_ICON_FILL_COLOR) + ","
        + diagram_annotation(size.width, size.height, title) + ");\n";
    mutable += "end " + net.model_name + ";\n\n";
    for unit in units { mutable += emit_unit(net, unit, layout); }
    #{
        source: mutable,
        units: units,
        layout: layout,
        source_roots: net.source_roots,
        // Promotion is policy-owned. The default policy exposes every
        // declared member output; a scenario policy can omit or rename these
        // entries without a Rust change.
        member_output_aliases: net.member_outputs,
    }
}
