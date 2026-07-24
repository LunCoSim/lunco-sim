//! Embedded Modelica example models — every `*.mo` under `assets/models/`.
//!
//! Why this lives HERE: `lunco-assets` owns every asset interaction. The
//! bundled models must be present at compile time on EVERY target — wasm has no
//! filesystem — so they're baked in with `include_dir!` and handed to consumers
//! as raw `(filename, source)` pairs. DROP A `.mo` in `assets/models/`, rebuild,
//! and it's picked up automatically: no code edit here or in the consumer.
//!
//! This module deliberately exposes ONLY raw file access. Domain interpretation
//! — Modelica `// tagline:` header parsing, the `BundledModel` view — stays in
//! `lunco-modelica`, the crate that understands `.mo`.

use include_dir::{include_dir, Dir};

/// Bundled model tree. Baked at compile time — rebuild after editing files
/// under `assets/models/`.
static MODELS_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/models");

/// Every top-level `*.mo` model as `(filename, source)`, sorted by filename so
/// iteration order is stable across desktop and wasm (filesystem order varies).
/// Non-UTF8 files are skipped (nothing legitimately authored here is binary).
pub fn model_files() -> Vec<(&'static str, &'static str)> {
    let mut out: Vec<(&'static str, &'static str)> = MODELS_DIR
        .files()
        .filter(|f| {
            f.path()
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("mo"))
                .unwrap_or(false)
        })
        .filter_map(|f| Some((f.path().file_name()?.to_str()?, f.contents_utf8()?)))
        .collect();
    out.sort_by(|a, b| a.0.cmp(b.0));
    out
}

/// One bundled model's source by basename (case-sensitive), or `None`.
pub fn model_source(filename: &str) -> Option<&'static str> {
    MODELS_DIR
        .files()
        .find(|f| f.path().file_name().and_then(|n| n.to_str()) == Some(filename))
        .and_then(|f| f.contents_utf8())
}

/// Every `.mo` under a package subdirectory of `assets/models/`, as
/// `(path-relative-to-models, source)` — e.g. `("LunCo/Electrical/Battery.mo", …)`.
///
/// RECURSIVE, unlike [`model_files`]: a structured Modelica package is a directory
/// tree (`package.mo` + subpackages + members), so a top-level-only scan misses
/// everything below the root. Used to seat a shipped library into a compile session,
/// which is why the paths are kept qualified — each is a stable, unique document URI.
pub fn package_files(package: &str) -> Vec<(String, String)> {
    fn walk(dir: &Dir, out: &mut Vec<(String, String)>) {
        for f in dir.files() {
            let is_mo = f
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("mo"))
                .unwrap_or(false);
            if is_mo {
                if let Some(src) = f.contents_utf8() {
                    out.push((f.path().to_string_lossy().into_owned(), src.to_string()));
                }
            }
        }
        for sub in dir.dirs() {
            walk(sub, out);
        }
    }

    let mut out = Vec::new();
    if let Some(dir) = MODELS_DIR.get_dir(package) {
        walk(dir, &mut out);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_files_nonempty_and_sorted() {
        let files = model_files();
        assert!(
            !files.is_empty(),
            "expected at least one .mo under assets/models/"
        );
        let mut sorted = files.clone();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        assert_eq!(files, sorted, "model_files not sorted by filename");
        for (name, src) in &files {
            assert!(
                !name.is_empty() && !src.is_empty(),
                "empty model entry {name}"
            );
        }
    }

    #[test]
    fn model_source_known_file() {
        // RC_Circuit.mo ships in-tree; a loud failure here if it goes missing.
        assert!(model_source("RC_Circuit.mo").is_some());
        assert!(model_source("DoesNotExist.mo").is_none());
    }
}
