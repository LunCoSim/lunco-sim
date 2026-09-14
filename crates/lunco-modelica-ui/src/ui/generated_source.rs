//! Operator-facing labels for generated Modelica documents.
//!
//! The generated-source data lives in the render-free runtime package. These
//! helpers stay in the UI because they are presentation choices, not runtime
//! identity or lifecycle rules.

use lunco_modelica_runtime::generated_source::GeneratedModelicaUnit;

/// Human-facing label for a generated USD network.
pub fn network_display_name(network_root: &str) -> String {
    network_root
        .trim_matches('/')
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .map(|segment| format!("{segment} network"))
        .unwrap_or_else(|| "Generated network".to_string())
}

/// Decode the stable generated-class spelling for UI labels while preserving
/// the exact Modelica name in source, diagnostics, and tooltips.
pub fn class_display_name(class_name: &str) -> String {
    let class_name = class_name.strip_suffix("_System").unwrap_or(class_name);
    class_name
        .split("_x2f_")
        .map(|segment| segment.replace("__", "_"))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Return the readable leaf of a composed USD path.
pub fn path_leaf(path: &str) -> &str {
    path.trim_matches('/')
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}

/// Short operator-facing name for a synthesized unit.
pub fn unit_display_name(unit: &GeneratedModelicaUnit) -> String {
    unit.members
        .first()
        .map(|member| format!("{} unit", path_leaf(member)))
        .unwrap_or_else(|| class_display_name(&unit.name))
}

/// Short member label with the exact class leaf retained for disambiguation.
pub fn member_display_name(member: &str, class: &str) -> String {
    let class_leaf = class.rsplit('.').next().unwrap_or(class);
    format!("{} · {class_leaf}", path_leaf(member))
}

/// Short telemetry label; the exact generated alias remains available in its
/// tooltip and source contract.
pub fn member_output_display_name(member: &str, output: &str) -> String {
    format!("{}.{}", path_leaf(member), output)
}
