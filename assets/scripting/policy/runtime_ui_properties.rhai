// Generic runtime-surface presentation policy.
//
// This is intentionally a small, model-neutral view model. Twin/model policy
// can replace it and use the same typed participant, output, pose, and
// telemetry facts to add a domain-specific surface without Rust changes.

fn status_label(status) {
    if status == "running" { return "RUNNING"; }
    if status == "compiling" { return "COMPILING"; }
    if status == "paused" { return "PAUSED"; }
    if status == "error" { return "ERROR"; }
    if status == "idle" { return "READY"; }
    "UNAVAILABLE"
}

fn status_color(status) {
    if status == "running" { return "var(--ok-color)"; }
    if status == "error" { return "var(--danger-color)"; }
    if status == "paused" { return "var(--caution-color)"; }
    "var(--accent-color)"
}

fn vector_text(value) {
    if value == () || value.len != 3 { return "—"; }
    value[0].to_string() + ", " + value[1].to_string() + ", " + value[2].to_string()
}

fn telemetry_text(values) {
    if values.len == 0 { return "TELEMETRY UNAVAILABLE"; }
    let result = "";
    for value in values {
        if result != "" { result += " | "; }
        let unit = if value.unit == () || value.unit == "" { "" } else { " " + value.unit };
        result += value.label + " " + value.value.to_string() + unit;
    }
    result
}

fn participant_text(values) {
    if values.len == 0 { return "no simulation participants"; }
    let result = "";
    for value in values {
        if result != "" { result += " | "; }
        result += value.model + ":" + status_label(value.status);
    }
    result
}

fn input_value(facts, name) {
    let inputs = facts.inputs;
    if inputs == () { return 0.0; }
    let value = inputs[name];
    if value == () { return 0.0; }
    value
}

fn camera_ui_properties(facts) {
    let result = #{};
    result.active_label = facts.active_label;
    // The HTML surface and the generic dropdown renderer consume the same
    // authored dimensions. A Twin can replace these values in its policy.
    result.dropdown_width = "280px";
    result.dropdown_max_height = "240px";
    let items = [];
    for camera in facts.cameras {
        items.push(#{
            key: camera.name,
            label: camera.label,
            action: "camera.select.path." + camera.name,
        });
    }
    if facts.avatar_available {
        items.push(#{
            key: "__avatar__",
            label: "Observe avatar",
            action: "camera.observe.avatar",
        });
    }
    if facts.director_available {
        items.push(#{
            key: "__director__",
            label: "Resume authored director",
            action: "camera.resume.director",
        });
    }
    result.camera_items = items;
    result
}

fn runtime_ui_properties(facts) {
    if facts.surface_id == "camera-status" {
        return camera_ui_properties(facts);
    }
    let result = #{};
    result.vehicle = facts.label;
    result.status = if facts.available { status_label(facts.status) } else { "WAITING" };
    result.status_color = if facts.available { status_color(facts.status) } else { "var(--muted-color)" };
    result.owner = facts.control_owner;
    result.speed = vector_text(facts.velocity);
    result.position = vector_text(facts.position);
    result.angular_velocity = vector_text(facts.angular_velocity);
    result.telemetry_summary = telemetry_text(facts.telemetry);
    result.simulation_summary = participant_text(facts.participants);
    result.scope = if facts.scope == () || facts.scope == "" { "NO PROGRAM SCOPE" } else { facts.scope };
    let programs = if facts.programs == () { [] } else { facts.programs };
    result.program_count = programs.len().to_string() + " PROGRAMS";
    result.program_empty_display = programs.len() == 0;
    let program_active = programs.len() > 0
        && input_value(facts, "program_active") > 0.5;
    result.program_label = if programs.len() == 0 {
        "PROGRAM UNAVAILABLE"
    } else if program_active {
        "AUTOPILOT ON"
    } else {
        "AUTOPILOT OFF"
    };
    result.program_color = if programs.len() == 0 {
        "var(--muted-color)"
    } else if program_active {
        "var(--ok-color)"
    } else {
        "var(--accent-color)"
    };
    let items = [];
    for index in 0..programs.len() {
        let program = programs[index];
        let source = if program.source == () || program.source == "" {
            program.implementation_source
        } else {
            program.source
        };
        items.push(#{
            key: program.path,
            path: program.path,
            source: source,
            status: program.status,
            // Paths are the canonical program identity. Indexes are only a
            // presentation order and must not select a different program
            // after a live edit reorders the collection.
            select_action: "program.select.path." + program.path,
            edit_action: "program.edit.path." + program.path,
        });
    }
    result.programs = items;
    result.current_program = if items.len() > 0 { items[0].path } else { "" };
    result.current_source = if items.len() > 0 { items[0].source } else { "" };
    result
}
