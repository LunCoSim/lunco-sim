//! Shared validation for names persisted as one file under a Twin directory.

use std::path::Path;

/// Validate a command-owned name used as a single file stem.
///
/// The same contract is used by timeline and tool-library persistence. A
/// name is one normal path component; separators, `.`/`..`, and empty names
/// are rejected before any filesystem path is built.
pub fn validate_file_stem(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err("name must be one non-empty file stem".to_string());
    }
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err("name must be one non-empty file stem".to_string());
    }
    lunco_assets_path::validate_portable_file_name(name).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_one_normal_component() {
        assert!(validate_file_stem("approach_v1").is_ok());
        assert!(validate_file_stem("Δelta").is_ok());
        assert!(validate_file_stem("literal#% probe").is_ok());
    }

    #[test]
    fn rejects_path_or_empty_names() {
        for name in [
            "",
            ".",
            "..",
            "../escape",
            "nested/name",
            r"nested\name",
            "NUL",
            "COM1",
            "stream:alias",
            "name?",
            "name.",
        ] {
            assert!(
                validate_file_stem(name).is_err(),
                "{name:?} must be rejected"
            );
        }
    }
}
