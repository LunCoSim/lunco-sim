//! Read-only Rhai reporting for SysML v2 analysis.
//!
//! Rhai owns test/report policy in LunCoSim. This adapter only exposes a
//! native semantic snapshot; it never parses source or mutates a document,
//! keeping the language boundary small and deterministic.

use bevy::math::{DQuat, DVec2, DVec3};
use lunco_core::DTransform;
use lunco_sysml_ast::{
    SysmlAnalysis, SysmlAttribute, SysmlDiagnostic, SysmlElement, SysmlEnumValue,
    SysmlModelicaType, SysmlMultiplicity, SysmlPrimitiveType, SysmlQuantityValue, SysmlRecord,
    SysmlSourceRef, SysmlSubject, SysmlType, SysmlTypeCategory, SysmlTypeRef,
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

/// Project all semantic facts as native Rhai values without copying the full
/// source text into the runtime snapshot. The source remains available from
/// the owning document; policies normally need source names, spans and the
/// content revision, not another copy of every file's bytes. Report shape and
/// selection policy belong to authored Rhai tools.
pub fn semantic_snapshot_dynamic(analysis: &SysmlAnalysis) -> Dynamic {
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

/// Register the native semantic values used by the SysML adapter.
///
/// Spatial values deliberately reuse the exact f64 Bevy/glam types already
/// registered by `lunco-scripting-rhai-core::rhai_math`: `DVec3` and `DQuat`.
/// This function only registers the SysML-specific semantic wrappers, so it
/// can safely be called by a report adapter without creating a second vector
/// or quaternion type family.
pub fn register_sysml_types(engine: &mut Engine) {
    engine
        .register_type_with_name::<SysmlTypeCategory>("SysmlTypeCategory")
        .register_type_with_name::<SysmlPrimitiveType>("SysmlPrimitiveType")
        .register_type_with_name::<SysmlModelicaType>("SysmlModelicaType")
        .register_type_with_name::<SysmlTypeRef>("SysmlTypeRef")
        .register_get("qualified_name", |value: &mut SysmlTypeRef| {
            value.qualified_name.clone()
        })
        .register_type_with_name::<SysmlType>("SysmlType")
        .register_get("base", |value: &mut SysmlType| value.base.clone())
        .register_get("category", |value: &mut SysmlType| value.category)
        .register_get("value_category", |value: &mut SysmlType| {
            value.value_category
        })
        .register_get("is_collection", |value: &mut SysmlType| {
            value.category == SysmlTypeCategory::Collection
        })
        .register_get("is_quantity", |value: &mut SysmlType| {
            value.value_category == SysmlTypeCategory::Quantity
        })
        .register_get("resolved_type", |value: &mut SysmlType| {
            value
                .resolved_type
                .clone()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("primitive", |value: &mut SysmlType| {
            value.primitive.map(Dynamic::from).unwrap_or(Dynamic::UNIT)
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
            value
                .quantity_kind
                .clone()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("unit", |value: &mut SysmlType| {
            value.unit.clone().unwrap_or_default()
        })
        .register_get("modelica_type", |value: &mut SysmlType| {
            value.modelica_type()
        })
        .register_type_with_name::<SysmlAttribute>("SysmlAttribute")
        .register_get("owner", |value: &mut SysmlAttribute| value.owner.clone())
        .register_get("name", |value: &mut SysmlAttribute| value.name.clone())
        .register_get("qualified_name", |value: &mut SysmlAttribute| {
            value.qualified_name.clone()
        })
        .register_get("type_name", |value: &mut SysmlAttribute| {
            value.type_name.clone().unwrap_or_default()
        })
        .register_get("declared_type", |value: &mut SysmlAttribute| {
            value
                .declared_type
                .clone()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("value", |value: &mut SysmlAttribute| {
            value
                .value
                .as_ref()
                .map(literal_dynamic)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("typed_value", |value: &mut SysmlAttribute| {
            typed_attribute_value_dynamic(value).unwrap_or(Dynamic::UNIT)
        })
        .register_get("file", |value: &mut SysmlAttribute| value.file.clone())
        .register_get("start", |value: &mut SysmlAttribute| value.start as i64)
        .register_get("end", |value: &mut SysmlAttribute| value.end as i64)
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
            quantity
                .quantity_kind
                .clone()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_type_with_name::<SysmlEnumValue>("EnumValue")
        .register_get("type_ref", |value: &mut SysmlEnumValue| {
            value
                .type_ref
                .clone()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
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

/// Extract a typed literal from the opaque native attribute value at a Rhai
/// adapter boundary. The world bridge need not depend on the SysML AST crate.
pub fn typed_dynamic_attribute_value(attribute: Dynamic) -> Option<Dynamic> {
    let attribute = attribute.try_cast::<SysmlAttribute>()?;
    typed_attribute_value_dynamic(&attribute)
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
    element.category = if element.dimensions.is_empty() && !element.multiplicity.is_collection() {
        element.value_category
    } else {
        SysmlTypeCategory::Collection
    };
    Some(element)
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
        if matches!(
            base,
            "Vec2"
                | "CartesianTwoVectorValue"
                | "CartesianVectorValue"
                | "NumericalVectorValue"
                | "VectorValue"
        ) && values.len() == 2
        {
            let coordinates = values
                .iter()
                .map(numeric_dynamic_f64)
                .collect::<Option<Vec<_>>>()?;
            let vector = DVec2::new(coordinates[0], coordinates[1]);
            return vector.is_finite().then_some(Dynamic::from(vector));
        }
        if matches!(
            base,
            "Vec3"
                | "Position"
                | "Direction"
                | "Dimensions"
                | "CartesianThreeVectorValue"
                | "ThreeVectorValue"
                | "CartesianVectorValue"
                | "NumericalVectorValue"
                | "VectorValue"
        ) && values.len() == 3
        {
            let coordinates = values
                .iter()
                .map(numeric_dynamic_f64)
                .collect::<Option<Vec<_>>>()?;
            return finite_vec3(coordinates[0], coordinates[1], coordinates[2]);
        }
        if matches!(base, "Quat" | "Quaternion") && values.len() == 4 {
            let components = values
                .iter()
                .map(numeric_dynamic_f64)
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
        || declared.is_some_and(|value| {
            value.value_category == lunco_sysml_ast::SysmlTypeCategory::Quantity
        })
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

    if declared.is_some_and(|value| {
        value.value_category == lunco_sysml_ast::SysmlTypeCategory::Enumeration
    }) {
        return Some(Dynamic::from(SysmlEnumValue {
            type_ref: declared?.resolved_type.clone(),
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

fn numeric_dynamic_f64(value: &Dynamic) -> Option<f64> {
    value
        .as_float()
        .ok()
        .or_else(|| value.as_int().ok().map(|integer| integer as f64))
        .filter(|number| number.is_finite())
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
        value.insert("declared_type".into(), Dynamic::from(declared_type.clone()));
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

    #[test]
    fn source_revision_is_lossless_text_in_native_snapshot() {
        let analysis = SysmlAnalysis::build(
            [("revision.sysml", "requirement def R {}")],
            false,
            u64::MAX,
        );
        let snapshot = semantic_snapshot_dynamic(&analysis);
        let snapshot = snapshot.cast::<Map>();
        assert_eq!(
            snapshot["source_revision"]
                .clone()
                .into_immutable_string()
                .unwrap(),
            u64::MAX.to_string()
        );
        assert_eq!(
            snapshot["source_revision_hex"]
                .clone()
                .into_immutable_string()
                .unwrap(),
            "0xffffffffffffffff"
        );
    }
}
