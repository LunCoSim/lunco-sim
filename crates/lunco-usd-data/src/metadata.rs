//! Data-only metadata shared by authored and composed USD readers.

/// Parsed `customData` UI hint for a scalar attribute.
///
/// The schema registry and composed-stage reader share this one representation;
/// the UI may interpret it, but the USD core only preserves and decodes the
/// authored metadata.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AttrUiHint {
    /// Inclusive lower bound for a numeric editor.
    pub min: Option<f64>,
    /// Inclusive upper bound for a numeric editor.
    pub max: Option<f64>,
    /// Authored unit label, when supplied.
    pub unit: Option<String>,
    /// USD value type used for write-back, such as `float` or `double`.
    pub type_name: Option<String>,
}

impl AttrUiHint {
    /// Parse a hint from a USD `customData` dictionary.
    pub fn from_dict(dict: &openusd::sdf::Dictionary) -> Option<Self> {
        let hint = Self {
            min: dict_f64(dict, "min"),
            max: dict_f64(dict, "max"),
            unit: dict_string(dict, "unit"),
            type_name: dict_string(dict, "type"),
        };
        (hint != Self::default()).then_some(hint)
    }
}

fn dict_f64(dict: &openusd::sdf::Dictionary, key: &str) -> Option<f64> {
    let value = dict.get(key)?;
    value
        .clone()
        .get::<f64>()
        .or_else(|| value.clone().get::<f32>().map(f64::from))
        .or_else(|| value.clone().get::<i32>().map(f64::from))
}

fn dict_string(dict: &openusd::sdf::Dictionary, key: &str) -> Option<String> {
    dict.get(key)
        .and_then(|value| value.clone().get::<String>())
}
