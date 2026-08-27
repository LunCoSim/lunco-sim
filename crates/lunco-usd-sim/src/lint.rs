//! USD-sim facts for the shared USD linter.
//!
//! `lunco-usd-avian` owns facts for standard `Physics*Joint` projections. This
//! module owns the additional `PhysxPhysicsGearJoint` projection because
//! `lunco-usd-sim` is the only crate that reads and installs that relation. It
//! calls the same gear-drive readers as runtime projection; policy remains in
//! `assets/scripting/policy/lint_usd.rhai`.

use lunco_hooks::HookValue as H;
use lunco_usd_bevy::{StageView, UsdRead};
use openusd::sdf::Path as SdfPath;

use crate::{
    domain_projection::select_synthesizer_name, is_gear_drive, read_gear_drive_type,
    read_gear_drive_values, read_gear_ratio, DifferentialDriveType,
};

/// Add the domain owner selected by the same composed-USD classifier used by
/// runtime domain projection.
///
/// `lunco-usd-avian` produces the generic collection/network topology facts so
/// the linter can remain useful without Modelica source compilation. Ownership
/// is not generic topology, however: a collection containing
/// `LunCoForceActuatorAPI` members is the actuator-wrench domain, while a
/// collection containing `LunCoProgramAPI` members is the generated Modelica
/// domain. Runtime projection already owns that decision in
/// `derive_synthesizer_name`; using it here prevents a missing selector from
/// becoming an invented default Modelica owner.
///
/// An invalid mixed collection is retained as an explicit fact. It is never
/// converted to the default owner, because doing that would turn a runtime
/// projection error into a false clean lint result.
pub fn append_network_synthesizer_facts(reader: &StageView<'_>, facts: &mut H) {
    let H::Map(entries) = facts else {
        return;
    };
    let Some((_, H::Array(scopes))) = entries.iter_mut().find(|(key, _)| key == "network_roots")
    else {
        return;
    };

    for scope in scopes {
        let Some(path) = scope.get("path").and_then(H::as_str) else {
            continue;
        };
        let Ok(root) = SdfPath::new(path) else {
            set_scope_fact(scope, "synthesizer", H::str("invalid"));
            set_scope_fact(
                scope,
                "synthesizer_error",
                H::str("network root path is not a valid absolute USD path"),
            );
            continue;
        };

        match select_synthesizer_name(reader, &root) {
            Ok(name) => {
                set_scope_fact(scope, "synthesizer", H::str(name));
                set_scope_fact(scope, "synthesizer_error", H::str(""));
            }
            Err(error) => {
                set_scope_fact(scope, "synthesizer", H::str("invalid"));
                set_scope_fact(scope, "synthesizer_error", H::str(error));
            }
        }
    }
}

