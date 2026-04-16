//! # lunco-twin
//!
//! Twin is LunCoSim's top-level persistent artifact: a **folder** on disk
//! plus a **`twin.toml` manifest**. This crate defines the container shape
//! — what a Twin is, how to open one, how to discover the files inside,
//! and how to classify them (Document vs. file reference).
//!
//! This crate is **UI-free, ECS-free, headless-capable**. It depends only
//! on [`lunco-doc`] and standard serialization libs. Domain crates (Modelica,
//! USD, SysML) parse their own files; `lunco-twin` just tracks where those
//! files live and what kind they are.
//!
//! ## The three modes
//!
//! LunCoSim mirrors VS Code's Open File / Open Folder / Open Workspace
//! triad. See [`TwinMode`] for the enum.
//!
//! | Mode | What the user opened | What this crate returns |
//! |------|----------------------|-------------------------|
//! | [`Orphan`](TwinMode::Orphan) | a single file | path only |
//! | [`Folder`](TwinMode::Folder) | a folder without `twin.toml` | discovered file index |
//! | [`Twin`](TwinMode::Twin) | a folder *with* `twin.toml` | manifest + file index |
//!
//! ## Example
//!
//! ```no_run
//! use lunco_twin::{TwinMode, Twin};
//! use std::path::Path;
//!
//! match TwinMode::open(Path::new("./my_base")).unwrap() {
//!     TwinMode::Twin(twin) => {
//!         let manifest = twin.manifest.as_ref().expect("Twin mode implies a manifest");
//!         println!("Opened Twin: {}", manifest.name);
//!         for entry in twin.files() {
//!             println!("  {:?}  {}", entry.kind, entry.relative_path.display());
//!         }
//!     }
//!     TwinMode::Folder(twin) => {
//!         println!("Opened folder (no twin.toml); {} files discovered", twin.files().len());
//!     }
//!     TwinMode::Orphan(path) => {
//!         println!("Opened single file: {}", path.display());
//!     }
//! }
//! ```
//!
//! ## What this crate does NOT do (yet)
//!
//! - **Load / parse Documents.** Each domain crate owns its parser. Twin
//!   tracks file paths and classifications only.
//! - **Cross-Document transactions.** Planned as `TwinTransaction` when
//!   we have a multi-Document op site. Don't exist yet.
//! - **Caches.** DAE / AST caches will live on a `CacheRegistry` when we
//!   have a cache consumer. Not today.
//! - **File watching.** Manual reload via `Twin::reload`. Inotify/FSEvents
//!   can be added later behind a feature flag.
//! - **Save/export of Documents.** Manifest save only (`save_manifest`);
//!   Document serialization is each domain's responsibility.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::path::{Path, PathBuf};

mod error;
mod file_kind;
mod manifest;

pub use error::TwinError;
pub use file_kind::{DocumentKind, FileEntry, FileKind};
pub use manifest::{TwinManifest, MANIFEST_FILENAME};

// Re-export lunco-doc so downstream crates don't need to depend on it
// separately just to use the types Twin hands back in the future.
pub use lunco_doc;

// ─────────────────────────────────────────────────────────────────────────────
// TwinMode — what did the user open?
// ─────────────────────────────────────────────────────────────────────────────

/// The three ways a user can open content in LunCoSim.
///
/// Produced by [`TwinMode::open`] after inspecting a path on disk.
#[derive(Debug)]
pub enum TwinMode {
    /// A single file was opened, outside any folder context. No sibling
    /// files are known. Example: double-clicking `balloon.mo` in a file
    /// manager.
    Orphan(PathBuf),

    /// A folder was opened but it contains no `twin.toml`. The folder's
    /// files are indexed so the user can browse them, but there is no
    /// Twin manifest, no registered libraries, no cross-reference repair
    /// flow. This is the VS Code "Open Folder" analog.
    Folder(Twin),

    /// A full Twin — folder containing a `twin.toml`. Indexed + manifest
    /// loaded. This is the full experience.
    Twin(Twin),
}

