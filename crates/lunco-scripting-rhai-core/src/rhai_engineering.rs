//! Rhai adapters for the shared, policy-free engineering value contract.
//!
//! Unit vocabulary is supplied by an authored library (UCUM-compatible in the
//! shipped policy). Rust only validates the resolved definition and performs
//! dimension-safe conversion.

use lunco_engineering_values::{Dimension, Quantity, Unit};
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString};

fn runtime_error(message: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(message.into().into(), rhai::Position::NONE).into()
}

fn dimension_from_array(values: Array) -> Result<Dimension, Box<EvalAltResult>> {
    if values.len() != 7 {
        return Err(runtime_error(
            "engineering dimensions require seven SI exponents",
        ));
    }
    let mut dimensions = [0_i8; 7];
    for (index, value) in values.into_iter().enumerate() {
        let exponent = value
            .as_int()
            .map_err(|_| runtime_error("engineering dimensions require integer exponents"))?;
        dimensions[index] = i8::try_from(exponent)
            .map_err(|_| runtime_error("engineering dimension exponent is out of range"))?;
    }
    Ok(Dimension(dimensions))
}

fn engineering_unit(
    symbol: ImmutableString,
    dimensions: Array,
    scale_to_si: f64,
    offset_to_si: f64,
) -> Result<Unit, Box<EvalAltResult>> {
    Unit::new(
        symbol.to_string(),
        dimension_from_array(dimensions)?,
        scale_to_si,
        offset_to_si,
    )
    .map_err(|unit_error| runtime_error(unit_error.to_string()))
}

fn quantity(value: f64, unit: Unit) -> Result<Quantity, Box<EvalAltResult>> {
    Quantity::with_unit(value, unit).map_err(|unit_error| runtime_error(unit_error.to_string()))
}

fn convert(value: f64, from: Unit, to: Unit) -> Result<f64, Box<EvalAltResult>> {
    Quantity::with_unit(value, from)
        .and_then(|value| value.value_in(to))
        .map_err(|unit_error| runtime_error(unit_error.to_string()))
}

fn convert_or_none(value: f64, from: Unit, to: Unit) -> Dynamic {
    convert(value, from, to)
        .map(Dynamic::from_float)
        .unwrap_or(Dynamic::UNIT)
}

fn unit_scale(unit: Unit) -> Dynamic {
    if unit.offset_to_si() == 0.0 {
        Dynamic::from_float(unit.scale_to_si())
    } else {
        Dynamic::UNIT
    }
}

fn quantity_dimension(quantity: &mut Quantity) -> Dynamic {
    Dynamic::from_array(
        quantity
            .dimension()
            .0
            .into_iter()
            .map(|exponent| Dynamic::from_int(i64::from(exponent)))
            .collect(),
    )
}

fn quantity_in(quantity: Quantity, unit: Unit) -> Result<Quantity, Box<EvalAltResult>> {
    quantity
        .in_unit(unit)
        .map_err(|unit_error| runtime_error(unit_error.to_string()))
}

fn quantity_value_in(quantity: Quantity, unit: Unit) -> Result<f64, Box<EvalAltResult>> {
    quantity
        .value_in(unit)
        .map_err(|unit_error| runtime_error(unit_error.to_string()))
}

/// Register the single engineering quantity surface used by source, scene,
/// and simulation adapters.
pub fn register(engine: &mut Engine) {
    engine
        .register_type_with_name::<Unit>("EngineeringUnit")
        .register_get("symbol", |unit: &mut Unit| unit.symbol().to_owned())
        .register_get("dimension", |unit: &mut Unit| {
            Dynamic::from_array(
                unit.dimension()
                    .0
                    .into_iter()
                    .map(|exponent| Dynamic::from_int(i64::from(exponent)))
                    .collect(),
            )
        })
        .register_type_with_name::<Quantity>("Quantity")
        .register_get("value", |quantity: &mut Quantity| quantity.value())
        .register_get("unit", |quantity: &mut Quantity| {
            quantity.unit().symbol().to_owned()
        })
        .register_get("dimension", quantity_dimension)
        .register_fn("engineering_unit", engineering_unit)
        .register_fn("quantity", quantity)
        .register_fn("engineering_convert", convert)
        .register_fn("engineering_convert_or_none", convert_or_none)
        .register_fn("engineering_unit_scale", unit_scale)
        .register_fn("quantity_in", quantity_in)
        .register_fn("quantity_value_in", quantity_value_in);
}

#[cfg(test)]
mod tests {
    use super::register;
    use rhai::Engine;

    #[test]
    fn native_bridge_converts_injected_units_without_a_symbol_table() {
        let mut engine = Engine::new();
        register(&mut engine);
        let value = engine
            .eval::<f64>(
                r#"
                let length = [1, 0, 0, 0, 0, 0, 0];
                let centimetres = engineering_unit("cm", length, 0.01, 0.0);
                let metres = engineering_unit("m", length, 1.0, 0.0);
                engineering_convert(25.0, centimetres, metres)
                "#,
            )
            .unwrap();
        assert_eq!(value, 0.25);
    }
}
