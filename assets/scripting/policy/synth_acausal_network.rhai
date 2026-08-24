// The shipped generated Modelica schema.
//
// Rust reads the composed USD graph once and supplies these facts. This policy
// owns the generated source, hierarchy, visual annotations, and layout. The
// Rust seam validates that the returned source represents only the composed
// members, boundary, units, and placements.

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
    let result = #{};
    result.x = 0;
    result.y = 0;
    result
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
    let result = #{};
    result.x = 0;
    result.y = 0;
    result
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
    #{}
}

// The Rust facts deliberately use compact topology coordinates. The policy
// owns the presentation scale so a future policy can replace this with a
// domain-specific arrangement without changing Rust.
fn visual_layout(net) {
    let layout = #{ units: [], members: [] };
    for item in net.layout.units {
        layout.units.push(#{ name: item.name, x: item.x * 2, y: item.y * 2 });
    }
    for item in net.layout.members {
        layout.members.push(#{ path: item.path, x: item.x * 2, y: item.y * 4 });
    }
    layout
}

fn member_extent(layout, members) {
    let half_width = 180;
    let half_height = 130;
    for path in members {
        let p = position(layout, path);
        if p.x + 70 > half_width { half_width = p.x + 70; }
        if -p.x + 70 > half_width { half_width = -p.x + 70; }
        if p.y + 55 > half_height { half_height = p.y + 55; }
        if -p.y + 55 > half_height { half_height = -p.y + 55; }
    }
    let result = #{};
    result.width = half_width;
    result.height = half_height;
    result
}

fn unit_extent(layout, units) {
    let half_width = 240;
    let half_height = 120;
    for unit in units {
        let p = unit_position(layout, unit.name);
        if p.x + 130 > half_width { half_width = p.x + 130; }
        if -p.x + 130 > half_width { half_width = -p.x + 130; }
        if p.y + 75 > half_height { half_height = p.y + 75; }
        if -p.y + 75 > half_height { half_height = -p.y + 75; }
    }
    let result = #{};
    result.width = half_width;
    result.height = half_height;
    result
}

fn compact_icon(line_color, fill_color) {
    let rectangle = "Rectangle(extent=" + rect(-92, -58, 92, 58)
        + ", lineColor=" + line_color
        + ", fillColor=" + fill_color
        + ", fillPattern=FillPattern.Solid, radius=10)";
    let name = "Text(extent=" + rect(-82, -16, 82, 16)
        + ", textString=" + string_literal("%name")
        + ", textColor={255,255,255}, fontSize=14)";
    "Icon(coordinateSystem(extent=" + rect(-100, -70, 100, 70)
        + "), graphics={" + rectangle + "," + name + "})"
}

fn diagram_annotation(width, height, title) {
    let heading = "Text(extent=" + rect(-width + 20, height - 34, width - 20, height - 12)
        + ", textString=" + string_literal(title)
        + ", textColor={75,85,110}, fontSize=12)";
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
    line += " annotation(Placement(transformation(origin = {" + p.x.to_string()
        + ", " + p.y.to_string() + "}, extent = {{-55, -35}, {55, 35}})));\n";
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
    let title = "Unit " + unit.name + " | " + unit.components.len.to_string() + " member(s)";
    mutable += "annotation(" + compact_icon("{60,90,145}", "{75,130,190}") + ","
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
        mutable += "  " + unit.name + " " + unit.instance
            + " annotation(Placement(transformation(origin = {" + p.x.to_string()
            + ", " + p.y.to_string() + "}, extent = {{-110, -55}, {110, 55}})));\n";
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
    mutable += "annotation(" + compact_icon("{70,85,130}", "{105,120,180}") + ","
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
