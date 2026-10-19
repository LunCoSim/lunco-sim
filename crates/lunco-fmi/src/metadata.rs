//! Lower source-owned FMI scalar declarations into the common port contract.

use fmi_rs::model_description::fmi3::{
    ModelDescription, ModelVariable, TypeDefinition, VariableType,
};
use lunco_engineering_values::{Dimension, Unit, UnitReference};
use lunco_port_core::ScalarPortContract;

/// The source node retains attribute presence, which is necessary to distinguish
/// an explicit false from an inherited relativeQuantity=true declaration.
pub(crate) fn scalar_contract(
    md: &ModelDescription,
    variable: &ModelVariable,
    node: roxmltree::Node<'_, '_>,
) -> Result<ScalarPortContract, String> {
    let VariableType::Float64 {
        declaredType,
        min,
        max,
        ..
    } = &variable.variableType
    else {
        return Err("scalar engineering metadata requires Float64".into());
    };
    let (inherited_min, inherited_max, inherited_relative) = if let Some(name) = declaredType {
        let mut definitions = md
            .typeDefinitions
            .iter()
            .filter(|definition| definition.name() == name);
        let definition = definitions.next();
        if definitions.next().is_some() {
            return Err(format!("duplicate FMI declaredType '{name}'"));
        }
        match definition {
            Some(TypeDefinition::Float64 {
                min,
                max,
                relativeQuantity,
                ..
            }) => (*min, *max, *relativeQuantity),
            _ => {
                return Err(format!(
                    "'{}' requires a Float64 declaredType '{name}'",
                    variable.name
                ));
            }
        }
    } else {
        (None, None, false)
    };
    let relative = match node.attribute("relativeQuantity") {
        Some("true" | "1") => true,
        Some("false" | "0") => false,
        Some(_) => {
            return Err(format!(
                "'{}' has an invalid relativeQuantity",
                variable.name
            ));
        }
        None => inherited_relative,
    };
    let unit = md
        .get_unit(variable)
        .map(|name| {
            let mut definitions = md.unitDefinitions.iter().filter(|unit| unit.name == name);
            let definition = definitions
                .next()
                .ok_or_else(|| format!("undefined FMI unit '{name}'"))?;
            if definitions.next().is_some() {
                return Err(format!("duplicate FMI unit '{name}'"));
            }
            let Some(base) = &definition.baseUnit else {
                return UnitReference::identified(name).map_err(|e| e.to_string());
            };
            let exponents = [base.m, base.kg, base.s, base.A, base.K, base.mol, base.cd];
            let mut dimension = [0_i8; 7];
            for (target, exponent) in dimension.iter_mut().zip(exponents) {
                *target = i8::try_from(exponent).map_err(|_| {
                    format!("FMI unit '{name}' exceeds the dimension exponent range")
                })?;
            }
            Unit::new(name, Dimension(dimension), base.factor, base.offset)
                .map(|unit| {
                    UnitReference::resolved(
                        unit.with_angle_power(base.rad)
                            .with_relative_quantity(relative),
                    )
                })
                .map_err(|e| format!("invalid FMI unit '{name}': {e}"))
        })
        .transpose()?;
    // FMI's omitted limits are unbounded. Explicit infinite sentinels have
    // that same meaning only on the corresponding side of the interval.
    let min = min
        .or(inherited_min)
        .filter(|value| *value != f64::NEG_INFINITY);
    let max = max
        .or(inherited_max)
        .filter(|value| *value != f64::INFINITY);
    let contract = ScalarPortContract {
        unit,
        min,
        max,
        frame: None,
    };
    contract
        .metadata(
            lunco_port_core::ports::PortDirection::InOut,
            "FMI",
            "FMU declaration",
        )
        .validate_contract()
        .map_err(|e| format!("invalid FMI variable '{}': {e}", variable.name))?;
    Ok(contract)
}

