//! Unit and inspector metadata projected from the native OpenUSD registry.
//!
//! OpenUSD owns schema inheritance, property declarations, variability and
//! fallback values. This module retains only the application metadata cache and
//! linear-unit facts which USD scalar types do not encode.

use lunco_usd_data::metadata::AttrUiHint;
use openusd::{sdf, usd};
use std::collections::HashMap;
use std::sync::OnceLock;

/// How an attribute's scalar value relates to a length in the stage's linear units.
///
/// USD has no role type for scalar lengths — `radius` is a bare `double` — so this
/// is carried per (schema, property) instead, which is the only place the fact is
/// actually known.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LinearUnit {
    /// Not a linear quantity. The DEFAULT and the safe answer: an unannotated
    /// attribute is left alone rather than guessed at.
    #[default]
    None,
    /// A length. `stage_units_per_unit` is how many stage linear units one
    /// authored unit represents — `1.0` for ordinary lengths, `0.1` for
    /// `UsdGeomCamera`'s focal length and apertures, which USD defines in TENTHS
    /// of a world unit.
    Length {
        /// Number of stage linear units represented by one authored unit.
        stage_units_per_unit: f64,
    },
}

/// The linear-unit facts core USD states in prose but encodes in no type.
///
/// `point3f`/`vector3f`/`normal3f` carry their role in the type system and are
/// handled by the role types. A `UsdGeomSphere`'s `radius` is a bare `double`, and
/// on a stage whose `metersPerUnit` is `0.01` it must be authored in centimetres —
/// a fact that lives in the schema's documentation and nowhere a program can read.
/// Keyed by `(schema, property)` for the same reason the registry itself is: core
/// declares `radius` and `height` on four different gprims.
///
/// These application facts are joined to the native compiled definitions;
/// missing schema/property identities fail the metadata validation gate.
const CORE_LINEAR_UNITS: &[(&str, &str, LinearUnit)] = &[
    // Gprim dimensions — plain lengths in stage linear units.
    (
        "Sphere",
        "radius",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Cube",
        "size",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Cylinder",
        "radius",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Cylinder",
        "height",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Cone",
        "radius",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Cone",
        "height",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Capsule",
        "radius",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Capsule",
        "height",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    // USD's axis-agnostic Cylinder_1 and Capsule_1 schemas declare separate
    // radiusTop and radiusBottom lengths.
    (
        "Cylinder_1",
        "radiusTop",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Cylinder_1",
        "radiusBottom",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Cylinder_1",
        "height",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Capsule_1",
        "radiusTop",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Capsule_1",
        "radiusBottom",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Capsule_1",
        "height",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Plane",
        "width",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Plane",
        "length",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    // `UsdGeomCamera` states its focal length and aperture in TENTHS of a world
    // unit, so that the schema's default 50 / 20.955 / 15.2908 read as the
    // photographer's millimetres on a stage authored in centimetres. This is USD's
    // documented convention, not a rounding of ours — see the class documentation
    // in `usdGeom`'s generated schema.
    (
        "Camera",
        "focalLength",
        LinearUnit::Length {
            stage_units_per_unit: 0.1,
        },
    ),
    (
        "Camera",
        "horizontalAperture",
        LinearUnit::Length {
            stage_units_per_unit: 0.1,
        },
    ),
    (
        "Camera",
        "verticalAperture",
        LinearUnit::Length {
            stage_units_per_unit: 0.1,
        },
    ),
    (
        "Camera",
        "horizontalApertureOffset",
        LinearUnit::Length {
            stage_units_per_unit: 0.1,
        },
    ),
    (
        "Camera",
        "verticalApertureOffset",
        LinearUnit::Length {
            stage_units_per_unit: 0.1,
        },
    ),
    // `focusDistance` is a distance in the scene, not through the lens, so it is an
    // ordinary world-unit length — the exception that makes the tenths above easy
    // to get wrong.
    (
        "Camera",
        "focusDistance",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
];

/// Application metadata associated with a native schema property.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyMetadata {
    /// Scalar linear units, which a USD scalar value type does not encode.
    pub linear: LinearUnit,
    /// Inspector bounds and unit label authored in the project schema source.
    pub ui_hint: Option<AttrUiHint>,
}

/// Immutable inspector metadata derived from composed native prim definitions.
#[derive(Debug)]
pub struct SchemaMetadata {
    properties: HashMap<(String, String), PropertyMetadata>,
    prim_types: Vec<String>,
    api_schemas: Vec<String>,
}

impl SchemaMetadata {
    /// Shared metadata for the compiled standard and project schema families.
    pub fn global() -> &'static Self {
        static METADATA: OnceLock<SchemaMetadata> = OnceLock::new();
        METADATA.get_or_init(|| Self::from_registry(&native_registry()))
    }

    fn from_registry(registry: &usd::SchemaRegistry) -> Self {
        let mut metadata = Self {
            properties: HashMap::new(),
            prim_types: Vec::new(),
            api_schemas: Vec::new(),
        };
        let project_names: std::collections::HashSet<_> = crate::registered_schema::SCHEMAS
            .schemas()
            .iter()
            .map(|schema| schema.identifier())
            .collect();
        for info in registry.schema_infos() {
            let schema = info.identifier();
            let definition = match info.kind() {
                usd::SchemaKind::ConcreteTyped => registry.concrete_prim_definition(schema),
                usd::SchemaKind::SingleApplyApi | usd::SchemaKind::MultipleApplyApi => {
                    registry.api_prim_definition(schema)
                }
                _ => None,
            };
            let Some(definition) = definition else {
                continue;
            };
            if project_names.contains(schema.as_str()) {
                if info.kind() == usd::SchemaKind::ConcreteTyped {
                    metadata.prim_types.push(schema.to_string());
                } else {
                    metadata.api_schemas.push(schema.to_string());
                }
            }
            for name in definition.property_names() {
                let property = definition
                    .property(name)
                    .expect("native definition names its property");
                if property.spec_type() != sdf::SpecType::Attribute {
                    continue;
                }
                if property.type_name().is_none() {
                    continue;
                }
                let linear = CORE_LINEAR_UNITS
                    .iter()
                    .find(|(owner, attr, _)| *owner == schema.as_str() && *attr == name.as_str())
                    .map(|(_, _, unit)| *unit)
                    .unwrap_or(LinearUnit::None);
                metadata.properties.insert(
                    (schema.to_string(), name.to_string()),
                    PropertyMetadata {
                        linear,
                        ui_hint: None,
                    },
                );
            }
        }
        for &(schema, name, min, max, unit, type_name) in DECLARED_HINTS {
            let property = metadata
                .properties
                .get_mut(&(schema.to_owned(), name.to_owned()))
                .expect("compiled inspector hint names a registered property");
            property.ui_hint = Some(AttrUiHint {
                min,
                max,
                unit: unit.map(str::to_owned),
                type_name: type_name.map(str::to_owned),
            });
        }
        metadata.prim_types.sort();
        metadata.api_schemas.sort();
        metadata
    }

    /// A declaration shared unambiguously by every schema exposing this name.
    pub fn property(&self, name: &str) -> Option<&PropertyMetadata> {
        let mut candidates = self
            .properties
            .iter()
            .filter(|((_, attr), _)| attr == name)
            .map(|(_, value)| value);
        let first = candidates.next()?;
        candidates
            .all(|candidate| candidate.linear == first.linear && candidate.ui_hint == first.ui_hint)
            .then_some(first)
    }

    /// Metadata for a specific composed schema property.
    pub fn property_in(&self, schema: &str, name: &str) -> Option<&PropertyMetadata> {
        self.properties.get(&(schema.to_owned(), name.to_owned()))
    }

    /// Linear-unit facts for a specific schema property.
    pub fn linear_unit(&self, schema: &str, name: &str) -> LinearUnit {
        self.property_in(schema, name)
            .map(|property| property.linear)
            .unwrap_or(LinearUnit::None)
    }

    /// Registered project concrete types.
    pub fn prim_types(&self) -> &[String] {
        &self.prim_types
    }

    /// Registered project applied API schemas.
    pub fn api_schemas(&self) -> &[String] {
        &self.api_schemas
    }
}

/// The native composed-stage registry with standard and project schemas.
pub fn native_registry() -> std::sync::Arc<openusd::usd::SchemaRegistry> {
    static REGISTRY: OnceLock<std::sync::Arc<openusd::usd::SchemaRegistry>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            let manifest = sdf::Layer::from_bytes(
                "physxManifest.usda",
                include_bytes!("../schema/physxManifest.usda").to_vec(),
            )
            .expect("valid PhysX family manifest");
            let schematics = sdf::Layer::from_bytes(
                "physxSchema.usda",
                include_bytes!("../schema/core/physxSchema.usda").to_vec(),
            )
            .expect("valid PhysX schema declarations");
            openusd_schemas::ALL
                .iter()
                .fold(
                    openusd::usd::SchemaRegistry::builder(),
                    |builder, family| builder.register(family),
                )
                .register(crate::registered_schema::SCHEMAS)
                .family(usd::FamilySource {
                    name: "physxSchema",
                    manifest: &manifest,
                    schematics: &schematics,
                })
                .build()
                .expect("compiled schema declarations must register")
        })
        .clone()
}

