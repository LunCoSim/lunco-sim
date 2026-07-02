//! Embedded tutorial curriculum data — files under `assets/tutorials/`.
//!
//! Why this lives HERE: `lunco-assets` owns every asset interaction. Data that
//! must be present at compile time on wasm (no filesystem) is baked in and
//! handed to consumers as raw text — parsing into domain types is the
//! consumer's job (this crate stays I/O-only).

/// The learning-paths curriculum as raw JSON (`assets/tutorials/learning_paths.json`).
/// The lunica Welcome panel parses this into its `LearningPathRegistry`. Edit the
/// JSON and rebuild to change the curriculum — no code edit here.
pub fn learning_paths_json() -> &'static str {
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/tutorials/learning_paths.json"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learning_paths_parse_as_json() {
        let v: serde_json::Value = serde_json::from_str(learning_paths_json())
            .expect("learning_paths.json must be valid JSON");
        assert!(v.get("paths").and_then(|p| p.as_array()).is_some_and(|a| !a.is_empty()));
    }
}
