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
    let Some((_, H::Array(scopes))) = entries.iter_mut().find(|(key, _)| key == "network_scopes")
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
                H::str("network scope path is not a valid absolute USD path"),
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
            "network_scopes".to_string(),
            H::Array(vec![H::map([
                ("path", H::str(root.to_string())),
                ("synthesizer", H::str("")),
            ])]),
        )]);

        append_network_synthesizer_facts(&stage.view(), &mut facts);

        let scope = facts
            .get("network_scopes")
            .and_then(|value| match value {
                H::Array(scopes) => scopes.first(),
                _ => None,
            })
            .expect("network scope fact");
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
}