/// FMI defines seconds when the independent variable omits a unit. Explicit
/// units must resolve to time; an angular independent variable cannot drive a
/// physics-time participant. This conversion belongs at the native ABI edge.
pub(crate) fn time_unit(
    md: &ModelDescription,
    document: &roxmltree::Document<'_>,
) -> Result<Unit, String> {
    use fmi_rs::model_description::fmi3::Causality;
    let mut independent = md
        .modelVariables
        .iter()
        .filter(|v| matches!(v.causality, Causality::Independent));
    let variable = independent
        .next()
        .ok_or("FMU requires one independent time variable")?;
    if independent.next().is_some() {
        return Err("FMU requires exactly one independent time variable".into());
    }
    let node = document
        .descendants()
        .find(|node| {
            node.is_element()
                && node.attribute("name") == Some(variable.name.as_str())
                && node.has_tag_name("Float64")
        })
        .ok_or("FMU independent time must use scalar Float64")?;
    if !variable.dimensions.is_empty() {
        return Err("FMU independent time must be scalar".into());
    }
    let expected = Unit::coherent_si(Dimension([0, 0, 1, 0, 0, 0, 0]));
    let contract = scalar_contract(md, variable, node)?;
    match contract.unit {
        None => Ok(expected),
        Some(unit) => match unit.definition() {
            Some(definition)
                if definition.dimension() == expected.dimension()
                    && definition.angle_power() == 0 =>
            {
                Ok(definition.clone())
            }
            _ => Err("FMU independent unit must resolve to physical time".into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const XML: &str = r#"<fmiModelDescription fmiVersion="3.0" modelName="Metadata" instantiationToken="test">
      <UnitDefinitions><Unit name="temperature"><BaseUnit K="1" offset="273.15"/></Unit>
      <Unit name="angularSpeed"><BaseUnit s="-1" rad="1" factor="0.017453292519943295"/></Unit></UnitDefinitions>
      <TypeDefinitions><Float64Type name="Temperature" unit="temperature" min="-10" max="100" relativeQuantity="true"/></TypeDefinitions>
      <ModelVariables><Float64 name="delta" valueReference="0" causality="input" declaredType="Temperature"/>
      <Float64 name="absolute" valueReference="1" causality="input" declaredType="Temperature" relativeQuantity="false" min="0"/>
      <Float64 name="omega" valueReference="2" causality="output" unit="angularSpeed"/></ModelVariables>
      <ModelStructure><Output valueReference="2"/></ModelStructure></fmiModelDescription>"#;

    #[test]
    fn scalar_metadata_preserves_inheritance_units_ranges_and_angle() {
        let document = roxmltree::Document::parse(XML).unwrap();
        let md = ModelDescription::from_node(&document.root_element()).unwrap();
        let contracts: Vec<_> = md
            .modelVariables
            .iter()
            .map(|variable| {
                let node = document
                    .descendants()
                    .find(|node| node.attribute("name") == Some(variable.name.as_str()))
                    .unwrap();
                scalar_contract(&md, variable, node).unwrap()
            })
            .collect();
        assert_eq!(
            (contracts[0].min, contracts[0].max),
            (Some(-10.0), Some(100.0))
        );
        let relative = contracts[0].unit.as_ref().unwrap().definition().unwrap();
        assert!(relative.relative_quantity());
        assert_eq!(relative.to_si(5.0).unwrap(), 5.0);
        assert_eq!(contracts[1].min, Some(0.0));
        let absolute = contracts[1].unit.as_ref().unwrap().definition().unwrap();
        assert!(!absolute.relative_quantity());
        assert_eq!(absolute.to_si(0.0).unwrap(), 273.15);
        let angular = contracts[2].unit.as_ref().unwrap().definition().unwrap();
        assert_eq!(angular.angle_power(), 1);
        assert_eq!(angular.dimension(), Dimension([0, 0, -1, 0, 0, 0, 0]));
        assert!((angular.to_si(180.0).unwrap() - std::f64::consts::PI).abs() < 1e-14);
        let invalid = XML.replace("min=\"0\"", "min=\"101\"");
        let document = roxmltree::Document::parse(&invalid).unwrap();
        let md = ModelDescription::from_node(&document.root_element()).unwrap();
        let node = document
            .descendants()
            .find(|node| node.attribute("name") == Some("absolute"))
            .unwrap();
        assert!(scalar_contract(&md, &md.modelVariables[1], node).is_err());
    }
    #[test]
    fn independent_time_uses_the_declared_unit_or_fmi_seconds_default() {
        for (unit, scale) in [("", 1.0), (" unit=\"millisecond\"", 0.001)] {
            let source = XML.replace("<UnitDefinitions>", "<UnitDefinitions><Unit name=\"millisecond\"><BaseUnit s=\"1\" factor=\"0.001\"/></Unit>")
                .replace("<ModelVariables>", &format!("<ModelVariables><Float64 name=\"time\" valueReference=\"4\" causality=\"independent\"{unit}/>"));
            let document = roxmltree::Document::parse(&source).unwrap();
            let md = ModelDescription::from_node(&document.root_element()).unwrap();
            let time = time_unit(&md, &document).unwrap();
            assert_eq!(time.scale_to_si(), scale);
            assert_eq!(time.to_si(time.from_si(0.5).unwrap()).unwrap(), 0.5);
        }
        let source = XML.replace("<ModelVariables>", "<ModelVariables><Float64 name=\"time\" valueReference=\"4\" causality=\"independent\" unit=\"angularSpeed\"/>");
        let document = roxmltree::Document::parse(&source).unwrap();
        let md = ModelDescription::from_node(&document.root_element()).unwrap();
        assert!(time_unit(&md, &document).is_err());
    }
}
