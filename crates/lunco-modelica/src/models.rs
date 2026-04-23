//! Bundled Modelica example models.
//!
//! Single source of truth: every `*.mo` file under the workspace
//! `assets/models/` directory is embedded at compile time via the
//! `include_dir!` macro. Drop a new `.mo` file in, rebuild, and it
//! appears in the Welcome tab — no edits to this file required.
//!
//! Per-model taglines (one-liners shown next to each entry on the
//! Welcome screen) are read from an optional header comment of the
//! form:
//!
//! ```modelica
//! // tagline: Two-stage RC low-pass filter — 6 MSL blocks
//! model CascadedRCFilter ...
//! ```
//!
//! Without a `// tagline:` line the model is still listed, just with
//! an empty tagline string. The marker must appear before the first
//! non-whitespace Modelica keyword.
//!
//! # Why embed instead of filesystem-scan?
//!
//! * **wasm32** has no filesystem — `include_dir!` is the only way.
//! * **desktop** benefits from the zero-I/O path too (Welcome tab
//!   renders instantly from a static slice; no async race on first
//!   paint).

use include_dir::{include_dir, Dir};

/// Bundled model tree. Baked at compile time — rebuild after editing
/// files under `assets/models/`.
static MODELS_DIR: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/../../assets/models");

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

/// All bundled models, in filesystem iteration order. Call this
/// wherever the old `BUNDLED_MODELS` slice was used; it builds the
/// list fresh every call (cheap — iterates the in-memory `Dir`).
pub fn bundled_models() -> Vec<BundledModel> {
    let mut out: Vec<BundledModel> = MODELS_DIR
        .files()
        .filter(|f| {
            f.path()
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("mo"))
                .unwrap_or(false)
        })
        .filter_map(|f| {
            let filename = f.path().file_name()?.to_str()?;
            let source = f.contents_utf8()?;
            Some(BundledModel {
                filename,
                source,
                tagline: extract_tagline(source),
            })
        })
        .collect();
    // Filesystem iteration order varies by platform; sort by filename
    // so the Welcome tab order is stable across desktop and wasm
    // builds.
    out.sort_by(|a, b| a.filename.cmp(b.filename));
    out
}

/// Get a bundled model's source by filename. Case-sensitive match on
/// the basename.
pub fn get_model(filename: &str) -> Option<&'static str> {
    MODELS_DIR
        .files()
        .find(|f| {
            f.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == filename)
                .unwrap_or(false)
        })
        .and_then(|f| f.contents_utf8())
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
