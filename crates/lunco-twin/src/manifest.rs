//! `twin.toml` — the Twin manifest.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

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

    /// Stable cross-session identity for this Twin/scenario.
    ///
    /// This is the **scenario id** the networking scenario-sync layer
    /// keys client asset caches on (`cache_dir()/scenarios/<uuid>/…`).
    /// It is *stable* across restarts and renames once minted — unlike
    /// `TwinId(u64)` (re-minted every session) or the on-disk path
    /// (changes on move). The **content revision** (which assets make
    /// up the scenario *now*) is a separate SHA-256 digest computed by
    /// the scenario-manifest builder; the uuid says "this scenario",
    /// the digest says "this version of it".
    ///
    /// Absent on Twins authored before the field existed; minted
    /// automatically by [`TwinManifest::new`] and
    /// [`Twin::promote_to_twin`](crate::Twin::promote_to_twin). The
    /// networking layer falls back to a path-derived digest when this
    /// is `None`, so old `twin.toml` files keep working.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<Uuid>,

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

    /// Edit-journal settings (`[journal]` section). Absent means the
    /// defaults in [`JournalManifest`] — a session-only journal that
    /// writes nothing to disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal: Option<JournalManifest>,
}

/// The `[journal]` section of `twin.toml`.
///
/// The edit journal always records in memory — undo, replication and the
/// history panel read it live. This section governs only whether it is
/// **written to and read from `<twin>/history/journal.json`**.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct JournalManifest {
    /// Persist this Twin's edit history across sessions.
    ///
    /// **Off by default.** A journal file is a continuously-growing record
    /// of every authored edit; a Twin gets one only when its author asks
    /// for one, so merely opening a folder never starts writing into it.
    ///
    /// The flag is a single switch over both directions: off means the
    /// journal is neither loaded at open nor saved, so a session's history
    /// is always exactly what that session did. (Loading without saving
    /// would show a history that silently stops growing.)
    #[serde(default)]
    pub persist: bool,
}

/// The `[usd]` section of `twin.toml`.
///
/// The Twin's `.usda` files are a referenceable asset library — not auto-loaded.
/// Full resolution rule in `docs/architecture/21-domain-usd.md`
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

    /// Which of this Twin's `.usda` files are **loadable scenes** — the ones a
    /// Scenarios menu offers, as opposed to the vessels, structures, looks and
    /// library layers that exist to be *referenced* by them.
    ///
    /// Glob patterns relative to the Twin root; `*` matches within a path
    /// segment, `**` across segments.
    ///
    /// **Why the Twin answers this.** The question used to be answered by a
    /// hardcoded `rel.starts_with("scenes/")` in the sandbox's menu — the engine
    /// library's own folder layout, applied to every project. A Twin that keeps
    /// its scenes anywhere else (this one's `sim/scenes/`) had *none* of them
    /// listed: you could have a Twin's scene on screen and not find it in the
    /// list of scenes you could load. Where a project keeps its scenes is the
    /// project's business, so the project states it.
    ///
    /// `None` falls back to the conventional layouts (`scenes/**`,
    /// `sim/scenes/**`) so existing Twins keep working undeclared — but a Twin
    /// that says so is the one that cannot be surprised by a reorganisation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenes: Option<Vec<String>>,
}

/// The scene globs assumed for a Twin that declares none: the two layouts in
/// use across this workspace's twins. Kept as a fallback, not a rule — see
/// [`UsdManifest::scenes`].
pub const DEFAULT_SCENE_GLOBS: &[&str] = &["scenes/**", "sim/scenes/**"];

