// Generic force-actuator Modelica source and visual schema.
//
// Rust reads the composed USD actuator facts and validates geometry. This
// policy owns the emitted Modelica document, including its executable matrix
// solve and its inspectable icon/diagram.

// Presentation and solver policy belong in Rhai so they can be tuned without
// changing the Rust host or the generic projection seam.
const WRENCH_ALLOCATION_ITERATIONS_TEXT = "64";
const WRENCH_ICON_COORDINATE_HALF_WIDTH = 100;
const WRENCH_ICON_COORDINATE_HALF_HEIGHT = 70;
const WRENCH_ICON_BOX_HALF_WIDTH = 92;
const WRENCH_ICON_BOX_HALF_HEIGHT = 58;
const WRENCH_ICON_LABEL_HALF_WIDTH = 82;
const WRENCH_ICON_LABEL_HALF_HEIGHT = 16;
const WRENCH_ICON_CORNER_RADIUS_TEXT = "10";
const WRENCH_ICON_LABEL_FONT_SIZE_TEXT = "14";
const WRENCH_DIAGRAM_TITLE_HALF_WIDTH = 180;
const WRENCH_DIAGRAM_TITLE_BOTTOM = 115;
const WRENCH_DIAGRAM_TITLE_TOP = 140;
const WRENCH_DIAGRAM_TITLE_FONT_SIZE_TEXT = "12";
const WRENCH_DIAGRAM_HALF_WIDTH = 200;
const WRENCH_DIAGRAM_BOTTOM = 140;
const WRENCH_DIAGRAM_TOP = 150;
const WRENCH_COMPONENT_HALF_WIDTH = 110;
const WRENCH_COMPONENT_HALF_HEIGHT = 70;
const WRENCH_ICON_LINE_COLOR = "{150,95,35}";
const WRENCH_ICON_FILL_COLOR = "{225,165,75}";
const WRENCH_ICON_TEXT_COLOR = "{255,255,255}";
const WRENCH_DIAGRAM_TITLE_COLOR = "{120,80,35}";
const WRENCH_VECTOR_ZERO_TEXT = "0.0";
const WRENCH_VECTOR_ONE_TEXT = "1.0";
const WRENCH_DEFAULT_DEMAND_TEXT = "0.0";
const WRENCH_ORIGIN_TEXT = "0, 0";

fn string_literal(value) {
    "\"" + value + "\""
}

fn rect(x1, y1, x2, y2) {
    "{{" + x1.to_string() + "," + y1.to_string() + "},{"
        + x2.to_string() + "," + y2.to_string() + "}}"
}

fn compact_icon(title) {
    let rectangle = "Rectangle(extent=" + rect(
            -WRENCH_ICON_BOX_HALF_WIDTH,
            -WRENCH_ICON_BOX_HALF_HEIGHT,
            WRENCH_ICON_BOX_HALF_WIDTH,
            WRENCH_ICON_BOX_HALF_HEIGHT
        )
        + ", lineColor=" + WRENCH_ICON_LINE_COLOR + ", fillColor=" + WRENCH_ICON_FILL_COLOR
        + ", fillPattern=FillPattern.Solid, radius=" + WRENCH_ICON_CORNER_RADIUS_TEXT + ")";
    let name = "Text(extent=" + rect(
            -WRENCH_ICON_LABEL_HALF_WIDTH,
            -WRENCH_ICON_LABEL_HALF_HEIGHT,
            WRENCH_ICON_LABEL_HALF_WIDTH,
            WRENCH_ICON_LABEL_HALF_HEIGHT
        )
        + ", textString=" + string_literal(title)
        + ", textColor=" + WRENCH_ICON_TEXT_COLOR + ", fontSize=" + WRENCH_ICON_LABEL_FONT_SIZE_TEXT + ")";
    "Icon(coordinateSystem(extent=" + rect(
            -WRENCH_ICON_COORDINATE_HALF_WIDTH,
            -WRENCH_ICON_COORDINATE_HALF_HEIGHT,
            WRENCH_ICON_COORDINATE_HALF_WIDTH,
            WRENCH_ICON_COORDINATE_HALF_HEIGHT
        )
        + "), graphics={" + rectangle + "," + name + "})"
}

fn diagram_annotation(title) {
    let heading = "Text(extent=" + rect(
            -WRENCH_DIAGRAM_TITLE_HALF_WIDTH,
            WRENCH_DIAGRAM_TITLE_BOTTOM,
            WRENCH_DIAGRAM_TITLE_HALF_WIDTH,
            WRENCH_DIAGRAM_TITLE_TOP
        )
        + ", textString=" + string_literal(title)
        + ", textColor=" + WRENCH_DIAGRAM_TITLE_COLOR + ", fontSize=" + WRENCH_DIAGRAM_TITLE_FONT_SIZE_TEXT + ")";
    "Diagram(coordinateSystem(extent=" + rect(
            -WRENCH_DIAGRAM_HALF_WIDTH,
            -WRENCH_DIAGRAM_BOTTOM,
            WRENCH_DIAGRAM_HALF_WIDTH,
            WRENCH_DIAGRAM_TOP
        )
        + "), graphics={" + heading + "})"
}

fn vector_zeros(count) {
    let result = "";
    for index in 0..count {
        if index > 0 { result += ", "; }
        result += WRENCH_VECTOR_ZERO_TEXT;
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
    mutable += "    allocation_iterations = " + WRENCH_ALLOCATION_ITERATIONS_TEXT + ",\n";
    mutable += "    allocation_step = " + net.allocation_step.to_string() + ",\n";
    mutable += "    wrench_matrix = " + matrix_literal(net.wrench_matrix) + ",\n";
    mutable += "    lower_command = {" + vector_zeros(net.actuator_count) + "},\n";
    let allocator_extent = rect(
        -WRENCH_COMPONENT_HALF_WIDTH,
        -WRENCH_COMPONENT_HALF_HEIGHT,
        WRENCH_COMPONENT_HALF_WIDTH,
        WRENCH_COMPONENT_HALF_HEIGHT
    );
    mutable += "    upper_command = {" + vector_ones(net.actuator_count)
        + "}) annotation(Placement(transformation(origin = {" + WRENCH_ORIGIN_TEXT + "}, extent = "
        + allocator_extent + ")));\n\n";
    mutable += "equation\n";

    let demanded = [
        "desired_force_x", "desired_force_y", "desired_force_z",
        "desired_torque_x", "desired_torque_y", "desired_torque_z"
    ];
    for name in demanded {
        let value = WRENCH_DEFAULT_DEMAND_TEXT;
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
        result += WRENCH_VECTOR_ONE_TEXT;
    }
    result
}
