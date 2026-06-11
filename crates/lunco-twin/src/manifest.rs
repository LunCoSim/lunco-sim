//! `twin.toml` — the Twin manifest.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use lunco_storage::{FileStorage, Storage, StorageError, StorageHandle};

use crate::error::TwinError;

/// Collapse a [`StorageError`] into the `std::io::Error` that
/// [`TwinError::Io`] carries, so routing manifest I/O through
/// `lunco-storage` (instead of direct `std::fs`, which is clippy-banned
/// here and absent on wasm) keeps the existing error shape.
fn storage_io(e: StorageError) -> std::io::Error {
    match e {
        StorageError::Io(io) => io,
        other => std::io::Error::other(other.to_string()),
    }
}

/// Name of the Twin manifest file at the root of a Twin folder.
pub const MANIFEST_FILENAME: &str = "twin.toml";

/// The parsed contents of `twin.toml`.
///
/// Kept deliberately small. Fields are added as concrete UI flows need
/// them — speculative fields rot faster than they help.
///
/// # Recursion
///
/// A Twin may nest other Twins via the `children` list. Each child is
/// either a **local** folder path relative to the parent (loaded
/// eagerly when the parent opens) or an **external** reference by URL
/// (not yet followed; reserved for remote-twin support). This mirrors
/// Cargo's `[workspace.members]` — a twin.toml describes one Twin and
/// optionally the Twins it composes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TwinManifest {
    /// Human-readable name of the Twin.
    pub name: String,

    /// Optional long-form description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Manifest schema version. Today always `"0.1.0"`; reserved for
    /// future breaking changes to the manifest format.
    pub version: String,

    /// Which **Perspective** (layout preset — `"build"`, `"simulate"`,
    /// `"analyze"`) to activate when this Twin opens. Perspectives are
    /// defined by the app; the manifest stores only the identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_perspective: Option<String>,

    /// Sub-Twins composed into this Twin. Empty for leaf twins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TwinChildRef>,

    /// USD domain settings (`[usd]` section). Holds the Twin's starting
    /// scene; absent for Twins with no USD content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd: Option<UsdManifest>,
}

/// The `[usd]` section of `twin.toml`.
///
/// Today carries only the entry-point scene. The Twin's other `.usda`
/// files are a referenceable asset library — not auto-loaded. Full
/// resolution rule in `docs/architecture/21-domain-usd.md`
/// § "Which stage opens".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct UsdManifest {
    /// Entry-point USD stage — the one loaded as the active stage when
    /// the Twin opens. Path is **relative to the Twin root**. `None`
    /// means "no starting scene declared" (the Twin opens like a plain
    /// folder: files indexed, nothing auto-loaded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_scene: Option<String>,
}

/// Reference to a sub-Twin. Local for now; remote URLs reserved for
/// future "point this child at an IPFS/HTTPS twin bundle" support.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TwinChildRef {
    /// Logical name for the child. Displayed in the Twin Browser as the
    /// node label; does not need to match the folder name on disk but
    /// conventionally does.
    pub name: String,

    /// Folder path relative to the parent Twin's root. Mutually
    /// exclusive with [`url`](Self::url).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,

    /// Remote reference (`https://…`, `ipfs://…`). Not yet followed at
    /// open time — reserved for the remote-twin milestone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl TwinManifest {
    /// Read and parse `twin.toml` from disk.
    pub fn read(path: &Path) -> Result<Self, TwinError> {
        let handle = StorageHandle::File(path.to_path_buf());
        let bytes = FileStorage::new()
            .read_sync(&handle)
            .map_err(|e| TwinError::Io {
                path: path.to_path_buf(),
                source: storage_io(e),
            })?;
        let text = String::from_utf8(bytes).map_err(|e| TwinError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        })?;
        Ok(toml::from_str(&text)?)
    }

    /// Serialize and write this manifest to disk. Overwrites if present.
    pub fn write(&self, path: &Path) -> Result<(), TwinError> {
        let text = toml::to_string_pretty(self)?;
        let handle = StorageHandle::File(path.to_path_buf());
        FileStorage::new()
            .write_sync(&handle, text.as_bytes())
            .map_err(|e| TwinError::Io {
                path: path.to_path_buf(),
                source: storage_io(e),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_minimal() {
        let manifest = TwinManifest {
            name: "demo".into(),
            description: None,
            version: "0.1.0".into(),
            default_perspective: None,
            children: vec![],
            usd: None,
        };
        let text = toml::to_string_pretty(&manifest).unwrap();
        let parsed: TwinManifest = toml::from_str(&text).unwrap();
        assert_eq!(parsed, manifest);
    }

    #[test]
    fn round_trip_full() {
        let manifest = TwinManifest {
            name: "lunar_base".into(),
            description: Some("a research outpost".into()),
            version: "0.1.0".into(),
            default_perspective: Some("simulate".into()),
            children: vec![
                TwinChildRef {
                    name: "rover".into(),
                    path: Some("rover/".into()),
                    url: None,
                },
                TwinChildRef {
                    name: "shared_sensors".into(),
                    path: None,
                    url: Some("https://twins.lunco.space/sensors".into()),
                },
            ],
            usd: Some(UsdManifest {
                default_scene: Some("main_scene.usda".into()),
            }),
        };
        let text = toml::to_string_pretty(&manifest).unwrap();
        let parsed: TwinManifest = toml::from_str(&text).unwrap();
        assert_eq!(parsed, manifest);
    }

    #[test]
    fn disk_round_trip_via_storage() {
        // Exercises the `lunco-storage`-backed read/write path end-to-end.
        let manifest = TwinManifest {
            name: "disk_demo".into(),
            description: Some("written via FileStorage".into()),
            version: "0.1.0".into(),
            default_perspective: None,
            children: vec![],
            usd: None,
        };
        let path = std::env::temp_dir().join(format!(
            "lunco_twin_manifest_{}.toml",
            std::process::id()
        ));
        manifest.write(&path).expect("write via storage");
        let read_back = TwinManifest::read(&path).expect("read via storage");
        assert_eq!(read_back, manifest);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unknown_field_rejected() {
        let text = r#"
name = "x"
version = "0.1.0"
rogue_field = true
"#;
        let result: Result<TwinManifest, _> = toml::from_str(text);
        assert!(result.is_err());
    }

    #[test]
    fn omitted_optionals_round_trip_cleanly() {
        let text = r#"
name = "x"
version = "0.1.0"
"#;
        let parsed: TwinManifest = toml::from_str(text).unwrap();
        assert_eq!(parsed.description, None);
        assert_eq!(parsed.default_perspective, None);
        assert!(parsed.children.is_empty());
        assert_eq!(parsed.usd, None);

        // Re-serializing should not add the optional keys with null/empty values.
        let out = toml::to_string_pretty(&parsed).unwrap();
        assert!(!out.contains("description"));
        assert!(!out.contains("default_perspective"));
        assert!(!out.contains("children"));
        assert!(!out.contains("usd"));
    }

    #[test]
    fn usd_default_scene_parses() {
        let text = r#"
name = "rig"
version = "0.1.0"

[usd]
default_scene = "scenes/main.usda"
"#;
        let parsed: TwinManifest = toml::from_str(text).unwrap();
        assert_eq!(
            parsed.usd.unwrap().default_scene.as_deref(),
            Some("scenes/main.usda")
        );
    }
}
