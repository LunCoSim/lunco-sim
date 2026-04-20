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
pub use manifest::{TwinChildRef, TwinManifest, MANIFEST_FILENAME};

// Re-export lunco-doc and lunco-storage so downstream crates don't need
// to depend on them separately just to use the types Twin hands back.
pub use lunco_doc;
pub use lunco_storage;

use lunco_storage::StorageHandle;

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
        let mut twin = Twin::index(path.to_path_buf())?;

        if manifest_path.is_file() {
            let manifest = TwinManifest::read(&manifest_path)?;

            // Recursively open children with local paths. External URL
            // children are left for the future remote-twin pipeline;
            // they stay on the manifest but don't produce a loaded
            // sub-Twin today.
            let mut children = Vec::new();
            for child_ref in &manifest.children {
                let Some(rel) = &child_ref.path else { continue };
                let child_root = twin.root.join(rel);
                // Skip silently if the child folder is missing — a Twin
                // opening with a broken reference should still load the
                // parent cleanly, and the UI can surface a warning. An
                // error here would cascade and prevent any editing.
                if !child_root.is_dir() {
                    continue;
                }
                // Recurse via TwinMode::open so the child's own
                // manifest + children are discovered. Guard against
                // cycles by comparing canonical paths against
                // ancestors visited on this open.
                if matches_ancestor(&twin.root, &child_root) {
                    continue;
                }
                match TwinMode::open(&child_root)? {
                    TwinMode::Twin(t) | TwinMode::Folder(t) => children.push(t),
                    TwinMode::Orphan(_) => {}
                }
            }

            twin.manifest = Some(manifest);
            twin.children = children;
            Ok(TwinMode::Twin(twin))
        } else {
            Ok(TwinMode::Folder(twin))
        }
    }
}

/// Cycle guard: returns `true` when `candidate` canonicalises to the
/// same path as `ancestor`. Cheaper than walking the entire ancestry
/// because `TwinMode::open` always recurses from the parent downward,
/// so a direct-parent check catches the common broken case (manifest
/// that lists its own folder as a child).
fn matches_ancestor(ancestor: &Path, candidate: &Path) -> bool {
    match (ancestor.canonicalize(), candidate.canonicalize()) {
        (Ok(a), Ok(c)) => a == c,
        _ => false,
    }
}

/// Depth-first iterator over a Twin and its sub-Twins.
struct TwinWalkIter<'a> {
    stack: Vec<&'a Twin>,
}

