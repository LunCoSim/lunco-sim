//! USD-sim facts for the shared USD linter.
//!
//! `lunco-usd-avian` owns facts for standard `Physics*Joint` projections. This
//! module owns the additional `PhysxPhysicsGearJoint` projection because the
//! vehicle simulation authoring reader owns that relation. It calls the same
//! gear-drive readers as runtime projection; policy remains in
//! `assets/scripting/policy/lint_usd.rhai`.

use lunco_hooks::HookValue as H;
use lunco_usd_bevy_stage::{StageView, UsdRead};

use crate::{is_gear_drive, read_gear_drive_type, read_gear_drive_values, read_gear_ratio};
use lunco_mobility::DifferentialDriveType;

/// Add gear-drive facts to the shared USD physics fact map.
///
/// Invalid values are retained as facts rather than silently omitted. The
/// runtime reader refuses those values and leaves the coupling unapplied; the
/// linter must make that authored error visible before a run.
pub fn append_gear_drive_facts(reader: &StageView<'_>, facts: &mut H) {
    let H::Map(entries) = facts else {
        return;
    };
    entries.push((
        "gear_drives".to_string(),
        H::Array(gear_drive_facts(reader)),
    ));
}

/// Add the canonical PhysX wheel-attachment topology to the authored lint facts.
///
/// The runtime wheel projector and this producer both consume
/// [`crate::wheel_params::collect_wheel_attachment_topology`]. The policy only
/// decides how an invalid result is presented; it never reimplements direct
/// versus relationship-form resolution or selects a first/last target.
pub fn append_wheel_attachment_facts(reader: &StageView<'_>, facts: &mut H) {
    let H::Map(entries) = facts else {
        return;
    };
    let topology = crate::wheel_params::collect_wheel_attachment_topology(reader);
    let mut wheels: Vec<String> = reader
        .prim_paths()
        .into_iter()
        .filter(|path| reader.has_api_schema(path, "PhysxVehicleWheelAPI"))
        .map(|path| path.to_string())
        .collect();
    wheels.sort();

    let wheel_attachments = wheels
        .into_iter()
        .map(|path| {
            let binding = topology.binding_for(&path);
            H::map([
                ("path", H::str(path)),
                ("valid", H::Bool(binding.is_some())),
                (
                    "suspension",
                    binding
                        .map(|binding| H::str(binding.suspension.clone()))
                        .unwrap_or(H::Unit),
                ),
                (
                    "tire",
                    binding
                        .map(|binding| H::str(binding.tire.clone()))
                        .unwrap_or(H::Unit),
                ),
                (
                    "index",
                    binding
                        .map(|binding| H::Int(i64::from(binding.index)))
                        .unwrap_or(H::Unit),
                ),
            ])
        })
        .collect();

    let mut invalid: Vec<String> = topology.invalid_wheels().cloned().collect();
    invalid.sort();
    entries.push(("wheel_attachments".to_string(), H::Array(wheel_attachments)));
    entries.push((
        "invalid_wheel_attachments".to_string(),
        H::Array(invalid.into_iter().map(H::str).collect()),
    ));
}

fn gear_drive_facts(reader: &StageView<'_>) -> Vec<H> {
    let mut facts = Vec::new();
    for path in reader.prim_paths() {
        if !is_gear_drive(reader, &path) {
            continue;
        }

        let ratio = read_gear_ratio(reader, &path);
        let values = read_gear_drive_values(reader, &path);
        let drive_type = read_gear_drive_type(reader, &path);
        let valid = ratio.is_some() && values.is_ok() && drive_type.is_some();
        let values = values.ok();
        let realization = match drive_type {
            Some(DifferentialDriveType::Force) => "implicit_force",
            Some(DifferentialDriveType::Acceleration) => "implicit_acceleration",
            None => "invalid",
        };

        facts.push(H::map([
            ("path", H::str(path.to_string())),
            ("valid", H::Bool(valid)),
            ("realization", H::str(realization)),
            ("ratio", ratio.map(H::Float).unwrap_or(H::Unit)),
            (
                "rest_offset",
                values
                    .map(|values| H::Float(values.rest_offset))
                    .unwrap_or(H::Unit),
            ),
            (
                "target_velocity",
                values
                    .map(|values| H::Float(values.target_velocity))
                    .unwrap_or(H::Unit),
            ),
            (
                "stiffness",
                values
                    .map(|values| H::Float(values.stiffness))
                    .unwrap_or(H::Unit),
            ),
            (
                "damping",
                values
                    .map(|values| H::Float(values.damping))
                    .unwrap_or(H::Unit),
            ),
            (
                "max_force",
                values
                    .map(|values| values.max_force)
                    .filter(|value| value.is_finite())
                    .map(H::Float)
                    .unwrap_or(H::Unit),
            ),
        ]));
    }
    facts
}
