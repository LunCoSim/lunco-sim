//! Typed identity for a Modelica class across the workbench.
//!
//! `ClassRef` is the canonical replacement for the legacy string ID
//! schemes that previously identified classes in different parts of
//! the codebase. Tree clicks, palette drops, drill-ins, typed API
//! commands, and session restore all converge on this one type, so
//! lookups for "open this", "is this loaded?", "what's its
//! documentation?", "where's its source file?" share a single path
//! by construction.
//!
//! ## Legacy scheme → `ClassRef`
//!
//! | Legacy string | `ClassRef` form |
//! |---|---|
//! | `msl_path:Modelica.Blocks.Examples.PID_Controller` | `ClassRef::msl(["Blocks","Examples","PID_Controller"])` |
//! | `bundled://AnnotatedRocketStage.mo#AnnotatedRocketStage.RocketStage` | `ClassRef::bundled(["AnnotatedRocketStage","RocketStage"])` |
//! | `/abs/path/to/MyModel.mo` | `ClassRef::user_file("/abs/path/to/MyModel.mo", [])` |
//! | `mem://Untitled1` | `ClassRef::untitled(doc_id, ["Untitled1"])` (needs doc id) |
//!
//! The [`Library`] field decides loading + resolution (filesystem
//! walk vs. in-memory map vs. user document registry). The
//! [`ClassRef::path`] field is the qualified name **within that
//! library's root** — so `Modelica` is *not* part of the path for an
//! MSL class. [`ClassRef::qualified`] joins them when callers need
//! the absolute MSL-style name (e.g. drill-in target for projection,
//! display in error messages).

use lunco_doc::DocumentId;
use std::path::PathBuf;

/// Where a class comes from. Each variant drives a distinct loading
/// strategy in the resolver.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Library {
    /// Modelica Standard Library — `<cache>/msl/Modelica/...` on disk,
    /// supplemented by the pre-baked `msl_index.json` component
    /// catalogue.
    Msl,
    /// Bundled LunCoSim example shipped in the binary. Source lookup
    /// goes through [`crate::models::get_model`]; metadata comes from
    /// the same `msl_index.json` (under its `bundled` section).
    Bundled,
    /// Third-party Modelica library cached on disk by
    /// `lunco-assets`. `cache_subdir` is the `Assets.toml` `dest`
    /// (e.g. `"thermofluidstream"`), `root` is the top-level
    /// package directory inside it (e.g. `"ThermofluidStream"`).
    ThirdParty { cache_subdir: String, root: String },
    /// A user-opened file on disk (Open File dialog, drag-drop, Twin
    /// folder file). `path` is the canonical filesystem path; the
    /// class identity is `(path, ClassRef::path)`.
    UserFile { path: PathBuf },
    /// In-memory document (`CreateNewScratchModel`, duplicate from
    /// a read-only library class). The document id is the stable
    /// identity — the name is convenience only.
    Untitled(DocumentId),
}

impl Library {
    /// Display name of the library's top-level package. Used as the
    /// first segment when joining to an absolute qualified name in
    /// [`ClassRef::qualified`].
    ///
    /// Returns the empty string for libraries that don't have a
    /// stable top-level name baked into the library identity
    /// (`UserFile`, `Untitled`, `Bundled`) — for those, the
    /// qualified name starts from [`ClassRef::path`] directly.
    pub fn root_name(&self) -> &str {
        match self {
            Library::Msl => "Modelica",
            Library::ThirdParty { root, .. } => root,
            Library::Bundled | Library::UserFile { .. } | Library::Untitled(_) => "",
        }
    }
}

/// Identity of a class anywhere in the workbench.
///
/// Constructed once at the boundary (tree click, typed command,
/// session restore) and flows through opening, drill-in, tab dedup,
/// projection target lookup, and documentation lookup without ever
/// being converted back to an untyped string until display time.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClassRef {
    pub library: Library,
    /// Qualified path within the library, segment by segment. Empty
    /// when the reference identifies the library root itself or a
    /// file with no specific class drill target (e.g. an open-file
    /// gesture before the projector picks a fallback).
    pub path: Vec<String>,
}

