//! Modelica application assets that are not part of the generic engine.

/// The curated Modelica example paths used by the application Welcome panel.
/// This is example navigation data, not a tutorial runtime contract.
pub fn example_paths_json() -> &'static str {
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/modelica/example_paths.json"
    ))
}

#[cfg(test)]
mod tests {
    use super::example_paths_json;

    #[test]
    fn example_paths_parse_as_json() {
        let value: serde_json::Value = serde_json::from_str(example_paths_json())
            .expect("modelica example_paths.json must be valid JSON");
        assert!(value
            .get("paths")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|paths| !paths.is_empty()));
    }
}
