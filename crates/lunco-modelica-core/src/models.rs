//! Bundled Modelica example models — the domain view.
//!
//! The raw embed lives in the asset-owning crate
//! ([`lunco_assets::models`]): every `*.mo` under `assets/models/` is baked in
//! at compile time (wasm has no filesystem) and handed here as `(filename,
//! source)` pairs. Drop a new `.mo` file in, rebuild, and it appears in the
//! Welcome tab — no edits to this file required.
//!
//! This module adds the Modelica-specific interpretation on top: the
//! [`BundledModel`] view and per-model **tagline** parsing (one-liners shown
//! next to each entry on the Welcome screen), read from an optional header
//! comment of the form:
//!
//! ```modelica
//! // tagline: Two-stage RC low-pass filter — 6 MSL blocks
//! model CascadedRCFilter ...
//! ```
//!
//! Without a `// tagline:` line the model is still listed, just with an empty
//! tagline string. The marker must appear before the first non-whitespace
//! Modelica keyword.

/// One bundled example.
#[derive(Clone, Copy)]
pub struct BundledModel {
    /// Filename (e.g. `"RocketEngine.mo"`), relative to
    /// `assets/models/`.
    pub filename: &'static str,
    /// Embedded source text.
    pub source: &'static str,
    /// Short description for the Welcome tab / tooltips. Taken from
    /// the `// tagline: …` header marker when present; empty
    /// otherwise.
    pub tagline: &'static str,
}

/// Extract the `// tagline: …` header from a `.mo` source, if
/// present. Scans the leading comment block only and stops at the
/// first real Modelica line.
fn extract_tagline(source: &str) -> &str {
    for line in source.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        for prefix in ["// tagline:", "//! tagline:", "//tagline:"] {
            if let Some(rest) = t.strip_prefix(prefix) {
                return rest.trim();
            }
        }
        if t.starts_with("//") || t.starts_with("/*") || t.starts_with('*') {
            continue;
        }
        break;
    }
    ""
}

/// All bundled models, sorted by filename (stable across desktop/wasm). Builds
/// the list fresh every call (cheap — enumerates the in-memory embed owned by
/// [`lunco_assets::models`]) and layers on the Modelica tagline parse.
pub fn bundled_models() -> Vec<BundledModel> {
    lunco_assets::models::model_files()
        .into_iter()
        .map(|(filename, source)| BundledModel {
            filename,
            source,
            tagline: extract_tagline(source),
        })
        .collect()
}

/// Get a bundled model's source by filename. Case-sensitive match on the
/// basename. Thin re-export of [`lunco_assets::models::model_source`].
pub fn get_model(filename: &str) -> Option<&'static str> {
    lunco_assets::models::model_source(filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_models_nonempty() {
        let list = bundled_models();
        assert!(
            !list.is_empty(),
            "expected at least one .mo file under assets/models/"
        );
        for m in &list {
            assert!(!m.filename.is_empty(), "bundled model with empty filename");
            assert!(
                !m.source.is_empty(),
                "bundled model '{}' has empty source",
                m.filename
            );
        }
    }

    #[test]
    fn extract_tagline_finds_leading_marker() {
        let src = "// tagline: hello world\nmodel Foo\nend Foo;\n";
        assert_eq!(extract_tagline(src), "hello world");
    }

    #[test]
    fn extract_tagline_handles_blank_and_plain_comments_before() {
        let src = "\n\n// some preamble\n// tagline: after preamble\nmodel F end F;";
        assert_eq!(extract_tagline(src), "after preamble");
    }

    #[test]
    fn extract_tagline_empty_when_missing() {
        assert_eq!(extract_tagline("model Foo\nend Foo;\n"), "");
    }

    #[test]
    fn get_model_known_file_returns_some() {
        // RC_Circuit.mo ships in-tree; if it goes missing we want a
        // loud failure here.
        assert!(get_model("RC_Circuit.mo").is_some());
        assert!(get_model("DoesNotExist.mo").is_none());
    }
}
