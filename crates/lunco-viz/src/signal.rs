//! Signal model — **the data types now live in [`lunco_signal`]** and are re-exported
//! here, so every existing `lunco_viz::SignalRegistry` / `SignalRef` / `ScalarHistory`
//! caller is unchanged.
//!
//! They moved because `lunco-viz` (with `ui`) links `bevy_egui → bevy_render → wgpu`,
//! while a ring buffer of `f64`s is data, not rendering: the telemetry sampler must push
//! into it from a headless `--no-ui` run, which cannot link a GPU stack. See the
//! `lunco-signal` crate docs and `docs/architecture/render-decoupling.md`.
//!
//! What stayed here is the one genuinely render-bound thing — turning a signal into a
//! *colour* — and that now comes from the **theme**.

pub use lunco_signal::{
    PersistedSignalRef, ScalarHistory, ScalarSample, SignalExposure, SignalMeta, SignalRef,
    SignalRegistry, SignalType, TelemetryFocus, DEFAULT_CAPACITY,
};

/// Convert an authored or generated identifier into the operator spelling used
/// by telemetry-facing surfaces. This deliberately only changes the
/// presentation: signal identities, USD paths, and persisted bindings remain
/// untouched.
pub fn humanize_identifier(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The one presentation policy shared by the telemetry browser, plot
/// toolbars, legends, and exported graph labels.
///
/// `SignalRef::path` remains the immutable identity. This function only
/// projects that identity into an operator label using the producer-supplied
/// ownership path and unit metadata. New channels therefore acquire the same
/// presentation automatically; callers must not hand-format Modelica or USD
/// names.
pub fn operator_channel_label(path: &str, group_path: Option<&str>, unit: Option<&str>) -> String {
    let category = group_path
        .and_then(|group| group.trim_matches('/').rsplit('/').next())
        .map(humanize_identifier)
        .unwrap_or_default();
    compact_channel_label(path, &category, unit)
}

/// Convert a channel variable into its operator wording while retaining the
/// exact authored spelling for identity and diagnostics. Modelica electrical
/// pins use the standard `p.v`/`p.i` and `n.v`/`n.i` fields; their physical
/// meaning is stable even when a generated solver does not carry source
/// descriptions for the flattened connector fields. Unit-qualified authored
/// names omit the redundant unit suffix because the unit column supplies it.
pub fn operator_identifier_label(value: &str, unit: Option<&str>) -> String {
    match value.trim() {
        "p.v" | "n.v" => return "pin voltage".to_string(),
        "p.i" | "n.i" => return "pin current".to_string(),
        _ => {}
    }

    let value = value.trim();
    let source = unit.and_then(|unit| {
        let suffix = match unit.trim() {
            "V" => "_v",
            "A" => "_a",
            "W" => "_w",
            "Ah" => "_ah",
            _ => return None,
        };
        value.strip_suffix(suffix).filter(|base| !base.is_empty())
    });
    humanize_identifier(source.unwrap_or(value))
}

/// Compact a channel name for a row whose owning category is already visible
/// in the surrounding tree.  Prefix removal is performed only at a name
/// boundary, so `Motor L01.speed` is not incorrectly shortened under
/// `Motor L0`.
pub fn compact_channel_label(path: &str, category: &str, unit: Option<&str>) -> String {
    let decoded = path.replace("_x2f_", "/");
    let readable = decoded.trim_matches('/').rsplit('/').next().unwrap_or(path);
    let mut label = operator_identifier_label(readable, unit);
    let category = category.trim();
    if !category.is_empty() {
        let label_lower = label.to_ascii_lowercase();
        let category_lower = category.to_ascii_lowercase();
        // Generated Modelica member aliases retain the escaped owner in their
        // solver identity. Their USD group is authoritative for the owner, so
        // remove everything through the last owner occurrence before applying
        // the ordinary category-relative rule. This keeps the generated
        // address out of the operator label without changing signal identity.
        if readable.starts_with("__member_") {
            if let Some(index) = label_lower.rfind(&category_lower) {
                let remainder =
                    label[index + category.len()..].trim_start_matches(|character: char| {
                        character.is_whitespace() || matches!(character, '.' | '/' | ':' | '_')
                    });
                if !remainder.is_empty() {
                    return remainder.to_string();
                }
            }
        }
        if let Some(remainder) = label_lower.strip_prefix(&category_lower) {
            let boundary = remainder.chars().next();
            if boundary.is_none()
                || boundary.is_some_and(|character| {
                    character.is_whitespace() || matches!(character, '.' | '/' | ':' | '_')
                })
            {
                let cut = category.len();
                label = label[cut..]
                    .trim_start_matches(|character: char| {
                        character.is_whitespace() || matches!(character, '.' | '/' | ':' | '_')
                    })
                    .to_string();
            }
        }
    }
    if label.is_empty() {
        category.to_string()
    } else {
        label
    }
}

/// Select the stable operator label or the exact generated runtime name.
/// Raw-name mode is intentionally explicit and is presentation-only; it does
/// not change signal identity, history, or graph bindings.
pub fn display_channel_label(
    path: &str,
    group_path: Option<&str>,
    unit: Option<&str>,
    show_generated_names: bool,
) -> String {
    if show_generated_names {
        path.to_owned()
    } else {
        operator_channel_label(path, group_path, unit)
    }
}

#[cfg(feature = "ui")]
use bevy_egui::egui;

/// Deterministic colour for a signal path, shared across every plot surface (panel
/// `Graphs`, `VizPanel`, in-canvas `PlotNodeVisual`, the inspector). Same `path` ⇒ same
/// colour everywhere; stable across sessions so a saved layout reopens with consistent
/// legend colours.
///
/// **The palette comes from the theme** ([`lunco_theme::PlotTokens`]), not from a
/// hardcoded table. It used to be a fixed 12-entry Tab10 list baked into this module,
/// which meant plot colours were the only colours in the app that ignored the active
/// theme — the same saturated blues on a light background as on a dark one, and no way
/// to re-theme them. Now they are palette-derived like every other colour.
///
/// Pass the theme in (`lunco_theme::active(ui.ctx())` at any egui call site).
#[cfg(feature = "ui")]
pub fn color_for_signal(theme: &lunco_theme::Theme, path: &str) -> egui::Color32 {
    theme.plot.color_for_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_label_is_category_relative_and_boundary_safe() {
        assert_eq!(
            operator_channel_label(
                "Motor__L0.electrical_power",
                Some("/Traverse/Rover/Motor L0"),
                Some("W"),
            ),
            "electrical power"
        );
        assert_eq!(
            operator_channel_label(
                "Motor L01.speed",
                Some("/Traverse/Rover/Motor L0"),
                Some("rad/s"),
            ),
            "Motor L01.speed"
        );
    }

    #[test]
    fn generated_label_mode_preserves_the_registry_identity() {
        let path = "_x2f_Traverse_x2f_Rover.generated_current_a";
        assert_eq!(display_channel_label(path, None, None, true), path);
        assert_eq!(
            display_channel_label(
                "SolarPanel_generated_current_a",
                Some("/Traverse/Rover/SolarPanel"),
                Some("A"),
                false,
            ),
            "generated current"
        );
        assert_eq!(
            operator_channel_label(
                "__member_Traverse_x2f_Rover_x2f_Motor__L0_electrical_power",
                Some("/Traverse/Rover/Motor_L0"),
                Some("W"),
            ),
            "electrical power"
        );
    }

    #[test]
    fn operator_labels_explain_connectors_and_use_metadata_units() {
        assert_eq!(operator_identifier_label("p.v", None), "pin voltage");
        assert_eq!(operator_identifier_label("p.i", Some("A")), "pin current");
        assert_eq!(
            operator_identifier_label("terminal_voltage_v", Some("V")),
            "terminal voltage"
        );
        assert_eq!(
            operator_identifier_label("terminal_power_w", Some("W")),
            "terminal power"
        );
        assert_eq!(
            compact_channel_label("Motor L0 terminal_current_a", "Motor L0", Some("A")),
            "terminal current"
        );
    }
}
