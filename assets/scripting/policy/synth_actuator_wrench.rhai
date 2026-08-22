// Generic force-actuator Modelica source and visual schema.
//
// Rust reads the composed USD actuator facts and validates geometry. This
// policy owns the emitted Modelica document, including its executable matrix
// solve and its inspectable icon/diagram.

fn string_literal(value) {
    "\"" + value + "\""
}

fn join_strings(values, separator) {
    let result = "";
    for index in 0..values.len {
        if index > 0 { result += separator; }
        result += values[index];
    }
    result
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
    mutable += "    upper_command = {" + vector_ones(net.actuator_count) + "});\n\n";
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

    let title = "USD FORCE-ACTUATOR ALLOCATION\\n" + net.actuator_count.to_string()
        + " actuator(s) | " + join_strings(net.actuator_paths, " | ");
    mutable += "annotation(Icon(coordinateSystem(extent={{-220,-160},{220,160}}), graphics={";
    mutable += "Rectangle(extent={{-85,-52},{85,52}}, lineColor={165,105,45}, fillColor={230,175,85}, fillPattern=FillPattern.Solid, radius=8),";
    mutable += "Text(extent={{-78,-16},{78,16}}, textString=";
    mutable += string_literal(net.model_name) + ", textColor={40,30,15}, fontSize=14)";
    mutable += "}), Diagram(coordinateSystem(extent={{-220,-160},{220,160}}), graphics={Text(extent={{-205,125},{205,150}}, textString=";
    mutable += string_literal(title) + ", textColor={125,85,35}, fontSize=12)}));\n";
    mutable += "end " + net.model_name + ";\n";
    #{ source: mutable }
}

fn vector_ones(count) {
    let result = "";
    for index in 0..count {
        if index > 0 { result += ", "; }
        result += "1.0";
    }
    result
}