impl TwinMode {
    /// Open a path, returning the appropriate mode.
    ///
    /// - If `path` is a file → `Orphan`.
    /// - If `path` is a directory containing `twin.toml` → `Twin`.
    /// - If `path` is a directory without `twin.toml` → `Folder`.
    pub fn open(path: &Path) -> Result<Self, TwinError> {
        let meta = std::fs::metadata(path).map_err(|e| TwinError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;

        if meta.is_file() {
            return Ok(TwinMode::Orphan(path.to_path_buf()));
        }

        if !meta.is_dir() {
            return Err(TwinError::NotAFileOrFolder(path.to_path_buf()));
        }

        let manifest_path = path.join(MANIFEST_FILENAME);
        let twin = Twin::index(path.to_path_buf())?;

        if manifest_path.is_file() {
            let manifest = TwinManifest::read(&manifest_path)?;
            Ok(TwinMode::Twin(Twin {
                manifest: Some(manifest),
                ..twin
            }))
        } else {
            Ok(TwinMode::Folder(twin))
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Twin — a loaded folder
// ─────────────────────────────────────────────────────────────────────────────

/// A folder that LunCoSim has indexed. May or may not have a `twin.toml`.
///
/// Both `TwinMode::Folder(Twin)` and `TwinMode::Twin(Twin)` return this
/// same struct — the distinction is whether `manifest` is `Some`.
///
/// All paths in [`files`](Self::files) are **relative to [`root`](Self::root)**
/// so the index survives the folder being moved.
#[derive(Debug)]
pub struct Twin {
    /// Absolute path to the folder on disk.
    pub root: PathBuf,
    /// The parsed `twin.toml`, if the folder has one.
    pub manifest: Option<TwinManifest>,
    /// Files discovered inside the folder, classified by extension.
    ///
    /// Excludes `twin.toml` itself and the hidden `.lunco/` directory.
    files: Vec<FileEntry>,
}

impl Twin {
    /// Walk the folder and classify every file by extension.
    ///
    /// Does *not* touch the manifest; callers pre-load that if present.
    fn index(root: PathBuf) -> Result<Self, TwinError> {
        let mut files = Vec::new();

        for entry in walkdir::WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                // Skip .lunco/ (session state), .git/, and any dotfile dir,
                // but never filter the root itself — it may legitimately live
                // under a dotfile parent (e.g. /tmp/.tmpXXXXXX on Linux).
                if e.depth() == 0 {
                    return true;
                }
                let name = e.file_name().to_string_lossy();
                !(e.file_type().is_dir() && name.starts_with('.'))
            })
        {
            let entry = entry.map_err(|e| TwinError::WalkDir(e.to_string()))?;
            if !entry.file_type().is_file() {
                continue;
            }

            let abs = entry.path();
            let rel = abs
                .strip_prefix(&root)
                .map_err(|_| TwinError::PathOutsideRoot {
                    path: abs.to_path_buf(),
                    root: root.clone(),
                })?
                .to_path_buf();

            // Skip the manifest file itself — it's not a Document or
            // file reference, it's metadata about the Twin.
            if rel == Path::new(MANIFEST_FILENAME) {
                continue;
            }

            let kind = FileKind::classify(&rel);
            files.push(FileEntry { relative_path: rel, kind });
        }

        // Stable order for reproducibility (tests, UI listings).
        files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        Ok(Twin {
            root,
            manifest: None,
            files,
        })
    }

    /// All files discovered inside the Twin, classified.
    pub fn files(&self) -> &[FileEntry] {
        &self.files
    }

    /// Iterate only files classified as [`Document`](FileKind::Document).
    pub fn documents(&self) -> impl Iterator<Item = &FileEntry> {
        self.files
            .iter()
            .filter(|e| matches!(e.kind, FileKind::Document(_)))
    }

    /// Iterate only files classified as [`FileReference`](FileKind::FileReference).
    pub fn file_references(&self) -> impl Iterator<Item = &FileEntry> {
        self.files
            .iter()
            .filter(|e| matches!(e.kind, FileKind::FileReference))
    }

    /// Returns true if this Twin has a loaded manifest (i.e. `twin.toml`
    /// exists on disk).
    pub fn has_manifest(&self) -> bool {
        self.manifest.is_some()
    }

    /// Walk the folder again and replace the file index. Useful after
    /// external edits (creating / deleting files outside LunCoSim).
    pub fn reload(&mut self) -> Result<(), TwinError> {
        let fresh = Twin::index(self.root.clone())?;
        self.files = fresh.files;
        Ok(())
    }

    /// Write the current manifest to `<root>/twin.toml`. Returns an error
    /// if `self.manifest` is `None` (there is nothing to save).
    pub fn save_manifest(&self) -> Result<(), TwinError> {
        let manifest = self.manifest.as_ref().ok_or(TwinError::NoManifest)?;
        manifest.write(&self.root.join(MANIFEST_FILENAME))
    }

    /// Promote a plain folder to a Twin by writing an initial `twin.toml`.
    ///
    /// Sets `self.manifest` to the provided manifest and persists it. No-op
    /// on the file index (the files are the same; only the manifest
    /// appeared).
    pub fn promote_to_twin(&mut self, manifest: TwinManifest) -> Result<(), TwinError> {
        manifest.write(&self.root.join(MANIFEST_FILENAME))?;
        self.manifest = Some(manifest);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn orphan_file_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("lonely.mo");
        write(&file, "model Lonely end Lonely;");

        match TwinMode::open(&file).unwrap() {
            TwinMode::Orphan(p) => assert_eq!(p, file),
            _ => panic!("expected Orphan mode"),
        }
    }

    #[test]
    fn folder_mode_no_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a.mo"), "model A end A;");
        write(&tmp.path().join("b.usda"), "#usda 1.0\n");
        write(&tmp.path().join("regolith.png"), "");

        match TwinMode::open(tmp.path()).unwrap() {
            TwinMode::Folder(twin) => {
                assert!(!twin.has_manifest());
                assert_eq!(twin.files().len(), 3);
                assert_eq!(twin.documents().count(), 2);
                assert_eq!(twin.file_references().count(), 1);
            }
            _ => panic!("expected Folder mode"),
        }
    }

    #[test]
    fn twin_mode_with_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("twin.toml"),
            r#"
name = "test_twin"
version = "0.1.0"
"#,
        );
        write(&tmp.path().join("rover.mo"), "model Rover end Rover;");

