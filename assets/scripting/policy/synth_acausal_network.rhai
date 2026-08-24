// The shipped generated Modelica schema.
//
// Rust reads the composed USD graph once and supplies these facts. This policy
// owns the generated source, hierarchy, visual annotations, and layout. The
// Rust seam validates that the returned source represents only the composed
// members, boundary, units, and placements.

// Presentation policy is intentionally tunable here. These are diagram
// coordinates, extents, and typography choices—not Rust/runtime behavior.
const UNIT_POSITION_SCALE = 2;
const BATTERY_X = -360;
const SOLAR_PANEL_X = 420;
const MOTOR_COLUMN_SPACING = 260;
const MOTOR_FIRST_ROW_Y = 230;
const MOTOR_ROW_SPACING = 460;
const MOTOR_COLUMN_COUNT = 2;
const BUS_Y = 0;
const OTHER_ORIGIN_X = -120;
const OTHER_COLUMN_SPACING = 240;
const OTHER_ORIGIN_Y = 500;
const OTHER_ROW_SPACING = 180;
const OTHER_COLUMN_COUNT = 3;

const MEMBER_MIN_HALF_WIDTH = 180;
const MEMBER_MIN_HALF_HEIGHT = 130;
const MEMBER_EXTENT_MARGIN_X = 70;
const MEMBER_EXTENT_MARGIN_Y = 55;
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
const ICON_LABEL_HALF_WIDTH = 82;
const ICON_LABEL_HALF_HEIGHT = 16;
const ICON_CORNER_RADIUS_TEXT = "10";
const ICON_LABEL_FONT_SIZE_TEXT = "14";
const DIAGRAM_TITLE_INSET_X = 20;
const DIAGRAM_TITLE_TOP_INSET = 34;
const DIAGRAM_TITLE_BOTTOM_INSET = 12;
const DIAGRAM_TITLE_FONT_SIZE_TEXT = "16";
const ICON_TEXT_COLOR = "{255,255,255}";
const UNIT_ICON_LINE_COLOR = "{60,90,145}";
const UNIT_ICON_FILL_COLOR = "{75,130,190}";
const ROOT_ICON_LINE_COLOR = "{70,85,130}";
const ROOT_ICON_FILL_COLOR = "{105,120,180}";
const DIAGRAM_TITLE_COLOR = "{165,175,205}";

fn string_literal(value) {
    "\"" + value + "\""
}

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

fn is_class(component, leaf) {
    component.path.split("/").contains(leaf)
        || component.class.split(".").contains(leaf)
}

