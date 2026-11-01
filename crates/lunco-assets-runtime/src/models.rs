//! External Modelica sources under the engine asset library.
//!
//! Modelica source is authored content, not Rust data. The asset library owns
//! its location and storage boundary; this module projects the shared source
//! inventory for native compiler/catalogue consumers. Web consumers
//! load the same files through Bevy's `ModelicaSource` asset loader because a
//! browser has no synchronous directory listing.
//!
//! There is intentionally no compiled-in snapshot and no disk-first or compiled
//! fallback. A present but unreadable package is an error at this boundary so
//! callers can report the unavailable source instead of compiling stale bytes.

use std::path::{Path, PathBuf};

fn models_root() -> Result<PathBuf, String> {
    lunco_assets_core::engine_models_root().map_err(|error| error.to_string())
}

fn source_error(path: &Path, error: impl std::fmt::Display) -> String {
    format!("cannot read Modelica asset {}: {error}", path.display())
}

fn modelica_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mo"))
}

fn asset_relative_path(path: &Path, root: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|error| format!("source has no owning package root: {error}"))?;
    if relative.to_str().is_none() {
        return Err(format!(
            "Modelica source path cannot be represented as a UTF-8 identity: {}",
            relative.display()
        ));
    }
    Ok(lunco_assets_path::slashed(relative))
}

fn model_entry(
    root: &Path,
    relative: &Path,
    expected: Option<lunco_storage::StorageEntryKind>,
) -> Result<Option<PathBuf>, String> {
    #[cfg(not(target_arch = "wasm32"))]
    return lunco_assets_core::resolve_native_asset(&[root.to_path_buf()], relative, expected)
        .map(|entry| entry.map(|(_, path)| path))
        .map_err(|error| source_error(&root.join(relative), error));
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (root, relative, expected);
        Err("native Modelica inventory requires a filesystem source".into())
    }
}

fn read_source(root: &Path, relative: &Path) -> Result<String, String> {
    let path = model_entry(root, relative, Some(lunco_storage::StorageEntryKind::File))?
        .ok_or_else(|| source_error(&root.join(relative), "source disappeared"))?;
    lunco_storage::read_text_file_sync(&path).map_err(|error| source_error(&path, error))
}

/// Every top-level `*.mo` filename, sorted by basename.
pub fn model_filenames() -> Result<Vec<String>, String> {
    let root = models_root()?;
    let entries =
        lunco_storage::read_directory_sync(&root).map_err(|error| source_error(&root, error))?;
    let mut filenames = Vec::new();
    for path in entries.into_iter().filter(|path| modelica_file(path)) {
        let relative = path
            .strip_prefix(&root)
            .map_err(|error| source_error(&path, error))?;
        let resolved = model_entry(&root, relative, None)?
            .ok_or_else(|| source_error(&path, "source disappeared"))?;
        let kind = lunco_storage::entry_kind_file_sync(&resolved)
            .map_err(|error| source_error(&path, error))?;
        if kind == lunco_storage::StorageEntryKind::File {
            filenames.push(
                path.file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        format!(
                            "Modelica asset filename is not valid UTF-8: {}",
                            path.display()
                        )
                    })
                    .map(str::to_owned)?,
            );
        }
    }
    filenames.sort();
    Ok(filenames)
}

/// Every top-level `*.mo` source, sorted by basename.
pub fn model_files() -> Result<Vec<(String, String)>, String> {
    let root = models_root()?;
    let mut files = model_filenames()?
        .into_iter()
        .map(|filename| read_source(&root, Path::new(&filename)).map(|source| (filename, source)))
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
    let root = models_root()?;
    read_source(&root, candidate).map(Some)
}

