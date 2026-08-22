// The shipped generated Modelica schema.
//
// Rust reads the composed USD graph once and supplies these facts. This policy
// owns the generated source and the visual schema: changing the presentation,
// merge shape, or a domain-independent wiring rule does not require a Rust
// edit. The Rust seam still proves that the returned source only represents
// the composed members, boundary, units, and placements.

fn string_literal(value) {
    // USD identifiers and Modelica class names are validated before they reach
    // this policy; they do not contain quote characters.
    "\"" + value + "\""
}

fn color(r, g, b) {
    "{" + r.to_string() + "," + g.to_string() + "," + b.to_string() + "}"
}

fn rect(x1, y1, x2, y2) {
    "{{" + x1.to_string() + "," + y1.to_string() + "},{"
        + x2.to_string() + "," + y2.to_string() + "}}"
}

fn position(layout, path) {
    for item in layout {
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
    for item in layout {
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

fn extent(net, members) {
    let half_width = 300;
    let half_height = 200;
    for path in members {
        let p = position(net.layout.members, path);
        if p.x + 70 > half_width { half_width = p.x + 70; }
        if -p.x + 70 > half_width { half_width = -p.x + 70; }
        if p.y + 70 > half_height { half_height = p.y + 70; }
        if -p.y + 70 > half_height { half_height = -p.y + 70; }
    }
    let result = #{};
    result.width = half_width;
    result.height = half_height;
    result
}

fn text_graphic(x1, y1, x2, y2, text, size, text_color) {
    let graphic = "Text(extent=";
    graphic += rect(x1, y1, x2, y2);
    graphic += ", textString=";
    graphic += string_literal(text);
    graphic += ", textColor=";
    graphic += text_color;
    graphic += ", fontSize=";
    graphic += size.to_string();
    graphic += ")";
    graphic
}

fn endpoint_x(a, b, right) {
    if right {
        if a.x <= b.x { return a.x + 35; }
        return a.x - 35;
    }
    if a.x <= b.x { return b.x - 35; }
    b.x + 35
}

fn line_graphic(x1, y1, x2, y2) {
    let graphic = "Line(points=";
    graphic += rect(x1, y1, x2, y2);
    graphic += ", color=";
    graphic += color(45, 105, 75);
    graphic += ", thickness=2)";
    graphic
}

fn port_graphic(x, y) {
    let graphic = "Ellipse(extent=";
    graphic += rect(x - 4, y - 4, x + 4, y + 4);
    graphic += ", lineColor=";
    graphic += color(35, 75, 55);
    graphic += ", fillColor=";
    graphic += color(245, 250, 245);
    graphic += ", fillPattern=FillPattern.Solid)";
    graphic
}

fn node_graphic(x, y, component) {
    let rectangle = "Rectangle(extent=";
    rectangle += rect(x - 30, y - 14, x + 30, y + 14);
    rectangle += ", lineColor=";
    rectangle += color(45, 85, 70);
    rectangle += ", fillColor=";
    rectangle += color(184, 135, 196);
    rectangle += ", fillPattern=FillPattern.Solid, radius=4)";
    let graphics = [rectangle];
    graphics.push(text_graphic(x - 27, y - 9, x + 27, y + 9,
        component.instance, 11, color(25, 40, 35)));
    graphics.push(text_graphic(x - 27, y - 23, x + 27, y - 14,
        component.class, 7, color(45, 75, 60)));
    graphics
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

fn member_paths(net) {
    let paths = [];
    for component in net.components { paths.push(component.path); }
    paths
}

fn join_strings(values, separator) {
    let result = "";
    for index in 0..values.len {
        let value = values[index];
        if index > 0 { result += separator; }
        result += value;
    }
    result
}

fn graph_graphics(net, members, title) {
    let size = extent(net, members);
    let graphics = [];
    graphics.push("Rectangle(extent=" + rect(-size.width, -size.height, size.width, size.height)
        + ", lineColor=" + color(45, 85, 70)
        + ", fillColor=" + color(215, 235, 225)
        + ", fillPattern=FillPattern.Solid, radius=10)");
    graphics.push(text_graphic(-size.width + 10, size.height - 28, size.width - 10,
        size.height - 8, title, 13, color(35, 75, 55)));

    // A line and two explicit port markers are emitted for every authored
    // acausal edge. Nothing is collapsed by component family or domain name.
    for edge in net.connections {
        if !has_member(members, edge.left_path) || !has_member(members, edge.right_path) {
            continue;
        }
        let left = position(net.layout.members, edge.left_path);
        let right = position(net.layout.members, edge.right_path);
        let start_x = endpoint_x(left, right, true);
        let end_x = endpoint_x(left, right, false);
        graphics.push(line_graphic(start_x, left.y, end_x, right.y));
        graphics.push(port_graphic(start_x, left.y));
        graphics.push(port_graphic(end_x, right.y));
        graphics.push(text_graphic(start_x - 22, left.y + 6, start_x + 22, left.y + 18,
            edge.left_connector, 8, color(35, 75, 55)));
        graphics.push(text_graphic(end_x - 22, right.y + 6, end_x + 22, right.y + 18,
            edge.right_connector, 8, color(35, 75, 55)));
    }

    for path in members {
        let component = component_for(net, path);
        let p = position(net.layout.members, path);
        for graphic in node_graphic(p.x, p.y, component) { graphics.push(graphic); }
    }
    graphics
}

fn component_declaration(component, net) {
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
    let p = position(net.layout.members, component.path);
    line += " annotation(Placement(transformation(origin = {" + p.x.to_string()
        + ", " + p.y.to_string() + "}, extent = {{-10, -10}, {10, 10}})));\n";
    line
}

fn emit_unit(net, unit) {
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
        mutable += component_declaration(component_for(net, path), net);
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
            let line = "  ";
            line += member_output.alias;
            line += " = ";
            line += member_output.member_instance;
            line += ".";
            line += member_output.output;
            line += ";\n";
            mutable += line;
        }
    }
    let title = "COMPOSED TOPOLOGY\\n" + unit.components.len.to_string()
        + " component(s) | " + net.connections.len.to_string() + " connection(s)";
    let graphics = graph_graphics(net, unit.components, title);
    let size = extent(net, unit.components);
    mutable += "annotation(Icon(coordinateSystem(extent=" + rect(-size.width, -size.height,
        size.width, size.height) + "), graphics={" + join_strings(graphics, ", ")
        + "}), Diagram(coordinateSystem(extent=" + rect(-size.width, -size.height,
        size.width, size.height) + "), graphics={}));\n";
    mutable += "end " + unit.name + ";\n\n";
    mutable
}

fn synthesize(net) {
    let source = "model " + net.model_name + "\n";
    let mutable = source;
    for input in net.inputs { mutable += "  input Real " + input + ";\n"; }
    for boundary in net.boundary_outputs { mutable += "  output Real " + boundary.name + ";\n"; }
    for member_output in net.member_outputs {
        mutable += "  output Real " + member_output.alias + ";\n";
    }
    for unit in net.units {
        let p = unit_position(net.layout.units, unit.name);
        mutable += "  " + unit.name + " " + unit.instance
            + " annotation(Placement(transformation(origin = {" + p.x.to_string()
            + ", " + p.y.to_string() + "}, extent = {{-135, -68}, {135, 68}})));\n";
    }
    mutable += "equation\n";
    for unit in net.units {
        for input in unit.inputs {
            mutable += "  " + unit.instance + "." + input + " = " + input + ";\n";
        }
        for output in unit.outputs {
            mutable += "  " + output + " = " + unit.instance + "." + output + ";\n";
        }
    }
    for member_output in net.member_outputs {
        for unit in net.units {
            if has_member(unit.components, member_output.member_path) {
                mutable += member_output_equation(member_output, unit.instance);
            }
        }
    }
    let paths = member_paths(net);
    let graphics = graph_graphics(net, paths, "COMPOSED MODEL: " + net.model_name);
    let size = extent(net, member_paths(net));
    mutable += "annotation(Icon(coordinateSystem(extent=" + rect(-size.width, -size.height,
        size.width, size.height) + "), graphics={" + join_strings(graphics, ", ")
        + "}), Diagram(coordinateSystem(extent=" + rect(-size.width, -size.height,
        size.width, size.height) + "), graphics={}));\n";
    mutable += "end " + net.model_name + ";\n\n";
    for unit in net.units { mutable += emit_unit(net, unit); }
    #{ source: mutable, units: net.units, layout: net.layout }
}