// The Rust facts deliberately use compact topology coordinates. The policy
// owns the presentation arrangement so a future policy can replace this with
// a domain-specific layout without changing Rust. Power networks get a
// source/load arrangement: the battery is the explicit bus anchor on the
// left, the panel is a source on the right, and the motors occupy two rows
// above/below the bus. This makes every generated connect() edge legible and
// prevents a routed solar edge from visually terminating at a motor.
fn visual_layout(net) {
    let layout = #{ units: [], members: [] };
    for item in net.layout.units {
        layout.units.push(#{
            name: item.name,
            x: item.x * UNIT_POSITION_SCALE,
            y: item.y * UNIT_POSITION_SCALE
        });
    }
    let motors = [];
    let battery = "";
    let solar = "";
    let other = [];
    for item in net.layout.members {
        let component = component_for(net, item.path);
        if is_class(component, "Battery") {
            battery = item.path;
        } else if is_class(component, "SolarPanel") {
            solar = item.path;
        } else if is_class(component, "DCMotor") || is_class(component, "Motor") {
            motors.push(item.path);
        } else {
            other.push(item.path);
        }
    }
    if battery != "" {
        layout.members.push(#{ path: battery, x: BATTERY_X, y: BUS_Y });
    }
    if solar != "" {
        layout.members.push(#{ path: solar, x: SOLAR_PANEL_X, y: BUS_Y });
    }
    let motor_index = 0;
    for path in motors {
        let column = motor_index % MOTOR_COLUMN_COUNT;
        let row = motor_index / MOTOR_COLUMN_COUNT;
        layout.members.push(#{
            path: path,
            x: column * MOTOR_COLUMN_SPACING,
            y: MOTOR_FIRST_ROW_Y - row * MOTOR_ROW_SPACING
        });
        motor_index += 1;
    }
    let other_index = 0;
    for path in other {
        let column = other_index % OTHER_COLUMN_COUNT;
        let row = other_index / OTHER_COLUMN_COUNT;
        let x = OTHER_ORIGIN_X + column * OTHER_COLUMN_SPACING;
        let y = OTHER_ORIGIN_Y + row * OTHER_ROW_SPACING;
        layout.members.push(#{
            path: path,
            x: x,
            y: y
        });
        other_index += 1;
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

fn compact_icon(title, line_color, fill_color) {
    let rectangle = "Rectangle(extent=" + rect(
            -ICON_BOX_HALF_WIDTH,
            -ICON_BOX_HALF_HEIGHT,
            ICON_BOX_HALF_WIDTH,
            ICON_BOX_HALF_HEIGHT
        )
        + ", lineColor=" + line_color
        + ", fillColor=" + fill_color
        + ", fillPattern=FillPattern.Solid, radius=" + ICON_CORNER_RADIUS_TEXT + ")";
    let name = "Text(extent=" + rect(
            -ICON_LABEL_HALF_WIDTH,
            -ICON_LABEL_HALF_HEIGHT,
            ICON_LABEL_HALF_WIDTH,
            ICON_LABEL_HALF_HEIGHT
        )
        + ", textString=" + string_literal(title)
        + ", textColor=" + ICON_TEXT_COLOR + ", fontSize=" + ICON_LABEL_FONT_SIZE_TEXT + ")";
    "Icon(coordinateSystem(extent=" + rect(
            -ICON_COORDINATE_HALF_WIDTH,
            -ICON_COORDINATE_HALF_HEIGHT,
            ICON_COORDINATE_HALF_WIDTH,
            ICON_COORDINATE_HALF_HEIGHT
        )
        + "), graphics={" + rectangle + "," + name + "})"
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
    line += ");\n";
    line
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
    let title = path_leaf(net.root) + " network | " + unit.components.len.to_string() + " member(s)";
    mutable += "annotation(" + compact_icon("Generated unit", UNIT_ICON_LINE_COLOR, UNIT_ICON_FILL_COLOR) + ","
        + diagram_annotation(size.width, size.height, title) + ");\n";
    mutable += "end " + unit.name + ";\n\n";
    mutable
}

fn synthesize(net) {
    let layout = visual_layout(net);
    let source = "model " + net.model_name + "\n";
    let mutable = source;
    for input in net.inputs { mutable += "  input Real " + input + ";\n"; }
    for boundary in net.boundary_outputs { mutable += "  output Real " + boundary.name + ";\n"; }
    for member_output in net.member_outputs { mutable += "  output Real " + member_output.alias + ";\n"; }
    for unit in net.units {
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
    for unit in net.units {
        for input in unit.inputs { mutable += "  " + unit.instance + "." + input + " = " + input + ";\n"; }
        for output in unit.outputs { mutable += "  " + output + " = " + unit.instance + "." + output + ";\n"; }
    }
    for member_output in net.member_outputs {
        for unit in net.units {
            if has_member(unit.components, member_output.member_path) {
                mutable += member_output_equation(member_output, unit.instance);
            }
        }
    }
    let size = unit_extent(layout, net.units);
    let title = "Generated network | " + net.units.len.to_string() + " unit(s)";
    mutable += "annotation(" + compact_icon("Generated network", ROOT_ICON_LINE_COLOR, ROOT_ICON_FILL_COLOR) + ","
        + diagram_annotation(size.width, size.height, title) + ");\n";
    mutable += "end " + net.model_name + ";\n\n";
    for unit in net.units { mutable += emit_unit(net, unit, layout); }
    #{
        source: mutable,
        units: net.units,
        layout: layout,
        source_roots: net.source_roots,
        // Promotion is policy-owned. The default policy exposes every
        // declared member output; a scenario policy can omit or rename these
        // entries without a Rust change.
        member_output_aliases: net.member_outputs,
    }
}