        match TwinMode::open(tmp.path()).unwrap() {
            TwinMode::Twin(twin) => {
                assert!(twin.has_manifest());
                assert_eq!(twin.manifest.as_ref().unwrap().name, "test_twin");
                // twin.toml itself is NOT in the file index
                assert_eq!(twin.files().len(), 1);
                assert_eq!(twin.documents().count(), 1);
            }
            _ => panic!("expected Twin mode"),
        }
    }

    #[test]
    fn dotfile_directories_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a.mo"), "model A end A;");
        write(&tmp.path().join(".lunco/layout.json"), "{}");
        write(&tmp.path().join(".git/config"), "");

        let TwinMode::Folder(twin) = TwinMode::open(tmp.path()).unwrap() else {
            panic!("expected Folder mode");
        };
        assert_eq!(twin.files().len(), 1, "dotfile dirs should not be indexed");
    }

    #[test]
    fn promote_folder_to_twin() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("thing.mo"), "model Thing end Thing;");

        let TwinMode::Folder(mut twin) = TwinMode::open(tmp.path()).unwrap() else {
            panic!("expected Folder mode");
        };
        assert!(!twin.has_manifest());

        let manifest = TwinManifest {
            name: "promoted".into(),
            description: None,
            version: "0.1.0".into(),
            default_workspace: None,
        };
        twin.promote_to_twin(manifest).unwrap();
        assert!(twin.has_manifest());
        assert!(tmp.path().join("twin.toml").is_file());

        // Re-opening picks up the manifest → now in Twin mode.
        let TwinMode::Twin(twin2) = TwinMode::open(tmp.path()).unwrap() else {
            panic!("expected Twin mode after promotion");
        };
        assert_eq!(twin2.manifest.unwrap().name, "promoted");
    }

    #[test]
    fn reload_picks_up_new_files() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("one.mo"), "model One end One;");

        let TwinMode::Folder(mut twin) = TwinMode::open(tmp.path()).unwrap() else {
            panic!();
        };
        assert_eq!(twin.files().len(), 1);

        write(&tmp.path().join("two.mo"), "model Two end Two;");
        twin.reload().unwrap();
        assert_eq!(twin.files().len(), 2);
    }

    #[test]
    fn save_manifest_fails_without_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a.mo"), "");
        let TwinMode::Folder(twin) = TwinMode::open(tmp.path()).unwrap() else {
            panic!();
        };
        assert!(matches!(twin.save_manifest(), Err(TwinError::NoManifest)));
    }

    #[test]
    fn missing_path_errors() {
        let err = TwinMode::open(Path::new("/definitely/does/not/exist/12345"));
        assert!(err.is_err());
    }
}

