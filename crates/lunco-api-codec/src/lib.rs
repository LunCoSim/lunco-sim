//! JSON conversion at explicit transport edges only.

use lunco_api_core::ApiValue;

/// Decode a JSON value received at a wire boundary into the typed API ABI.
pub fn value_from_json(value: &serde_json::Value) -> Result<ApiValue, String> {
    use serde_json::Value;

    match value {
        Value::Null => Ok(ApiValue::Unit),
        Value::Bool(value) => Ok(ApiValue::Bool(*value)),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(ApiValue::Int(value))
            } else if let Some(value) = value.as_u64() {
                Ok(lunco_api_core::api_value_from_u64(value))
            } else {
                value
                    .as_f64()
                    .filter(|value| value.is_finite())
                    .map(ApiValue::Float)
                    .ok_or_else(|| "JSON number is not a finite f64".to_string())
            }
        }
        Value::String(value) => Ok(ApiValue::Str(value.clone())),
        Value::Array(values) => values
            .iter()
            .map(value_from_json)
            .collect::<Result<Vec<_>, _>>()
            .map(ApiValue::Array),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| value_from_json(value).map(|value| (key.clone(), value)))
            .collect::<Result<Vec<_>, _>>()
            .map(ApiValue::Map),
    }
}

/// Encode a typed API value at a JSON wire boundary.
pub fn value_to_json(value: &ApiValue) -> Result<serde_json::Value, String> {
    use serde_json::Value;

    match value {
        ApiValue::Unit => Ok(Value::Null),
        ApiValue::Int(value) => Ok(Value::from(*value)),
        ApiValue::UInt(value) => Ok(Value::from(*value)),
        ApiValue::Float(value) => serde_json::Number::from_f64(*value)
            .map(Value::Number)
            .ok_or_else(|| "typed API value contains a non-finite f64".to_string()),
        ApiValue::Bool(value) => Ok(Value::Bool(*value)),
        ApiValue::Str(value) => Ok(Value::String(value.clone())),
        ApiValue::Array(values) => values
            .iter()
            .map(value_to_json)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        ApiValue::Map(values) => values
            .iter()
            .map(|(key, value)| value_to_json(value).map(|value| (key.clone(), value)))
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(Value::Object),
        ApiValue::Bytes(_) => Err("binary API values require an explicit byte transport".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_is_confined_to_the_codec() {
        let json = serde_json::json!({
            "id": 42,
            "pose": [1.0, 2.0, 3.0],
            "enabled": true,
            "missing": null,
        });
        let value = value_from_json(&json).expect("wire JSON should decode");
        assert_eq!(
            value_to_json(&value).expect("typed value should encode"),
            json
        );
    }

    #[test]
    fn values_without_a_json_representation_fail_visibly() {
        let error = value_to_json(&ApiValue::Bytes(vec![1, 2, 3]))
            .expect_err("binary data needs an explicit byte transport");
        assert!(error.contains("binary API values"));
        let large_unsigned = value_from_json(&serde_json::json!(u64::MAX))
            .expect("large unsigned IDs remain typed unsigned values");
        assert_eq!(large_unsigned, ApiValue::UInt(u64::MAX));
        assert_eq!(
            value_to_json(&large_unsigned).expect("unsigned integer has a JSON representation"),
            serde_json::json!(u64::MAX)
        );
    }
}