impl ClassRef {
    pub fn new(library: Library, path: Vec<String>) -> Self {
        Self { library, path }
    }

    pub fn msl<I, S>(path: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::new(Library::Msl, path.into_iter().map(Into::into).collect())
    }

    pub fn bundled<I, S>(path: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::new(Library::Bundled, path.into_iter().map(Into::into).collect())
    }

    pub fn third_party<I, S>(
        cache_subdir: impl Into<String>,
        root: impl Into<String>,
        path: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::new(
            Library::ThirdParty {
                cache_subdir: cache_subdir.into(),
                root: root.into(),
            },
            path.into_iter().map(Into::into).collect(),
        )
    }

    pub fn user_file<I, S>(path_on_disk: impl Into<PathBuf>, qualified: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::new(
            Library::UserFile {
                path: path_on_disk.into(),
            },
            qualified.into_iter().map(Into::into).collect(),
        )
    }

    pub fn untitled<I, S>(doc: DocumentId, qualified: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::new(
            Library::Untitled(doc),
            qualified.into_iter().map(Into::into).collect(),
        )
    }

    /// Absolute qualified name with the library root prepended.
    ///
    /// - `ClassRef::msl(["Blocks","Examples","PID_Controller"])` →
    ///   `"Modelica.Blocks.Examples.PID_Controller"`.
    /// - `ClassRef::third_party("thermofluidstream", "ThermofluidStream",
    ///   ["Boundaries","CreateState"])` →
    ///   `"ThermofluidStream.Boundaries.CreateState"`.
    /// - Libraries with empty `root_name()` (`Bundled`, `UserFile`,
    ///   `Untitled`) start the qualified name from `path` directly.
    pub fn qualified(&self) -> String {
        let root = self.library.root_name();
        if root.is_empty() {
            self.path.join(".")
        } else if self.path.is_empty() {
            root.to_string()
        } else {
            format!("{}.{}", root, self.path.join("."))
        }
    }

    /// Last segment of the qualified path, or the library root if
    /// the path is empty. The short display name used in tab titles
    /// and tree rows.
    pub fn short_name(&self) -> &str {
        if let Some(last) = self.path.last() {
            last.as_str()
        } else {
            self.library.root_name()
        }
    }

    /// Whether the reference identifies the library root itself
    /// (e.g. clicking the `Modelica` row in the tree, not a class
    /// inside it).
    pub fn is_library_root(&self) -> bool {
        self.path.is_empty()
    }

    /// Parse a legacy tree-row ID string into a `ClassRef`. Returns
    /// `None` when the string doesn't match a recognised scheme or
    /// requires state the parser can't see (e.g. `mem://` lookups
    /// need the in-memory cache to resolve a `DocumentId`).
    ///
    /// Recognised schemes:
    /// - `msl_path:<qualified>` → MSL or third-party (based on the
    ///   first qualified segment).
    /// - `bundled://<file>` and `bundled://<file>#<qualified>` →
    ///   [`Library::Bundled`].
    /// - `file:///<abs>` and any absolute `.mo` path → [`Library::UserFile`].
    pub fn parse_tree_id(s: &str) -> Option<Self> {
        if let Some(qualified) = s.strip_prefix("msl_path:") {
            return parse_msl_qualified(qualified);
        }
        if let Some(tail) = s.strip_prefix("bundled://") {
            return Some(parse_bundled(tail));
        }
        if let Some(rest) = s.strip_prefix("file://") {
            return Some(ClassRef::user_file(
                PathBuf::from(rest),
                Vec::<String>::new(),
            ));
        }
        // mem:// identifiers don't carry a DocumentId; resolution
        // requires consulting the in-memory model cache.
        if s.starts_with("mem://") {
            return None;
        }
        let path = std::path::Path::new(s);
        if path.is_absolute()
            && path
                .extension()
                .map(|e| e.eq_ignore_ascii_case("mo"))
                .unwrap_or(false)
        {
            return Some(ClassRef::user_file(
                path.to_path_buf(),
                Vec::<String>::new(),
            ));
        }
        None
    }
}