impl<'a> Iterator for TwinWalkIter<'a> {
    type Item = &'a Twin;
    fn next(&mut self) -> Option<&'a Twin> {
        let t = self.stack.pop()?;
        // Push children reversed so the iteration order is stable
        // left-to-right in declaration order.
        for c in t.children.iter().rev() {
            self.stack.push(c);
        }
        Some(t)
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
///
/// # Recursion
///
/// If the manifest lists `[[twin.children]]` entries with local `path`s,
/// those child folders are eagerly opened as sub-Twins and stored in
/// [`children`](Self::children). External URL children are listed in
/// the manifest but not followed today.
///
/// `Clone` is implemented via a recursive copy of the manifest + file
/// index + sub-Twins. Trees are typically small (one digit twins, tens
/// of files each) so the cost is negligible; the clone is needed so a
/// Twin can be held simultaneously in the legacy `OpenTwin` resource
/// and the new `WorkspaceResource` during the migration period.
#[derive(Debug, Clone)]
pub struct Twin {
    /// Absolute path to the folder on disk.
    pub root: PathBuf,
    /// The parsed `twin.toml`, if the folder has one.
    pub manifest: Option<TwinManifest>,
    /// Files discovered inside the folder, classified by extension.
    ///
    /// Excludes `twin.toml` itself and the hidden `.lunco/` directory.
    files: Vec<FileEntry>,
    /// Sub-Twins loaded from `[[twin.children]]` with local `path`.
    /// Empty for plain folders and for twins with no children.
    children: Vec<Twin>,
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
            children: Vec::new(),
        })
    }

    /// All files discovered inside the Twin, classified.
    pub fn files(&self) -> &[FileEntry] {
        &self.files
    }

    /// Sub-Twins loaded from the manifest's `[[twin.children]]` with a
    /// local `path`. Read-only accessor so callers can't skip the
    /// open/load invariant. External-URL children are on the manifest
    /// but not here (not yet followed).
    pub fn children(&self) -> &[Twin] {
        &self.children
    }

    /// Storage handle pointing at this Twin's root folder. Convenience
    /// for callers that work in `lunco-storage` terms (e.g. the
    /// Workspace layer) rather than raw `PathBuf`.
    pub fn root_handle(&self) -> StorageHandle {
        StorageHandle::File(self.root.clone())
    }

    /// Whether `handle` refers to a file inside this Twin's folder
    /// **or any of its sub-Twins**. This is the core predicate
    /// Workspace uses to decide which Twin owns an open Document —
    /// without materialising a document list on the Twin side.
    ///
    /// Only meaningful for [`StorageHandle::File`] today. Other
    /// backends always return `false`.
    pub fn owns(&self, handle: &StorageHandle) -> bool {
        handle.is_under(&self.root_handle())
            || self.children.iter().any(|c| c.owns(handle))
    }

    /// Recursively walk every Twin in this tree (self first, then
    /// children depth-first). Useful for "save all", "find Twin owning
    /// this path", and similar workspace-level operations.
    pub fn walk(&self) -> impl Iterator<Item = &Twin> {
        TwinWalkIter {
            stack: vec![self],
        }
    }

    /// Locate the deepest Twin in this subtree whose folder contains
    /// `handle`. Returns `None` if no Twin in the tree owns it.
    pub fn find_owning(&self, handle: &StorageHandle) -> Option<&Twin> {
        // Depth-first so a sub-Twin wins over its parent when both
        // technically contain the file (matches the "enclosing twin"
        // rule — nearest twin.toml wins).
        for child in &self.children {
            if let Some(t) = child.find_owning(handle) {
                return Some(t);
            }
        }
        if handle.is_under(&self.root_handle()) {
            Some(self)
        } else {
            None
        }
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
            default_perspective: None,
            children: vec![],
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

    fn write_manifest(path: &Path, toml_body: &str) {
        write(path, toml_body);
    }

    #[test]
    fn recursive_twin_loads_local_children() {
        // Layout:
        //   root/twin.toml  (children: rover/, lander/)
        //   root/rover/twin.toml
        //   root/rover/Rover.mo
        //   root/lander/Lander.mo   (no twin.toml — child still loads as Folder)
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_manifest(
            &root.join("twin.toml"),
            r#"
name = "mission"
version = "0.1.0"

[[children]]
name = "rover"
path = "rover"

[[children]]
name = "lander"
path = "lander"
"#,
        );
        write_manifest(
            &root.join("rover/twin.toml"),
            r#"
name = "rover"
version = "0.1.0"
"#,
        );
        write(&root.join("rover/Rover.mo"), "model Rover end Rover;");
        write(&root.join("lander/Lander.mo"), "model Lander end Lander;");

        let TwinMode::Twin(t) = TwinMode::open(root).unwrap() else {
            panic!("expected Twin mode");
        };
        assert_eq!(t.children().len(), 2);
        assert_eq!(t.children()[0].manifest.as_ref().unwrap().name, "rover");
        // Second child had no manifest — loaded as Folder-variant twin.
        assert!(t.children()[1].manifest.is_none());
    }

    #[test]
    fn missing_child_folder_is_tolerated() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            &tmp.path().join("twin.toml"),
            r#"
name = "broken"
version = "0.1.0"

[[children]]
name = "ghost"
path = "does_not_exist"
"#,
        );
        let TwinMode::Twin(t) = TwinMode::open(tmp.path()).unwrap() else {
            panic!("expected Twin mode");
        };
        assert_eq!(t.children().len(), 0);
        // Manifest entry is still visible to the UI so it can surface
        // a "missing child" warning.
        assert_eq!(t.manifest.unwrap().children.len(), 1);
    }

    #[test]
    fn owns_predicate_respects_hierarchy() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_manifest(
            &root.join("twin.toml"),
            r#"
name = "parent"
version = "0.1.0"

[[children]]
name = "sub"
path = "sub"
"#,
        );
        write_manifest(
            &root.join("sub/twin.toml"),
            r#"
name = "sub"
version = "0.1.0"
"#,
        );
        write(&root.join("top.mo"), "model Top end Top;");
        write(&root.join("sub/inner.mo"), "model Inner end Inner;");

        let TwinMode::Twin(parent) = TwinMode::open(root).unwrap() else {
            panic!();
        };

        let top_handle = lunco_storage::StorageHandle::File(root.join("top.mo"));
        let inner_handle =
            lunco_storage::StorageHandle::File(root.join("sub/inner.mo"));
        let outside =
            lunco_storage::StorageHandle::File(tmp.path().parent().unwrap().join("elsewhere.mo"));

        assert!(parent.owns(&top_handle));
        assert!(parent.owns(&inner_handle));
        assert!(!parent.owns(&outside));

        // find_owning picks the deepest match: `sub` for the inner file.
        assert_eq!(
            parent.find_owning(&inner_handle).unwrap().root,
            root.join("sub").canonicalize().unwrap_or(root.join("sub"))
        );
        assert_eq!(
            parent.find_owning(&top_handle).unwrap().root,
            parent.root
        );
        assert!(parent.find_owning(&outside).is_none());
    }

    #[test]
    fn walk_visits_every_twin_in_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_manifest(
            &root.join("twin.toml"),
            r#"
name = "a"
version = "0.1.0"

[[children]]
name = "b"
path = "b"

[[children]]
name = "c"
path = "c"
"#,
        );
        write_manifest(
            &root.join("b/twin.toml"),
            r#"
name = "b"
version = "0.1.0"
"#,
        );
        write_manifest(
            &root.join("c/twin.toml"),
            r#"
name = "c"
version = "0.1.0"
"#,
        );

        let TwinMode::Twin(t) = TwinMode::open(root).unwrap() else {
            panic!();
        };
        let names: Vec<String> = t
            .walk()
            .map(|t| t.manifest.as_ref().unwrap().name.clone())
            .collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }
}

