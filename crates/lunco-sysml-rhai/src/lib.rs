//! Read-only Rhai reporting for SysML v2 analysis.
//!
//! Rhai owns test/report policy in LunCoSim. This adapter only exposes a
//! native semantic snapshot; it never parses source or mutates a document,
//! keeping the language boundary small and deterministic.

use std::sync::Arc;

use bevy::math::{DQuat, DVec3};
use lunco_core::DTransform;
use lunco_sysml_ast::{
    SysmlAnalysis, SysmlAttribute, SysmlDiagnostic, SysmlElement, SysmlEnumValue,
    SysmlMultiplicity, SysmlQuantityValue, SysmlRecord, SysmlSourceRef, SysmlSubject, SysmlType,
    SysmlTypeCategory,
};
use rhai::{Dynamic, Engine, Map};

/// A source-backed SysML record exposed as a native Rhai object.
///
/// Fields are resolved from typed AST attributes, so a script never needs to
/// split CSV strings or parse a source comment to obtain a component value.
#[derive(Clone, Debug)]
pub struct SysmlRecordValue {
    inner: SysmlRecord,
}

fn record_value(record: &mut SysmlRecordValue, name: &str) -> Dynamic {
    record
        .inner
        .fields
        .iter()
        .find(|field| field.name == name)
        .and_then(typed_attribute_value_dynamic)
        .unwrap_or(Dynamic::UNIT)
}

fn record_has_field(record: &mut SysmlRecordValue, name: &str) -> bool {
    record.inner.fields.iter().any(|field| field.name == name)
}