fn parse_msl_qualified(qualified: &str) -> Option<ClassRef> {
    if qualified.is_empty() {
        return None;
    }
    let mut parts = qualified.split('.').map(String::from);
    let head = parts.next()?;
    let tail: Vec<String> = parts.collect();
    match head.as_str() {
        "Modelica" => Some(ClassRef::new(Library::Msl, tail)),
        // Third-party libraries: we don't know the on-disk
        // `cache_subdir` from the qualified name alone — the
        // discovery scan owns that mapping. Default to a lowercase
        // guess; the resolver can override when it has the
        // authoritative pairing.
        other => Some(ClassRef::new(
            Library::ThirdParty {
                cache_subdir: other.to_lowercase(),
                root: other.to_string(),
            },
            tail,
        )),
    }
}

fn parse_bundled(tail: &str) -> ClassRef {
    let (filename, frag) = match tail.split_once('#') {
        Some((f, q)) => (f, Some(q)),
        None => (tail, None),
    };
    let stem = filename.strip_suffix(".mo").unwrap_or(filename);
    let mut path: Vec<String> = vec![stem.to_string()];
    if let Some(q) = frag {
        // Fragment is the fully-qualified path including the top
        // class (= file stem). Replace `path` with the fragment if
        // it starts with the stem, otherwise nest the fragment under
        // the stem.
        let parts: Vec<&str> = q.split('.').collect();
        if parts.first().map(|s| s.eq(&stem)).unwrap_or(false) {
            path = parts.into_iter().map(String::from).collect();
        } else {
            path.extend(parts.into_iter().map(String::from));
        }
    }
    ClassRef::new(Library::Bundled, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msl_qualified_round_trip() {
        let c =
            ClassRef::parse_tree_id("msl_path:Modelica.Blocks.Examples.PID_Controller").unwrap();
        assert_eq!(c.library, Library::Msl);
        assert_eq!(c.path, vec!["Blocks", "Examples", "PID_Controller"]);
        assert_eq!(c.qualified(), "Modelica.Blocks.Examples.PID_Controller");
        assert_eq!(c.short_name(), "PID_Controller");
        assert!(!c.is_library_root());
    }

    #[test]
    fn third_party_msl_path_routes_to_third_party_variant() {
        let c =
            ClassRef::parse_tree_id("msl_path:ThermofluidStream.Boundaries.CreateState").unwrap();
        match &c.library {
            Library::ThirdParty { cache_subdir, root } => {
                assert_eq!(cache_subdir, "thermofluidstream");
                assert_eq!(root, "ThermofluidStream");
            }
            other => panic!("expected ThirdParty, got {other:?}"),
        }
        assert_eq!(c.path, vec!["Boundaries", "CreateState"]);
        assert_eq!(c.qualified(), "ThermofluidStream.Boundaries.CreateState");
    }

    #[test]
    fn bundled_with_fragment() {
        let c = ClassRef::parse_tree_id(
            "bundled://AnnotatedRocketStage.mo#AnnotatedRocketStage.RocketStage",
        )
        .unwrap();
        assert_eq!(c.library, Library::Bundled);
        assert_eq!(c.path, vec!["AnnotatedRocketStage", "RocketStage"]);
        assert_eq!(c.qualified(), "AnnotatedRocketStage.RocketStage");
        assert_eq!(c.short_name(), "RocketStage");
    }

    #[test]
    fn bundled_fragment_already_prefixed_does_not_double() {
        let c =
            ClassRef::parse_tree_id("bundled://AnnotatedRocketStage.mo#AnnotatedRocketStage.Tank")
                .unwrap();
        // Stem ("AnnotatedRocketStage") matches frag head — don't
        // produce "AnnotatedRocketStage.AnnotatedRocketStage.Tank".
        assert_eq!(c.path, vec!["AnnotatedRocketStage", "Tank"]);
    }

    #[test]
    fn bundled_fragment_unprefixed_nests_under_stem() {
        let c = ClassRef::parse_tree_id("bundled://Demo.mo#Helper.Thing").unwrap();
        assert_eq!(c.path, vec!["Demo", "Helper", "Thing"]);
    }

    #[test]
    fn bundled_without_fragment_is_top_level() {
        let c = ClassRef::parse_tree_id("bundled://AnnotatedRocketStage.mo").unwrap();
        assert_eq!(c.library, Library::Bundled);
        assert_eq!(c.path, vec!["AnnotatedRocketStage"]);
        assert_eq!(c.qualified(), "AnnotatedRocketStage");
        assert_eq!(c.short_name(), "AnnotatedRocketStage");
    }

    #[test]
    fn user_file_absolute_path() {
        let c = ClassRef::parse_tree_id("/home/user/models/MyRocket.mo").unwrap();
        match &c.library {
            Library::UserFile { path } => {
                assert_eq!(path.as_os_str(), "/home/user/models/MyRocket.mo");
            }
            other => panic!("expected UserFile, got {other:?}"),
        }
        assert!(c.path.is_empty());
    }

    #[test]
    fn file_url_user_file() {
        let c = ClassRef::parse_tree_id("file:///home/user/MyRocket.mo").unwrap();
        assert!(matches!(c.library, Library::UserFile { .. }));
    }

    #[test]
    fn mem_scheme_returns_none() {
        assert!(ClassRef::parse_tree_id("mem://Untitled1").is_none());
    }

    #[test]
    fn unknown_scheme_returns_none() {
        assert!(ClassRef::parse_tree_id("garbage").is_none());
        assert!(ClassRef::parse_tree_id("").is_none());
        assert!(ClassRef::parse_tree_id("relative/path.mo").is_none());
    }

    #[test]
    fn library_root_short_name_falls_back_to_library() {
        let c = ClassRef::new(Library::Msl, Vec::new());
        assert!(c.is_library_root());
        assert_eq!(c.short_name(), "Modelica");
        assert_eq!(c.qualified(), "Modelica");
    }

    #[test]
    fn bundled_root_short_name_is_path_head() {
        let c = ClassRef::bundled(["DemoModel"]);
        assert_eq!(c.short_name(), "DemoModel");
        assert_eq!(c.qualified(), "DemoModel");
    }

    #[test]
    fn user_file_with_drilled_class_qualifies_from_path() {
        let c = ClassRef::user_file("/tmp/foo.mo", ["MyPkg", "Inner"]);
        assert_eq!(c.qualified(), "MyPkg.Inner");
        assert_eq!(c.short_name(), "Inner");
    }

    #[test]
    fn third_party_qualified_includes_library_root() {
        let c = ClassRef::third_party(
            "thermofluidstream",
            "ThermofluidStream",
            ["Utilities", "DropOfCommons"],
        );
        assert_eq!(c.qualified(), "ThermofluidStream.Utilities.DropOfCommons");
        assert_eq!(c.short_name(), "DropOfCommons");
    }

    #[test]
    fn equality_includes_full_library_state() {
        let a = ClassRef::third_party("foo", "Foo", ["X"]);
        let b = ClassRef::third_party("foo", "Foo", ["X"]);
        let c = ClassRef::third_party("bar", "Foo", ["X"]);
        assert_eq!(a, b);
        assert_ne!(
            a, c,
            "ThirdParty libraries with different cache subdirs must not compare equal"
        );
    }
}