/// Match `path` (a `/`-separated Twin-relative path) against one glob, where
/// `*` matches within a segment and `**` matches across segments.
///
/// Deliberately tiny and dependency-free: this matches asset paths in a
/// manifest, not a filesystem — no character classes, no brace expansion, no
/// `.` special-casing.
pub fn glob_matches(glob: &str, path: &str) -> bool {
    fn seg_match(pat: &[&str], seg: &[&str]) -> bool {
        match pat.split_first() {
            None => seg.is_empty(),
            Some((&"**", rest)) => {
                // `**` is the tail-anything case: `scenes/**` matches everything
                // beneath `scenes/`, which is what an author means by it.
                if rest.is_empty() {
                    return true;
                }
                (0..=seg.len()).any(|i| seg_match(rest, &seg[i..]))
            }
            Some((p, rest)) => match seg.split_first() {
                Some((s, srest)) if star_match(p, s) => seg_match(rest, srest),
                _ => false,
            },
        }
    }
    /// `*` within one segment.
    fn star_match(pat: &str, s: &str) -> bool {
        match pat.split_once('*') {
            None => pat == s,
            Some((head, tail)) => {
                let Some(s) = s.strip_prefix(head) else { return false };
                (0..=s.len()).any(|k| star_match(tail, &s[k..]))
            }
        }
    }
    seg_match(
        &glob.split('/').collect::<Vec<_>>(),
        &path.split('/').collect::<Vec<_>>(),
    )
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
    /// Create a minimal manifest with a freshly-minted [`uuid`](Self::uuid)
    /// and the current schema `version`. Caller fills in `usd` / `children`
    /// / `description` / `default_perspective` as needed.
    ///
    /// Prefer this over a struct literal so the uuid invariant (present on
    /// newly-authored Twins) is upheld by construction.
    pub fn new(name: impl Into<String>) -> Self {
        TwinManifest {
            name: name.into(),
            description: None,
            version: "0.1.0".into(),
            uuid: Some(Uuid::new_v4()),
            default_perspective: None,
            children: Vec::new(),
            usd: None,
            journal: None,
        }
    }

    /// Return this manifest's stable id, minting one in place if absent.
    ///
    /// Used by [`Twin::promote_to_twin`](crate::Twin::promote_to_twin) so a
    /// folder promoted to a Twin persists a uuid on first save. Idempotent:
    /// a second call returns the already-minted id.
    pub fn ensure_uuid(&mut self) -> Uuid {
        *self.uuid.get_or_insert_with(Uuid::new_v4)
    }

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
            uuid: None,
            default_perspective: None,
            children: vec![],
            usd: None,
            journal: None,
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
            uuid: Some(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()),
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
                scenes: Some(vec!["scenes/**".into()]),
            }),
            journal: Some(JournalManifest { persist: true }),
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
            uuid: None,
            default_perspective: None,
            children: vec![],
            usd: None,
            journal: None,
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
        assert_eq!(parsed.uuid, None);

        // Re-serializing should not add the optional keys with null/empty values.
        let out = toml::to_string_pretty(&parsed).unwrap();
        assert!(!out.contains("description"));
        assert!(!out.contains("default_perspective"));
        assert!(!out.contains("children"));
        assert!(!out.contains("usd"));
        assert!(!out.contains("uuid"));
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

    #[test]
    fn uuid_round_trips_when_present() {
        let id = Uuid::new_v4();
        let text = format!(
            r#"
name = "tracked"
version = "0.1.0"
uuid = "{id}"
"#
        );
        let parsed: TwinManifest = toml::from_str(&text).unwrap();
        assert_eq!(parsed.uuid, Some(id));
        // Re-serialize keeps the key (it's `Some`).
        let out = toml::to_string_pretty(&parsed).unwrap();
        assert!(out.contains("uuid"));
    }

    #[test]
    fn new_mints_uuid_and_current_schema_version() {
        let m = TwinManifest::new("fresh");
        assert_eq!(m.name, "fresh");
        assert_eq!(m.version, "0.1.0");
        assert!(m.uuid.is_some(), "new() must mint a uuid");
        // Two calls mint distinct ids.
        assert_ne!(TwinManifest::new("fresh").uuid, m.uuid);
    }

    #[test]
    fn ensure_uuid_is_idempotent() {
        let mut m = TwinManifest::new("x");
        let first = m.ensure_uuid();
        let second = m.ensure_uuid();
        assert_eq!(first, second, "ensure_uuid must not re-mint");
        // A manifest with no uuid gets one minted on first ensure.
        let mut bare = TwinManifest {
            name: "bare".into(),
            description: None,
            version: "0.1.0".into(),
            uuid: None,
            default_perspective: None,
            children: vec![],
            usd: None,
            journal: None,
        };
        let minted = bare.ensure_uuid();
        assert!(bare.uuid == Some(minted));
    }

    #[test]
    fn scene_globs_match_both_conventional_layouts() {
        // The engine library's layout and this workspace's twins' layout.
        assert!(glob_matches("scenes/**", "scenes/tests/comms_demo.usda"));
        assert!(glob_matches("sim/scenes/**", "sim/scenes/traverse.usda"));
        // …and nothing else in the twin: a rover is referenced BY a scene, not
        // offered as one. This is the whole point of asking the twin.
        assert!(!glob_matches("sim/scenes/**", "sim/rovers/awful.usda"));
        assert!(!glob_matches("scenes/**", "vessels/rovers/skid_rover.usda"));
    }

    #[test]
    fn glob_star_stays_inside_one_segment() {
        assert!(glob_matches("sim/scenes/*.usda", "sim/scenes/traverse.usda"));
        // `*` must not eat the separator, or "scenes/*.usda" would claim every
        // nested file and the pattern would say less than the author meant.
        assert!(!glob_matches("sim/scenes/*.usda", "sim/scenes/old/traverse.usda"));
        assert!(glob_matches("sim/scenes/**", "sim/scenes/old/traverse.usda"));
        // Prefix/suffix around the star.
        assert!(glob_matches("sim/scenes/traverse*.usda", "sim/scenes/traverse_apollo15.usda"));
        assert!(!glob_matches("sim/scenes/traverse*.usda", "sim/scenes/other.usda"));
    }

    #[test]
    fn declared_scenes_round_trip_through_toml() {
        let text = "name = \"t\"\nversion = \"0.1.0\"\n\n[usd]\ndefault_scene = \"sim/scenes/traverse.usda\"\nscenes = [\"sim/scenes/*.usda\"]\n";
        let parsed: TwinManifest = toml::from_str(text).unwrap();
        let usd = parsed.usd.expect("[usd] section");
        assert_eq!(usd.scenes.as_deref(), Some(&["sim/scenes/*.usda".to_string()][..]));
    }

    #[test]
    fn a_twin_that_declares_nothing_still_finds_its_scenes() {
        // Undeclared is the existing state of every twin on disk; the fallback
        // is what keeps them working until they say so themselves.
        assert!(DEFAULT_SCENE_GLOBS.iter().any(|g| glob_matches(g, "sim/scenes/traverse.usda")));
        assert!(DEFAULT_SCENE_GLOBS.iter().any(|g| glob_matches(g, "scenes/tests/link.usda")));
        assert!(!DEFAULT_SCENE_GLOBS.iter().any(|g| glob_matches(g, "models/rover.usda")));
    }
}