include!(concat!(env!("OUT_DIR"), "/schema_hints.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_definitions_supply_units_and_project_inspector_hints() {
        let metadata = SchemaMetadata::global();
        assert!(
            metadata
                .prim_types()
                .iter()
                .any(|name| name == "LunCoEnvironment")
        );
        assert!(
            metadata
                .api_schemas()
                .iter()
                .any(|name| name == "LunCoProgramAPI")
        );
        let radius = metadata.property_in("Sphere", "radius").unwrap();
        assert_eq!(
            radius.linear,
            LinearUnit::Length {
                stage_units_per_unit: 1.0
            }
        );
        let suspension = metadata
            .property_in("LunCoSuspensionAPI", "lunco:suspension:restLength")
            .unwrap();
        let hint = suspension.ui_hint.as_ref().unwrap();
        assert_eq!(hint.min, Some(0.0));
        assert_eq!(hint.max, Some(2.0));
        assert_eq!(hint.unit.as_deref(), Some("m"));
        let wheel = metadata
            .property_in("PhysxVehicleWheelAPI", "physxVehicleWheel:radius")
            .unwrap();
        assert_eq!(wheel.ui_hint.as_ref().unwrap().min, Some(0.05));
        let registry = native_registry();
        let info = registry
            .schema_info(&openusd::tf::Token::new("PhysxVehicleWheelAPI"))
            .unwrap();
        assert_eq!(info.kind(), usd::SchemaKind::SingleApplyApi);
        let suspension = registry
            .api_prim_definition(&openusd::tf::Token::new("PhysxVehicleSuspensionAPI"))
            .unwrap();
        assert!(
            suspension
                .property(&openusd::tf::Token::new(
                    "physxVehicleSuspension:springStrength"
                ))
                .is_some()
        );
        assert!(
            suspension
                .property(&openusd::tf::Token::new(
                    "physxVehicleSuspension:springStiffness"
                ))
                .is_none()
        );
    }
}