/// Every `.mo` in a structured package under the engine Modelica library.
pub fn package_files(package: &str) -> Result<Vec<(String, String)>, String> {
    let package_root = lunco_assets_core::models_package_root_path(package)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Modelica package `{package}` has no structured source root"))?;
    let (files, diagnostics) = crate::discovery::read_files_with_extension(&package_root, "mo");
    if !diagnostics.is_empty() {
        return Err(diagnostics.join("\n"));
    }
    let mut files = files
        .into_iter()
        .map(|(path, source)| {
            asset_relative_path(Path::new(&path), &package_root).map(|relative| (relative, source))
        })
        .collect::<Result<Vec<_>, _>>()?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

/// Top-level structured Modelica packages that contain `package.mo`.
pub fn package_roots() -> Result<Vec<String>, String> {
    let root = models_root()?;
    let entries =
        lunco_storage::read_directory_sync(&root).map_err(|error| source_error(&root, error))?;
    let mut roots = Vec::new();
    for path in entries {
        let relative = path
            .strip_prefix(&root)
            .map_err(|error| source_error(&path, error))?;
        let resolved = model_entry(&root, relative, None)?
            .ok_or_else(|| source_error(&path, "source disappeared"))?;
        let kind = lunco_storage::entry_kind_file_sync(&resolved)
            .map_err(|error| source_error(&path, error))?;
        if kind == lunco_storage::StorageEntryKind::Directory {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!(
                        "Modelica package name is not valid UTF-8: {}",
                        path.display()
                    )
                })?;
            if lunco_assets_core::models_package_root_path(&name)
                .map_err(|error| source_error(&path, error))?
                .is_some()
            {
                roots.push(name);
            }
        }
    }
    roots.sort();
    Ok(roots)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn package_inventory_contract() {
        const CHILD: &str = "LUNCO_PACKAGE_INVENTORY_TEST";
        if let Ok(mode) = std::env::var(CHILD) {
            if mode == "valid" {
                let files = package_files("Probe").unwrap();
                assert_eq!(
                    files
                        .iter()
                        .map(|(path, _)| path.as_str())
                        .collect::<Vec<_>>(),
                    ["nested/Demo.mo", "package.mo"]
                );
                assert_eq!(
                    model_source("Loose.mo").unwrap().as_deref(),
                    Some("model Loose end Loose;")
                );
                assert_eq!(package_roots().unwrap(), ["Probe"]);
                assert!(package_files("../../outside").is_err());
            } else if mode == "escaped" {
                let error = package_files("Probe")
                    .expect_err("escaped package member must reject the inventory");
                assert!(error.contains("outside"), "{error}");
            } else if mode == "escaped_root" {
                assert!(
                    package_files("EscapedRoot")
                        .unwrap_err()
                        .contains("outside")
                );
                assert!(package_roots().unwrap_err().contains("outside"));
                assert!(model_filenames().unwrap_err().contains("outside"));
            } else {
                panic!("unknown package inventory test mode");
            }
            return;
        }
        let folder = tempfile::tempdir().unwrap();
        let assets = folder.path().join("assets");
        let package = assets.join("models").join("Probe");
        std::fs::create_dir_all(package.join("nested")).unwrap();
        std::fs::write(
            assets.join("models").join("Loose.mo"),
            "model Loose end Loose;",
        )
        .unwrap();
        std::fs::write(package.join("package.mo"), "package Probe end Probe;").unwrap();
        std::fs::write(
            package.join("nested/Demo.mo"),
            "within Probe; model Demo end Demo;",
        )
        .unwrap();
        let run = |mode: &str| {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "models::tests::package_inventory_contract",
                    "--nocapture",
                ])
                .env(CHILD, mode)
                .env(lunco_assets_core::ASSET_ROOT_ENV, &assets)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run("valid");
        let outside = folder.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("Escaped.mo"), "model Escaped end Escaped;").unwrap();
        crate::discovery::link_test_directory(&package.join("escape"), &outside);
        run("escaped");
        std::fs::write(
            outside.join("package.mo"),
            "package EscapedRoot end EscapedRoot;",
        )
        .unwrap();
        crate::discovery::link_test_directory(&assets.join("models").join("EscapedRoot"), &outside);
        crate::discovery::link_test_directory(&assets.join("models").join("Escaped.mo"), &outside);
        run("escaped_root");
    }
}
