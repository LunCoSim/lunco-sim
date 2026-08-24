//! Embedded Modelica example models — every `*.mo` under `assets/models/`.
//! The source module is intentionally touched when the shipped model contract
//! changes so Cargo rebuilds the include_dir snapshot used by headless runs.
//! The reusable PositionPID3D/PIDAxis/AccelerationLimiter signal boundaries are
//! part of that contract; reusable signal blocks publish explicit output aliases
//! and the guidance exposes the bounded vertical channel.
//! FrameVectorTransform is the shared quaternion frame-conversion boundary used
//! by sensors and guidance.
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
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

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

/// Top-level structured Modelica packages embedded under `assets/models/`.
///
/// A directory is a Modelica package root only when it contains `package.mo`.
/// Keeping this inventory data-driven lets every consumer use normal
/// root-segment lookup (`LunCo.Electrical.Pin` → `LunCo`) without naming a
/// particular library in Rust.
pub fn package_roots() -> Vec<String> {
    let mut roots = MODELS_DIR
        .dirs()
        .filter(|dir| {
            dir.files()
                .any(|file| file.path().file_name() == Some("package.mo"))
        })
        .filter_map(|dir| dir.path().file_name()?.to_str().map(str::to_owned))
        .collect::<Vec<_>>();
    roots.sort();
    roots
}

/// Top-level structured package roots from the live native asset tree, with
/// the embedded tree as the portable fallback on wasm or when the package is
/// not present on disk.
pub fn package_roots_live() -> Vec<String> {
    let mut roots = package_roots();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let models_dir = crate::assets_dir_abs().join("models");
        if let Ok(entries) = std::fs::read_dir(models_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("package.mo").is_file() {
                    if let Some(root) = path.file_name().and_then(|name| name.to_str()) {
                        roots.push(root.to_string());
                    }
                }
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

/// Read one structured package from the live native asset tree, falling back
/// to the embedded snapshot on wasm or when the native tree is unavailable.
/// The asset crate owns this filesystem access; Modelica consumers receive
/// only the source-root file list.
pub fn package_files_live(package: &str) -> Vec<(String, String)> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(root) = crate::models_package_root_path(package) {
        let mut files = Vec::new();
        read_disk_package_files(&root, &mut files);
        files.sort_by(|a, b| a.0.cmp(&b.0));
        if !files.is_empty() {
            return files;
        }
    }
    package_files(package)
}

#[cfg(not(target_arch = "wasm32"))]
fn read_disk_package_files(root: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            read_disk_package_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "mo") {
            if let Ok(source) = std::fs::read_to_string(&path) {
                out.push((path.display().to_string(), source));
            }
        }
    }
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

    #[test]
    fn package_roots_are_structured_and_sorted() {
        let roots = package_roots();
        assert_eq!(roots, {
            let mut sorted = roots.clone();
            sorted.sort();
            sorted
        });
        assert!(roots.iter().any(|root| root == "LunCo"));
    }
}
