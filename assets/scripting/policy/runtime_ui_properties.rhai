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

fn runtime_ui_properties(facts) {
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
    result
}