/// Produce a native Rhai map for a complete immutable SysML snapshot.
///
/// This is the preferred in-process path: every field is constructed directly
/// as a Rhai value, so callers do not serialize to JSON and immediately parse
/// the same data back into maps.  Source generations are exposed as canonical
/// decimal text plus hexadecimal text: Rhai's signed integer is not a lossless
/// representation of the full u64 identity, so the bridge never narrows it.
pub fn report_dynamic(analysis: &SysmlAnalysis) -> Dynamic {
    let mut report = Map::new();
    report.insert(
        "source_revision_hex".into(),
        Dynamic::from(format!("0x{:016x}", analysis.source_revision())),
    );
    report.insert(
        "source_revision".into(),
        Dynamic::from(analysis.source_revision().to_string()),
    );
    report.insert(
        "stdlib".into(),
        Dynamic::from_bool(analysis.includes_stdlib()),
    );
    report.insert(
        "files".into(),
        Dynamic::from_array(
            analysis
                .files()
                .iter()
                .map(|file| {
                    let mut value = Map::new();
                    value.insert("name".into(), Dynamic::from(file.name.clone()));
                    value.insert("text".into(), Dynamic::from(file.text.clone()));
                    Dynamic::from_map(value)
                })
                .collect(),
        ),
    );
    report.insert(
        "elements".into(),
        Dynamic::from_array(analysis.elements().iter().map(element_dynamic).collect()),
    );
    report.insert(
        "references".into(),
        Dynamic::from_array(
            analysis
                .references()
                .iter()
                .map(reference_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "relationships".into(),
        Dynamic::from_array(
            analysis
                .relationships()
                .iter()
                .map(relationship_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "constraints".into(),
        Dynamic::from_array(
            analysis
                .constraints()
                .iter()
                .map(constraint_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "attributes".into(),
        Dynamic::from_array(
            analysis
                .attributes()
                .iter()
                .map(|attribute| {
                    attribute_dynamic_at_revision(attribute, analysis.source_revision())
                })
                .collect(),
        ),
    );
    report.insert(
        "records".into(),
        Dynamic::from_array(
            analysis
                .records()
                .iter()
                .cloned()
                .map(|record| Dynamic::from(SysmlRecordValue { inner: record }))
                .collect(),
        ),
    );
    report.insert(
        "requirements".into(),
        Dynamic::from_array(
            analysis
                .requirements()
                .iter()
                .map(|record| requirement_dynamic(record, analysis.source_revision()))
                .collect(),
        ),
    );
    report.insert(
        "verifications".into(),
        Dynamic::from_array(
            analysis
                .verifications()
                .iter()
                .map(verification_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "diagnostics".into(),
        Dynamic::from_array(
            analysis
                .diagnostics()
                .iter()
                .map(diagnostic_dynamic)
                .collect(),
        ),
    );
    Dynamic::from_map(report)
}

/// Produce the compact native Rhai requirement/verification projection.
pub fn requirement_report_dynamic(analysis: &SysmlAnalysis) -> Dynamic {
    let mut report = Map::new();
    report.insert(
        "source_revision_hex".into(),
        Dynamic::from(format!("0x{:016x}", analysis.source_revision())),
    );
    report.insert(
        "source_revision".into(),
        Dynamic::from(analysis.source_revision().to_string()),
    );
    report.insert(
        "stdlib".into(),
        Dynamic::from_bool(analysis.includes_stdlib()),
    );
    report.insert(
        "source_files".into(),
        Dynamic::from_array(
            analysis
                .files()
                .iter()
                .map(|file| Dynamic::from(file.name.clone()))
                .collect(),
        ),
    );
    report.insert(
        "attributes".into(),
        attributes_short_dynamic(analysis, analysis.source_revision()),
    );
    report.insert(
        "records".into(),
        Dynamic::from_array(
            analysis
                .records()
                .iter()
                .cloned()
                .map(|record| Dynamic::from(SysmlRecordValue { inner: record }))
                .collect(),
        ),
    );
    report.insert(
        "attributes_qualified".into(),
        attributes_qualified_dynamic(analysis, analysis.source_revision()),
    );
    report.insert(
        "attribute_collisions".into(),
        attribute_collisions_dynamic(analysis),
    );
    report.insert(
        "relationships".into(),
        Dynamic::from_array(
            analysis
                .relationships()
                .iter()
                .map(relationship_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "constraints".into(),
        Dynamic::from_array(
            analysis
                .constraints()
                .iter()
                .map(constraint_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "requirements".into(),
        Dynamic::from_array(
            analysis
                .requirements()
                .iter()
                .map(|record| requirement_dynamic(record, analysis.source_revision()))
                .collect(),
        ),
    );
    report.insert(
        "verifications".into(),
        Dynamic::from_array(
            analysis
                .verifications()
                .iter()
                .map(verification_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "diagnostics".into(),
        Dynamic::from_array(
            analysis
                .diagnostics()
                .iter()
                .map(diagnostic_dynamic)
                .collect(),
        ),
    );
    Dynamic::from_map(report)
}

/// Register read-only native report functions in a Rhai engine.
///
/// A snapshot is captured by `Arc`, so script execution does not borrow a
/// Bevy world or a live document. Callers can create a fresh registration when
/// a `DocumentChanged` event publishes a newer generation.
pub fn register_sysml_report(engine: &mut rhai::Engine, analysis: Arc<SysmlAnalysis>) {
    register_sysml_types(engine);
    let dynamic_report = Arc::clone(&analysis);
    let compact_report = Arc::clone(&analysis);
    engine.register_fn("sysml_report", move || report_dynamic(&dynamic_report));
    engine.register_fn("sysml_requirement_report", move || {
        requirement_report_dynamic(&compact_report)
    });
}

/// Register the native semantic values used by the SysML adapter.
///
/// Spatial values deliberately reuse the exact f64 Bevy/glam types already
/// registered by `lunco-scripting-rhai-core::rhai_math`: `DVec3` and `DQuat`.
/// This function only registers the SysML-specific semantic wrappers, so it
/// can safely be called by a report adapter without creating a second vector
/// or quaternion type family.
pub fn register_sysml_types(engine: &mut Engine) {
    engine
        .register_type_with_name::<SysmlType>("SysmlType")
        .register_get("base", |value: &mut SysmlType| value.base.clone())
        .register_get("category", |value: &mut SysmlType| {
            format!("{:?}", value.category)
        })
        .register_get("primitive", |value: &mut SysmlType| {
            value
                .primitive
                .map(|primitive| format!("{:?}", primitive))
                .unwrap_or_default()
        })
        .register_get("dimensions", |value: &mut SysmlType| {
            Dynamic::from_array(
                value
                    .dimensions
                    .iter()
                    .map(|dimension| Dynamic::from_int(*dimension as i64))
                    .collect(),
            )
        })
        .register_get("multiplicity", |value: &mut SysmlType| value.multiplicity)
        .register_get("quantity_kind", |value: &mut SysmlType| {
            value.quantity_kind.clone().unwrap_or_default()
        })
        .register_get("unit", |value: &mut SysmlType| {
            value.unit.clone().unwrap_or_default()
        })
        .register_get("modelica_type", |value: &mut SysmlType| {
            format!("{:?}", value.modelica_type())
        })
        .register_type_with_name::<SysmlMultiplicity>("Multiplicity")
        .register_get("lower", |value: &mut SysmlMultiplicity| value.lower as i64)
        .register_get("upper", |value: &mut SysmlMultiplicity| {
            value
                .upper
                .map(|upper| Dynamic::from_int(upper as i64))
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("ordered", |value: &mut SysmlMultiplicity| value.ordered)
        .register_get("unique", |value: &mut SysmlMultiplicity| value.unique)
        .register_type_with_name::<SysmlQuantityValue>("Quantity")
        .register_get("value", |quantity: &mut SysmlQuantityValue| {
            quantity.value.as_f64()
        })
        .register_get("unit", |quantity: &mut SysmlQuantityValue| {
            quantity.unit.clone()
        })
        .register_get("kind", |quantity: &mut SysmlQuantityValue| {
            quantity.quantity_kind.clone().unwrap_or_default()
        })
        .register_type_with_name::<SysmlEnumValue>("EnumValue")
        .register_get("type_name", |value: &mut SysmlEnumValue| {
            value.type_name.clone()
        })
        .register_get("literal", |value: &mut SysmlEnumValue| {
            value.literal.clone()
        })
        .register_type_with_name::<SysmlSourceRef>("SourceRef")
        .register_get("file", |value: &mut SysmlSourceRef| value.file.clone())
        .register_get("start", |value: &mut SysmlSourceRef| value.start as i64)
        .register_get("end", |value: &mut SysmlSourceRef| value.end as i64)
        .register_get("revision", |value: &mut SysmlSourceRef| {
            value.revision.to_string()
        })
        .register_type_with_name::<SysmlRecordValue>("SysmlRecord")
        .register_get("type_name", |value: &mut SysmlRecordValue| {
            value.inner.type_name.clone()
        })
        .register_get("field_names", |value: &mut SysmlRecordValue| {
            Dynamic::from_array(
                value
                    .inner
                    .fields
                    .iter()
                    .map(|field| Dynamic::from(field.name.clone()))
                    .collect(),
            )
        })
        .register_get("source", |value: &mut SysmlRecordValue| {
            value.inner.source.clone()
        })
        .register_fn("value", record_value)
        .register_fn("has_field", record_has_field);
}

/// Lower one resolved SysML literal into the native value used by Rhai and
/// the Editor.  This is intentionally a read-side conversion: unresolved
/// expressions return `None` and must be evaluated by the owning semantic
/// engine instead of being guessed by a string parser.
pub fn typed_attribute_value_dynamic(attribute: &SysmlAttribute) -> Option<Dynamic> {
    let literal = attribute.value.as_ref()?;
    let declared = attribute.declared_type.as_ref();
    typed_literal_dynamic(literal, declared)
}

/// Lower the already-projected `ValidateSysml` attribute record without
/// serializing it again.  The production API query is language-neutral and
/// therefore returns a Rhai map; this adapter consumes the structured fields
/// and restores the same native f64 values used by direct analysis snapshots.
pub fn typed_report_attribute_value(record: &Map) -> Option<Dynamic> {
    let declared = record
        .get("declared_type")
        .and_then(|value| value.clone().try_cast::<Map>());
    let literal = record
        .get("value")
        .and_then(|value| value.clone().try_cast::<Map>())?;
    typed_report_literal_value(&literal, declared.as_ref())
}

fn report_element_type(declared: Option<&Map>) -> Option<Map> {
    let mut element = declared?.clone();
    let mut dimensions = element
        .get("dimensions")?
        .clone()
        .try_cast::<rhai::Array>()?;
    if dimensions.is_empty() {
        return None;
    }
    let _ = dimensions.remove(0);
    let remaining = dimensions.len();
    element.insert("dimensions".into(), Dynamic::from_array(dimensions));
    if remaining == 0
        && element
            .get("category")
            .and_then(|value| value.clone().into_immutable_string().ok())
            .is_some_and(|category| category.as_str() == "Collection")
    {
        let base = element
            .get("base")
            .and_then(|value| value.clone().into_immutable_string().ok())?;
        let category = semantic_category_for_element(&base);
        element.insert("category".into(), Dynamic::from(category));
    }
    Some(element)
}

fn semantic_category_for_element(base: &str) -> &'static str {
    match base.rsplit("::").next().unwrap_or(base) {
        "Boolean" | "Integer" | "Natural" | "Rational" | "Real" | "Complex" | "String" => {
            "Primitive"
        }
        "Vec2" | "Vec3" | "Position" | "Direction" | "Quaternion" | "Quat" | "Transform"
        | "Dimensions" | "Bounds" => "Structured",
        "Length" | "Distance" | "Angle" | "Mass" | "Time" | "Duration" | "Velocity"
        | "Speed" | "Acceleration" | "Force" | "Power" | "Energy" | "Temperature" => {
            "Quantity"
        }
        _ => "Unknown",
    }
}

fn sysml_element_type(declared: Option<&SysmlType>) -> Option<SysmlType> {
    let mut element = declared?.clone();
    if element.dimensions.is_empty() {
        return None;
    }
    element.dimensions.remove(0);
    element.multiplicity = element
        .dimensions
        .first()
        .copied()
        .map(SysmlMultiplicity::fixed)
        .unwrap_or_else(SysmlMultiplicity::one);
    if element.dimensions.is_empty() && element.category == SysmlTypeCategory::Collection {
        element.category = match semantic_category_for_element(&element.base) {
            "Primitive" => SysmlTypeCategory::Primitive,
            "Structured" => SysmlTypeCategory::Structured,
            "Quantity" => SysmlTypeCategory::Quantity,
            _ => SysmlTypeCategory::Unknown,
        };
    }
    Some(element)
}

fn typed_report_literal_value(literal: &Map, declared: Option<&Map>) -> Option<Dynamic> {
    if let Some(elements) = literal
        .get("elements")
        .and_then(|value| value.clone().try_cast::<rhai::Array>())
    {
        let base = declared
            .and_then(|value| value.get("base"))
            .and_then(|value| value.clone().into_immutable_string().ok())
            .unwrap_or_default();
        let element_type = report_element_type(declared);
        let values: Vec<Dynamic> = elements
            .iter()
            .map(|element| {
                let element = element.clone().try_cast::<Map>()?;
                typed_report_literal_value(&element, element_type.as_ref())
            })
            .collect::<Option<_>>()?;
        if matches!(
            base.rsplit("::").next().unwrap_or_default(),
            "Vec3" | "Position" | "Direction" | "Dimensions"
        ) && values.len() == 3
        {
            let coordinates = values
                .iter()
                .map(|value| value.as_float().ok())
                .collect::<Option<Vec<_>>>()?;
            return finite_vec3(coordinates[0], coordinates[1], coordinates[2]);
        }
        if matches!(
            base.rsplit("::").next().unwrap_or_default(),
            "Quat" | "Quaternion"
        ) && values.len() == 4
        {
            let components = values
                .iter()
                .map(|value| value.as_float().ok())
                .collect::<Option<Vec<_>>>()?;
            let quaternion =
                DQuat::from_xyzw(components[0], components[1], components[2], components[3]);
            return normalized_quat(quaternion);
        }
        if base.rsplit("::").next().unwrap_or_default() == "Transform" && values.len() == 3 {
            return native_transform(&values);
        }
        return Some(Dynamic::from_array(values));
    }

    let category = declared
        .and_then(|value| value.get("category"))
        .and_then(|value| value.clone().into_immutable_string().ok())
        .unwrap_or_default();
    if category == "Quantity" {
        let number = literal.get("number_value")?.as_float().ok()?;
        let unit = literal
            .get("unit")
            .and_then(|value| value.clone().into_immutable_string().ok())
            .or_else(|| {
                declared
                    .and_then(|value| value.get("unit"))
                    .and_then(|value| value.clone().into_immutable_string().ok())
            })
            .unwrap_or_default();
        let quantity_kind = declared
            .and_then(|value| value.get("quantity_kind"))
            .and_then(|value| value.clone().into_immutable_string().ok());
        return Some(Dynamic::from(SysmlQuantityValue {
            value: lunco_sysml_ast::SysmlNumber::new(number)?,
            unit: unit.to_string(),
            quantity_kind: quantity_kind.map(|value| value.to_string()),
        }));
    }
    if category == "Enumeration" {
        let type_name = declared
            .and_then(|value| value.get("base"))
            .and_then(|value| value.clone().into_immutable_string().ok())?;
        let literal = literal
            .get("string_value")
            .and_then(|value| value.clone().into_immutable_string().ok())
            .or_else(|| {
                literal
                    .get("literal")
                    .and_then(|value| value.clone().into_immutable_string().ok())
            })?;
        return Some(Dynamic::from(SysmlEnumValue {
            type_name: type_name.to_string(),
            literal: literal.to_string(),
        }));
    }
    if let Some(value) = literal
        .get("integer_value")
        .and_then(|value| value.as_int().ok())
    {
        return Some(Dynamic::from_int(value));
    }
    if let Some(value) = literal
        .get("number_value")
        .and_then(|value| value.as_float().ok())
    {
        return Some(Dynamic::from_float(value));
    }
    if let Some(value) = literal
        .get("boolean_value")
        .and_then(|value| value.as_bool().ok())
    {
        return Some(Dynamic::from_bool(value));
    }
    literal
        .get("string_value")
        .and_then(|value| value.clone().into_immutable_string().ok())
        .map(Dynamic::from)
}

fn typed_literal_dynamic(
    literal: &lunco_sysml_ast::SysmlLiteral,
    declared: Option<&SysmlType>,
) -> Option<Dynamic> {
    if let Some(elements) = &literal.elements {
        let base = declared
            .map(|value| value.base.rsplit("::").next().unwrap_or(&value.base))
            .unwrap_or_default();
        let element_type = sysml_element_type(declared);
        let values: Vec<Dynamic> = elements
            .iter()
            .map(|element| typed_literal_dynamic(element, element_type.as_ref()))
            .collect::<Option<_>>()?;
        if matches!(base, "Vec3" | "Position" | "Direction" | "Dimensions") && values.len() == 3 {
            let coordinates = values
                .iter()
                .map(|value| value.as_float().ok())
                .collect::<Option<Vec<_>>>()?;
            return finite_vec3(coordinates[0], coordinates[1], coordinates[2]);
        }
        if matches!(base, "Quat" | "Quaternion") && values.len() == 4 {
            let components = values
                .iter()
                .map(|value| value.as_float().ok())
                .collect::<Option<Vec<_>>>()?;
            return normalized_quat(DQuat::from_xyzw(
                components[0],
                components[1],
                components[2],
                components[3],
            ));
        }
        if base == "Transform" && values.len() == 3 {
            return native_transform(&values);
        }
        return Some(Dynamic::from_array(values));
    }

    if literal.unit.is_some()
        || declared
            .is_some_and(|value| value.category == lunco_sysml_ast::SysmlTypeCategory::Quantity)
    {
        return Some(Dynamic::from(SysmlQuantityValue {
            value: literal.number_value?,
            unit: literal
                .unit
                .clone()
                .or_else(|| declared.and_then(|value| value.unit.clone()))
                .unwrap_or_default(),
            quantity_kind: declared.and_then(|value| value.quantity_kind.clone()),
        }));
    }

    if declared
        .is_some_and(|value| value.category == lunco_sysml_ast::SysmlTypeCategory::Enumeration)
    {
        return Some(Dynamic::from(SysmlEnumValue {
            type_name: declared?.base.clone(),
            literal: literal
                .string_value
                .clone()
                .unwrap_or_else(|| literal.literal.clone()),
        }));
    }

    if let Some(value) = literal.integer_value {
        return Some(Dynamic::from_int(value));
    }
    if let Some(value) = literal.number_value {
        return Some(Dynamic::from_float(value.as_f64()));
    }
    if let Some(value) = literal.boolean_value {
        return Some(Dynamic::from_bool(value));
    }
    literal.string_value.clone().map(Dynamic::from)
}

fn native_transform(values: &[Dynamic]) -> Option<Dynamic> {
    let translation = dynamic_vec3(&values[0])?;
    let rotation = dynamic_quat(&values[1])?;
    let scale = dynamic_vec3(&values[2])?;
    Some(Dynamic::from(DTransform::new(
        translation,
        rotation,
        scale,
    )?))
}

fn dynamic_vec3(value: &Dynamic) -> Option<DVec3> {
    if let Some(value) = value.clone().try_cast::<DVec3>() {
        return value.is_finite().then_some(value);
    }
    let values = value.clone().try_cast::<rhai::Array>()?;
    if values.len() != 3 {
        return None;
    }
    let components = values
        .iter()
        .map(|value| value.as_float().ok())
        .collect::<Option<Vec<_>>>()?;
    let vector = DVec3::new(components[0], components[1], components[2]);
    vector.is_finite().then_some(vector)
}

fn dynamic_quat(value: &Dynamic) -> Option<DQuat> {
    if let Some(value) = value.clone().try_cast::<DQuat>() {
        return normalized_quat_value(value);
    }
    let values = value.clone().try_cast::<rhai::Array>()?;
    if values.len() != 4 {
        return None;
    }
    let components = values
        .iter()
        .map(|value| value.as_float().ok())
        .collect::<Option<Vec<_>>>()?;
    normalized_quat_value(DQuat::from_xyzw(
        components[0],
        components[1],
        components[2],
        components[3],
    ))
}

fn normalized_quat_value(value: DQuat) -> Option<DQuat> {
    if !value.is_finite() || value.length_squared() <= f64::EPSILON {
        return None;
    }
    Some(value.normalize())
}

fn finite_vec3(x: f64, y: f64, z: f64) -> Option<Dynamic> {
    let value = DVec3::new(x, y, z);
    value.is_finite().then(|| Dynamic::from(value))
}

fn normalized_quat(value: DQuat) -> Option<Dynamic> {
    if !value.is_finite() || value.length_squared() <= f64::EPSILON {
        return None;
    }
    let normalized = value.normalize();
    normalized.is_finite().then(|| Dynamic::from(normalized))
}

fn string_array(values: &[String]) -> Dynamic {
    Dynamic::from_array(values.iter().cloned().map(Dynamic::from).collect())
}

fn element_dynamic(element: &SysmlElement) -> Dynamic {
    let mut value = Map::new();
    value.insert("id".into(), Dynamic::from_int(element.id as i64));
    value.insert("file".into(), Dynamic::from(element.file.clone()));
    value.insert(
        "qualified_name".into(),
        Dynamic::from(element.qualified_name.clone()),
    );
    value.insert("kind".into(), Dynamic::from(element.kind.clone()));
    value.insert("start".into(), Dynamic::from_int(element.start as i64));
    value.insert("end".into(), Dynamic::from_int(element.end as i64));
    Dynamic::from_map(value)
}

fn reference_dynamic(reference: &lunco_sysml_ast::SysmlReference) -> Dynamic {
    let mut value = Map::new();
    value.insert("file".into(), Dynamic::from(reference.file.clone()));
    value.insert("start".into(), Dynamic::from_int(reference.start as i64));
    value.insert("end".into(), Dynamic::from_int(reference.end as i64));
    value.insert("name".into(), Dynamic::from(reference.name.clone()));
    value.insert("from".into(), Dynamic::from(reference.from.clone()));
    value.insert("target".into(), Dynamic::from(reference.target.clone()));
    Dynamic::from_map(value)
}

fn relationship_dynamic(relationship: &lunco_sysml_ast::SysmlRelationship) -> Dynamic {
    let mut value = Map::new();
    value.insert("element".into(), element_dynamic(&relationship.element));
    value.insert(
        "properties".into(),
        Dynamic::from_array(
            relationship
                .properties
                .iter()
                .map(|property| {
                    let mut value = Map::new();
                    value.insert("name".into(), Dynamic::from(property.name.clone()));
                    value.insert("targets".into(), string_array(&property.targets));
                    Dynamic::from_map(value)
                })
                .collect(),
        ),
    );
    Dynamic::from_map(value)
}

fn constraint_dynamic(constraint: &lunco_sysml_ast::SysmlConstraint) -> Dynamic {
    let mut value = Map::new();
    value.insert("element".into(), element_dynamic(&constraint.element));
    if let Some(expression) = &constraint.expression {
        value.insert("expression".into(), Dynamic::from(expression.clone()));
    }
    Dynamic::from_map(value)
}

fn subject_array(values: &[SysmlSubject]) -> Dynamic {
    Dynamic::from_array(
        values
            .iter()
            .map(|subject| {
                let mut value = Map::new();
                value.insert("name".into(), Dynamic::from(subject.name.clone()));
                if let Some(type_name) = &subject.type_name {
                    value.insert("type_name".into(), Dynamic::from(type_name.clone()));
                }
                Dynamic::from_map(value)
            })
            .collect(),
    )
}

fn optional_string(value: &Option<String>) -> Option<Dynamic> {
    value.as_ref().map(|value| Dynamic::from(value.clone()))
}

fn literal_dynamic(literal: &lunco_sysml_ast::SysmlLiteral) -> Dynamic {
    let mut value = Map::new();
    value.insert("literal".into(), Dynamic::from(literal.literal.clone()));
    value.insert("kind".into(), Dynamic::from(literal.kind.clone()));
    if let Some(number) = &literal.number {
        value.insert("number".into(), Dynamic::from(number.clone()));
    }
    if let Some(number) = literal.number_value {
        value.insert("number_value".into(), Dynamic::from_float(number.as_f64()));
    }
    if let Some(integer) = literal.integer_value {
        value.insert("integer_value".into(), Dynamic::from_int(integer));
    }
    if let Some(boolean) = literal.boolean_value {
        value.insert("boolean_value".into(), Dynamic::from_bool(boolean));
    }
    if let Some(string) = &literal.string_value {
        value.insert("string_value".into(), Dynamic::from(string.clone()));
    }
    if let Some(unit) = &literal.unit {
        value.insert("unit".into(), Dynamic::from(unit.clone()));
    }
    value.insert(
        "literal_kind".into(),
        Dynamic::from(format!("{:?}", literal.literal_kind)),
    );
    if let Some(elements) = &literal.elements {
        value.insert(
            "elements".into(),
            Dynamic::from_array(elements.iter().map(literal_dynamic).collect()),
        );
    }
    Dynamic::from_map(value)
}

fn attribute_dynamic_at_revision(attribute: &SysmlAttribute, revision: u64) -> Dynamic {
    let mut value = Map::new();
    value.insert("owner".into(), Dynamic::from(attribute.owner.clone()));
    value.insert("name".into(), Dynamic::from(attribute.name.clone()));
    value.insert(
        "qualified_name".into(),
        Dynamic::from(attribute.qualified_name.clone()),
    );
    if let Some(type_name) = optional_string(&attribute.type_name) {
        value.insert("type_name".into(), type_name);
    }
    if let Some(declared_type) = &attribute.declared_type {
        value.insert("typed_type".into(), Dynamic::from(declared_type.clone()));
        let mut type_value = Map::new();
        type_value.insert("base".into(), Dynamic::from(declared_type.base.clone()));
        type_value.insert(
            "category".into(),
            Dynamic::from(format!("{:?}", declared_type.category)),
        );
        type_value.insert(
            "modelica_type".into(),
            Dynamic::from(format!("{:?}", declared_type.modelica_type())),
        );
        type_value.insert(
            "dimensions".into(),
            Dynamic::from_array(
                declared_type
                    .dimensions
                    .iter()
                    .map(|dimension| Dynamic::from_int(*dimension as i64))
                    .collect(),
            ),
        );
        type_value.insert(
            "multiplicity".into(),
            Dynamic::from(declared_type.multiplicity),
        );
        if let Some(quantity_kind) = &declared_type.quantity_kind {
            type_value.insert("quantity_kind".into(), Dynamic::from(quantity_kind.clone()));
        }
        value.insert("declared_type".into(), Dynamic::from_map(type_value));
    }
    if let Some(literal) = &attribute.value {
        value.insert("value".into(), literal_dynamic(literal));
    }
    if let Some(typed_value) = typed_attribute_value_dynamic(attribute) {
        value.insert("typed_value".into(), typed_value);
    }
    value.insert("file".into(), Dynamic::from(attribute.file.clone()));
    value.insert("start".into(), Dynamic::from_int(attribute.start as i64));
    value.insert("end".into(), Dynamic::from_int(attribute.end as i64));
    value.insert(
        "source".into(),
        Dynamic::from(attribute.source_ref(revision)),
    );
    Dynamic::from_map(value)
}

fn attributes_qualified_dynamic(analysis: &SysmlAnalysis, revision: u64) -> Dynamic {
    let mut output = Map::new();
    for attribute in analysis.attributes() {
        output.insert(
            attribute.qualified_name.clone().into(),
            attribute_dynamic_at_revision(attribute, revision),
        );
    }
    Dynamic::from_map(output)
}

fn attributes_short_dynamic(analysis: &SysmlAnalysis, revision: u64) -> Dynamic {
    let mut output = Map::new();
    for attribute in analysis.attributes() {
        output.insert(
            attribute.name.clone().into(),
            attribute_dynamic_at_revision(attribute, revision),
        );
    }
    Dynamic::from_map(output)
}

fn attribute_collisions_dynamic(analysis: &SysmlAnalysis) -> Dynamic {
    let mut names = std::collections::BTreeMap::<String, Vec<String>>::new();
    for attribute in analysis.attributes() {
        names
            .entry(attribute.name.clone())
            .or_default()
            .push(attribute.qualified_name.clone());
    }
    Dynamic::from_array(
        names
            .into_iter()
            .filter_map(|(name, qualified_names)| {
                if qualified_names.len() < 2 {
                    return None;
                }
                let mut value = Map::new();
                value.insert("name".into(), Dynamic::from(name));
                value.insert("qualified_names".into(), string_array(&qualified_names));
                Some(Dynamic::from_map(value))
            })
            .collect(),
    )
}

fn requirement_dynamic(record: &lunco_sysml_ast::SysmlRequirementRecord, revision: u64) -> Dynamic {
    let mut value = Map::new();
    value.insert("element".into(), element_dynamic(&record.element));
    value.insert("documentation".into(), string_array(&record.documentation));
    value.insert("subjects".into(), subject_array(&record.subjects));
    value.insert(
        "attributes".into(),
        Dynamic::from_array(
            record
                .attributes
                .iter()
                .map(|attribute| attribute_dynamic_at_revision(attribute, revision))
                .collect(),
        ),
    );
    value.insert("verifies".into(), string_array(&record.verifies));
    value.insert("satisfies".into(), string_array(&record.satisfies));
    value.insert("realizations".into(), string_array(&record.realizations));
    Dynamic::from_map(value)
}

fn verification_dynamic(record: &lunco_sysml_ast::SysmlVerificationRecord) -> Dynamic {
    let mut value = Map::new();
    value.insert("element".into(), element_dynamic(&record.element));
    value.insert("documentation".into(), string_array(&record.documentation));
    value.insert("subjects".into(), subject_array(&record.subjects));
    value.insert("verifies".into(), string_array(&record.verifies));
    value.insert("realizations".into(), string_array(&record.realizations));
    Dynamic::from_map(value)
}

fn diagnostic_dynamic(diagnostic: &SysmlDiagnostic) -> Dynamic {
    let mut value = Map::new();
    value.insert("file".into(), Dynamic::from(diagnostic.file.clone()));
    value.insert(
        "kind".into(),
        Dynamic::from(format!("{:?}", diagnostic.kind)),
    );
    value.insert("start".into(), Dynamic::from_int(diagnostic.start as i64));
    value.insert("end".into(), Dynamic::from_int(diagnostic.end as i64));
    value.insert("message".into(), Dynamic::from(diagnostic.message.clone()));
    Dynamic::from_map(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;

    #[test]
    fn report_is_consumable_by_rhai() {
        let analysis = Arc::new(SysmlAnalysis::from_files([(
            "example.sysml",
            "requirement def MassRequirement {}",
        )]));
        let mut engine = rhai::Engine::new();
        register_sysml_report(&mut engine, analysis);
        let native: Dynamic = engine.eval("sysml_requirement_report()").unwrap();
        let native = native.cast::<Map>();
        assert!(native.contains_key("requirements"));
        let report: Map = engine.eval("sysml_report()").unwrap();
        let files = report
            .get("files")
            .cloned()
            .expect("source files report")
            .cast::<rhai::Array>();
        let file = files[0].clone().cast::<Map>();
        assert_eq!(
            file.get("name")
                .cloned()
                .expect("file name")
                .into_immutable_string()
                .unwrap(),
            "example.sysml"
        );
    }

    #[test]
    fn native_report_exposes_numeric_literal_without_string_parsing() {
        let analysis = Arc::new(SysmlAnalysis::from_files_without_stdlib([(
            "numeric.sysml",
            "part def A { attribute mass : Real = 2.5; }",
        )]));
        let mut engine = rhai::Engine::new();
        register_sysml_report(&mut engine, analysis);
        let value: f64 = engine
            .eval("sysml_report().attributes[0].value.number_value")
            .expect("native numeric projection");
        assert_eq!(value, 2.5);
    }

    #[test]
    fn rhai_report_exposes_semantic_position_as_shared_f64_vec3() {
        let analysis = Arc::new(SysmlAnalysis::from_files_without_stdlib([(
            "position.sysml",
            "part def Lander { attribute station : Position = (1.25, -2.0, 3.5); }",
        )]));
        let mut engine = rhai::Engine::new();
        engine
            .register_type_with_name::<DVec3>("Vec3")
            .register_get("x", |value: &mut DVec3| value.x)
            .register_get("y", |value: &mut DVec3| value.y)
            .register_get("z", |value: &mut DVec3| value.z);
        register_sysml_report(&mut engine, analysis);
        let coordinates: rhai::Array = engine
            .eval("let p = sysml_report().attributes[0].typed_value; [p.x, p.y, p.z]")
            .expect("Rhai typed position projection");
        let values: Vec<f64> = coordinates
            .into_iter()
            .map(|value| value.as_float().expect("f64 coordinate"))
            .collect();
        assert_eq!(values, [1.25, -2.0, 3.5]);
    }

    #[test]
    fn rhai_report_exposes_quantity_and_enum_as_typed_values() {
        let analysis = Arc::new(SysmlAnalysis::from_files_without_stdlib([(
            "typed-values.sysml",
            r#"enum def Pose { Landed; }
                part def Lander {
                    attribute mass : Mass = 1200 [kg];
                    attribute pose : Pose = "Landed";
                }"#,
        )]));
        let mut engine = rhai::Engine::new();
        register_sysml_report(&mut engine, analysis);
        let values: rhai::Array = engine
            .eval(
                "let a = sysml_report().attributes; \
                 [a[0].typed_value.value, a[0].typed_value.unit, \
                  a[1].typed_value.type_name, a[1].typed_value.literal]",
            )
            .expect("typed quantity and enumeration projection");
        assert_eq!(values[0].as_float().unwrap(), 1200.0);
        assert_eq!(values[1].clone().into_immutable_string().unwrap(), "kg");
        assert_eq!(values[2].clone().into_immutable_string().unwrap(), "Pose");
        assert_eq!(values[3].clone().into_immutable_string().unwrap(), "Landed");
    }

    #[test]
    fn rhai_report_exposes_recursive_vector_literals_without_csv_parsing() {
        let analysis = Arc::new(SysmlAnalysis::from_files_without_stdlib([(
            "vectors.sysml",
            "part def Lander { attribute stations : Real[2][2] = ((1.0, 2.0), (3.0, 4.0)); }",
        )]));
        let mut engine = rhai::Engine::new();
        register_sysml_report(&mut engine, analysis);
        let values: rhai::Array = engine
            .eval(
                "let a = sysml_report().attributes[0]; \
                 [a.typed_type.base, a.typed_type.dimensions[0], \
                  a.value.elements[0].elements[1].number_value, \
                  a.value.elements[1].elements[0].number_value]",
            )
            .expect("recursive typed vector projection");
        assert_eq!(values[0].clone().into_immutable_string().unwrap(), "Real");
        assert_eq!(values[1].as_int().unwrap(), 2);
        assert_eq!(values[2].as_float().unwrap(), 2.0);
        assert_eq!(values[3].as_float().unwrap(), 3.0);
    }

    #[test]
    fn source_revision_is_lossless_text_in_native_reports() {
        let analysis = SysmlAnalysis::build(
            [("revision.sysml", "requirement def R {}")],
            false,
            u64::MAX,
        );
        let report = report_dynamic(&analysis);
        let report = report.cast::<Map>();
        assert_eq!(
            report["source_revision"]
                .clone()
                .into_immutable_string()
                .unwrap(),
            u64::MAX.to_string()
        );
        assert_eq!(
            report["source_revision_hex"]
                .clone()
                .into_immutable_string()
                .unwrap(),
            "0xffffffffffffffff"
        );
    }

    #[test]
    fn native_record_exposes_typed_transform_and_source() {
        let analysis = Arc::new(SysmlAnalysis::from_files_without_stdlib([(
            "griffin.sysml",
            "part def Griffin { attribute pose : Transform = ((1.0, 2.0, 3.0), (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0)); attribute railLength : Real = 2.6; }",
        )]));
        let mut engine = rhai::Engine::new();
        lunco_scripting_rhai_core::rhai_math::register(&mut engine);
        register_sysml_report(&mut engine, analysis);
        let values: rhai::Array = engine
            .eval(
                "let record = sysml_report().records[0]; \
                 [record.type_name, record.field_names[0], \
                  record.value(\"pose\").translation.x, \
                  record.source.file, record.source.revision]",
            )
            .expect("typed SysML record projection");
        assert_eq!(
            values[0].clone().into_immutable_string().unwrap(),
            "Griffin"
        );
        assert_eq!(values[1].clone().into_immutable_string().unwrap(), "pose");
        assert_eq!(values[2].as_float().unwrap(), 1.0);
        assert_eq!(
            values[3].clone().into_immutable_string().unwrap(),
            "griffin.sysml"
        );
        assert_eq!(values[4].clone().into_immutable_string().unwrap(), "0");
    }
}
