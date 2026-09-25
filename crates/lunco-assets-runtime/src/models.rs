//! External Modelica sources under the engine asset library.
//!
//! Modelica source is authored content, not Rust data. The asset library owns
//! its location and storage boundary; this module only provides the common
//! recursive walk used by native compiler/catalogue consumers. Web consumers
//! load the same files through Bevy's `ModelicaSource` asset loader because a
//! browser has no synchronous directory listing.
//!
//! There is intentionally no compiled-in snapshot and no disk-first or compiled
//! fallback. A present but unreadable package is an error at this boundary so
//! callers can report the unavailable source instead of compiling stale bytes.

use std::path::{Path, PathBuf};

fn models_root() -> PathBuf {
    lunco_assets_core::engine_models_root()
}

fn source_error(path: &Path, error: impl std::fmt::Display) -> String {
    format!("cannot read Modelica asset {}: {error}", path.display())
}

fn modelica_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mo"))
}

fn asset_relative_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .expect("walked Modelica path must be below its root")
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

fn read_source(path: &Path) -> Result<String, String> {
    lunco_storage::read_text_file_sync(path).map_err(|error| source_error(path, error))
}

fn walk_package(
    root: &Path,
    package_root: &Path,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    let entries =
        lunco_storage::read_directory_sync(root).map_err(|error| source_error(root, error))?;
    for path in entries {
        if path.is_dir() {
            walk_package(&path, package_root, out)?;
        } else if modelica_file(&path) {
            out.push((
                asset_relative_path(&path, package_root),
                read_source(&path)?,
            ));
        }
    }
    Ok(())
}

/// Every top-level `*.mo` filename, sorted by basename.
pub fn model_filenames() -> Result<Vec<String>, String> {
    let root = models_root();
    let entries =
        lunco_storage::read_directory_sync(&root).map_err(|error| source_error(&root, error))?;
    let mut filenames = entries
        .into_iter()
        .filter(|path| path.is_file() && modelica_file(path))
        .map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    format!(
                        "Modelica asset filename is not valid UTF-8: {}",
                        path.display()
                    )
                })
                .map(str::to_owned)
        })
        .collect::<Result<Vec<_>, String>>()?;
    filenames.sort();
    Ok(filenames)
}

/// Every top-level `*.mo` source, sorted by basename.
pub fn model_files() -> Result<Vec<(String, String)>, String> {
    let root = models_root();
    let mut files = model_filenames()?
        .into_iter()
        .map(|filename| {
            let path = root.join(&filename);
            read_source(&path).map(|source| (filename, source))
        })
        .collect::<Result<Vec<_>, _>>()?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

/// Read one top-level Modelica source by basename.
pub fn model_source(filename: &str) -> Result<Option<String>, String> {
    let candidate = Path::new(filename);
    if candidate.components().count() != 1 || !modelica_file(candidate) {
        return Ok(None);
    }
    if !model_filenames()?.iter().any(|name| name == filename) {
        return Ok(None);
    }
    let root = models_root();
    let path = root.join(candidate);
    read_source(&path).map(Some)
}

/// Every `.mo` in a structured package under the engine Modelica library.
pub fn package_files(package: &str) -> Result<Vec<(String, String)>, String> {
    let package_root = models_root().join(package);
    let kind = lunco_storage::entry_kind_file_sync(&package_root)
        .map_err(|error| source_error(&package_root, error))?;
    if kind != lunco_storage::StorageEntryKind::Directory {
        return Err(format!(
            "Modelica package path is not a directory: {}",
            package_root.display()
        ));
    }
    let mut files = Vec::new();
    walk_package(&package_root, &package_root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

/// Top-level structured Modelica packages that contain `package.mo`.
pub fn package_roots() -> Result<Vec<String>, String> {
    let root = models_root();
    let entries =
        lunco_storage::read_directory_sync(&root).map_err(|error| source_error(&root, error))?;
    let mut roots = entries
        .into_iter()
        .filter(|path| path.is_dir() && path.join("package.mo").is_file())
        .map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!(
                        "Modelica package name is not valid UTF-8: {}",
                        path.display()
                    )
                })
        })
        .collect::<Result<Vec<_>, String>>()?;
    roots.sort();
    Ok(roots)
}

/// External package roots. Kept as a named function because callers should
/// use the package inventory rather than inventing another asset walk.
pub fn package_roots_live() -> Result<Vec<String>, String> {
    package_roots()
}

/// Read one external structured package through the same source path as the
/// package inventory. No alternate embedded generation is consulted.
pub fn package_files_live(package: &str) -> Result<Vec<(String, String)>, String> {
    package_files(package)
}