fn set_scope_fact(scope: &mut H, key: &str, value: H) {
    let H::Map(entries) = scope else {
        return;
    };
    if let Some((_, existing)) = entries.iter_mut().find(|(name, _)| name == key) {
        *existing = value;
    } else {
        entries.push((key.to_string(), value));
    }
}

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
                values.map(|values| H::Float(values.0)).unwrap_or(H::Unit),
            ),
            (
                "target_velocity",
                values.map(|values| H::Float(values.1)).unwrap_or(H::Unit),
            ),
            (
                "stiffness",
                values.map(|values| H::Float(values.2)).unwrap_or(H::Unit),
            ),
            (
                "damping",
                values.map(|values| H::Float(values.3)).unwrap_or(H::Unit),
            ),
            (
                "max_force",
                values
                    .map(|values| values.4)
                    .filter(|value| value.is_finite())
                    .map(H::Float)
                    .unwrap_or(H::Unit),
            ),
        ]));
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_usd_bevy::{CanonicalStage, StageRecipe};

    #[test]
    fn network_facts_use_the_runtime_role_classifier() {
        let recipe = StageRecipe::from_source(
            "actuator_network_lint.usda",
            "#usda 1.0\n\
             def Scope \"Rig\" {\n\
                 def Scope \"Actuation\" ( prepend apiSchemas = [\"CollectionAPI:components\"] )\n\
                 {\n\
                     uniform token collection:components:expansionRule = \"explicitOnly\"\n\
                     prepend rel collection:components:includes = [</Rig/Actuator>]\n\
                 }\n\
                 def Xform \"Actuator\" ( prepend apiSchemas = [\"LunCoForceActuatorAPI\"] ) {}\n\
             }\n",
        );
        let stage = CanonicalStage::from_recipe(&recipe).expect("actuator fixture composes");
        let root = SdfPath::new("/Rig/Actuation").expect("root path");
        let mut facts = H::Map(vec![(
            "network_roots".to_string(),
            H::Array(vec![H::map([
                ("path", H::str(root.to_string())),
                ("synthesizer", H::str("")),
            ])]),
        )]);

        append_network_synthesizer_facts(&stage.view(), &mut facts);

        let scope = facts
            .get("network_roots")
            .and_then(|value| match value {
                H::Array(scopes) => scopes.first(),
                _ => None,
            })
            .expect("network root fact");
        assert_eq!(scope.get("synthesizer"), Some(&H::str("actuator-wrench")));
        assert_eq!(scope.get("synthesizer_error"), Some(&H::str("")));
    }

    #[test]
    fn gear_facts_use_the_runtime_reader_and_mark_invalid_values() {
        let recipe = StageRecipe::from_source(
            "gear_lint.usda",
            "#usda 1.0\n\
             ( metersPerUnit = 1 )\n\
             def PhysxPhysicsGearJoint \"Gear\" ( prepend apiSchemas = [\"PhysicsDriveAPI:angular\"] )\n\
             {\n\
                 float physxGearJoint:gearRatio = -1.0\n\
                 float drive:angular:physics:stiffness = -1.0\n\
             }\n",
        );
        let stage = CanonicalStage::from_recipe(&recipe).expect("gear fixture composes");
        let mut facts = H::Map(Vec::new());
        append_gear_drive_facts(&stage.view(), &mut facts);
        let Some(H::Array(gear_facts)) = facts.get("gear_drives") else {
            panic!("gear facts");
        };
        assert_eq!(gear_facts.len(), 1);
        assert_eq!(gear_facts[0].get("valid"), Some(&H::Bool(false)));
        assert_eq!(gear_facts[0].get("stiffness"), Some(&H::Unit));
        assert_eq!(gear_facts[0].get("damping"), Some(&H::Unit));
    }

    #[test]
    fn wheel_facts_use_the_runtime_attachment_topology() {
        let recipe = StageRecipe::from_source(
            "wheel_attachment_lint.usda",
            "#usda 1.0\n\
             def Xform \"Rig\" {\n\
                 def Cylinder \"Wheel\" ( prepend apiSchemas = [\"PhysxVehicleWheelAPI\", \"PhysxVehicleSuspensionAPI\", \"PhysxVehicleTireAPI\"] ) {\n\
                     float physxVehicleWheel:radius = 0.4\n\
                 }\n\
                 def Xform \"AttachmentA\" ( prepend apiSchemas = [\"PhysxVehicleWheelAttachmentAPI\"] ) {\n\
                     rel physxVehicleWheelAttachment:wheel = </Rig/Wheel>\n\
                     rel physxVehicleWheelAttachment:suspension = </Rig/Wheel>\n\
                     rel physxVehicleWheelAttachment:tire = </Rig/Wheel>\n\
                     int physxVehicleWheelAttachment:index = 0\n\
                 }\n\
                 def Xform \"AttachmentB\" ( prepend apiSchemas = [\"PhysxVehicleWheelAttachmentAPI\"] ) {\n\
                     rel physxVehicleWheelAttachment:wheel = </Rig/Wheel>\n\
                     rel physxVehicleWheelAttachment:suspension = </Rig/Wheel>\n\
                     rel physxVehicleWheelAttachment:tire = </Rig/Wheel>\n\
                     int physxVehicleWheelAttachment:index = 1\n\
                 }\n\
             }\n",
        );
        let stage = CanonicalStage::from_recipe(&recipe).expect("wheel fixture composes");
        let mut facts = H::Map(Vec::new());
        append_wheel_attachment_facts(&stage.view(), &mut facts);
        let wheel = facts
            .get("wheel_attachments")
            .and_then(|value| match value {
                H::Array(wheels) => wheels.first(),
                _ => None,
            })
            .expect("wheel attachment fact");
        assert_eq!(wheel.get("valid"), Some(&H::Bool(false)));
        assert_eq!(
            facts.get("invalid_wheel_attachments"),
            Some(&H::Array(vec![H::str("/Rig/Wheel")])),
        );
    }
}
