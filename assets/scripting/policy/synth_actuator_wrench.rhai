// Generic force-actuator Modelica source and visual schema.
//
// Rust reads the composed USD actuator facts and validates geometry. This
// policy owns the emitted Modelica document, including its executable matrix
// solve and its inspectable icon/diagram.

fn string_literal(value) {
    "\"" + value + "\""
}

fn rect(x1, y1, x2, y2) {
    "{{" + x1.to_string() + "," + y1.to_string() + "},{"
        + x2.to_string() + "," + y2.to_string() + "}}"
}

fn compact_icon(title) {
    let rectangle = "Rectangle(extent=" + rect(-92, -58, 92, 58)
        + ", lineColor={150,95,35}, fillColor={225,165,75}"
        + ", fillPattern=FillPattern.Solid, radius=10)";
    let name = "Text(extent=" + rect(-82, -16, 82, 16)
        + ", textString=" + string_literal(title)
        + ", textColor={255,255,255}, fontSize=14)";
    "Icon(coordinateSystem(extent=" + rect(-100, -70, 100, 70)
        + "), graphics={" + rectangle + "," + name + "})"
}

fn diagram_annotation(title) {
    let heading = "Text(extent=" + rect(-180, 115, 180, 140)
        + ", textString=" + string_literal(title)
        + ", textColor={120,80,35}, fontSize=12)";
    "Diagram(coordinateSystem(extent=" + rect(-200, -140, 200, 150)
        + "), graphics={" + heading + "})"
}

fn vector_zeros(count) {
    let result = "";
    for index in 0..count {
        if index > 0 { result += ", "; }
        result += "0.0";
    }
    result
}

fn matrix_literal(matrix) {
    let result = "[";
    for row_index in 0..matrix.len {
        if row_index > 0 { result += "; "; }
        let row = matrix[row_index];
        for column_index in 0..row.len {
            if column_index > 0 { result += ", "; }
            result += row[column_index].to_string();
        }
    }
    result + "]"
}

fn synthesize(net) {
    let source = "model " + net.model_name + "\n";
    let mutable = source;
    for input in net.inputs { mutable += "  input Real " + input + ";\n"; }
    for output in net.outputs { mutable += "  output Real " + output + ";\n"; }
    mutable += "  LunCo.Actuation.WrenchAllocator allocator(\n";
    mutable += "    actuator_count = " + net.actuator_count.to_string() + ",\n";
    mutable += "    allocation_iterations = 64,\n";
    mutable += "    allocation_step = " + net.allocation_step.to_string() + ",\n";
    mutable += "    wrench_matrix = " + matrix_literal(net.wrench_matrix) + ",\n";
    mutable += "    lower_command = {" + vector_zeros(net.actuator_count) + "},\n";
    mutable += "    upper_command = {" + vector_ones(net.actuator_count)
        + "}) annotation(Placement(transformation(origin = {0, 0}, extent = {{-110, -70}, {110, 70}})));\n\n";
    mutable += "equation\n";

    let demanded = [
        "desired_force_x", "desired_force_y", "desired_force_z",
        "desired_torque_x", "desired_torque_y", "desired_torque_z"
    ];
    for name in demanded {
        let value = "0.0";
        for input in net.inputs {
            if input == name { value = input; }
        }
        mutable += "  allocator." + name + " = " + value + ";\n";
    }
    for index in 0..net.outputs.len {
        mutable += "  " + net.outputs[index] + " = allocator.command[";
        mutable += (index + 1).to_string() + "];\n";
    }

    let title = "Force allocation | " + net.actuator_count.to_string() + " actuator(s)";
    mutable += "annotation(" + compact_icon(net.model_name) + ","
        + diagram_annotation(title) + ");\n";
    mutable += "end " + net.model_name + ";\n";
    #{ source: mutable, source_roots: ["LunCo"] }
}

fn vector_ones(count) {
    let result = "";
    for index in 0..count {
        if index > 0 { result += ", "; }
        result += "1.0";
    }
    result
}
