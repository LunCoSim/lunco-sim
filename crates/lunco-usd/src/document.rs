//! `UsdDocument` — the canonical Document representation of one USD
//! source file (`.usda` for now; `.usdc` deferred).
//!
//! Mirrors the shape of [`lunco_modelica::ModelicaDocument`]:
//! source text is canonical, ops mutate text, generation bumps on
//! every change, last-saved-generation gates the dirty flag, a bounded
//! ring of recent changes lets views catch up without polling.
//!
//! ## Why text-canonical
//!
//! USD `.usda` is plain text. Treating the text as canonical (rather
//! than the parsed `TextReader`) gives us:
//!
//! - Lossless round-trip with external USD tools (Omniverse, USDView,
//!   Blender) — comments and formatting survive untouched until an op
//!   actually rewrites their byte range.
//! - One mutation path: every op funnels through a `(range, replacement)`
//!   patch, same as Modelica. No parallel "in-memory tree" representation
//!   to keep in sync.
//! - Trivial Phase 1: a `ReplaceSource` op is enough to plumb the
//!   `Document` trait + undo/redo without committing to a prim-level op
//!   shape yet (that lands in Phase 5).
//!
//! ## Edit target
//!
//! Per the Omniverse pattern, every `UsdOp` carries an `edit_target:
//! LayerId` so future composition-aware editing can name *which layer*
//! receives an opinion. Phase 1 only knows about the root layer
//! ([`LayerId::root`]); the field exists so Phase 5 can extend without
//! repainting the type.

use std::collections::VecDeque;
use std::ops::Range;

use bevy::reflect::Reflect;
use lunco_doc::{Document, DocumentError, DocumentId, DocumentOp, DocumentOrigin};

use crate::text_edit;

/// How many recent changes to keep in the per-document ring buffer.
///
/// Views consume the suffix via [`UsdDocument::changes_since`]; 256 is
/// generous for realistic edit cadences without growing unbounded.
const CHANGE_HISTORY_CAPACITY: usize = 256;

// ─────────────────────────────────────────────────────────────────────
// LayerId — names a layer in a stage's layer stack
// ─────────────────────────────────────────────────────────────────────

/// Identifies one layer in a USD stage's layer stack.
///
/// In Phase 1 every document is a single root layer and every op
/// targets [`LayerId::root`]. The newtype exists now so future
/// sublayer-aware editing can extend without changing op shapes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Reflect, serde::Serialize, serde::Deserialize)]
pub struct LayerId(String);

impl LayerId {
    /// The root layer of a stage — the file the document was opened from.
    pub fn root() -> Self {
        Self("@root@".to_string())
    }

    /// Wrap an arbitrary layer identifier (path or anonymous handle).
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The raw identifier string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when this id refers to the document's root layer.
    pub fn is_root(&self) -> bool {
        self.0 == "@root@"
    }
}

impl Default for LayerId {
    fn default() -> Self {
        Self::root()
    }
}

// ─────────────────────────────────────────────────────────────────────
// UsdChange — Omniverse-style change notification
// ─────────────────────────────────────────────────────────────────────

/// Coarse-grained change classification, modelled on USD's
/// `Tf::Notice` split between resync (structural) and info-only
/// (attribute value) changes.
///
/// Views subscribe to the kinds they care about — the prim-tree
/// browser only rebuilds on `Resync`; the property inspector reacts
/// to `InfoOnly` for the selected prim. This is the plumbing that
/// keeps frame discipline (see `AGENTS.md` §7) when a single attr
/// edit happens on a 100k-prim stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsdChange {
    /// Structural change: prim added, removed, renamed, or moved.
    /// Forces a tree rebuild.
    Resync {
        /// Prim path (or `/` for whole-stage replacement).
        path: String,
    },
    /// Attribute value changed; tree shape unchanged.
    InfoOnly {
        /// Prim path whose attribute changed.
        path: String,
        /// Attribute name (e.g. `xformOp:translate`).
        attr: String,
    },
    /// Whole source replaced — every observer should refresh.
    /// Used by `ReplaceSource` and Save-As round-trips.
    FullReload,
}

// ─────────────────────────────────────────────────────────────────────
// UsdOp — typed mutation
// ─────────────────────────────────────────────────────────────────────

/// A typed, reversible mutation to a [`UsdDocument`].
///
/// Every variant carries an `edit_target: LayerId` so future
/// composition-aware editing can name *which layer* receives the
/// opinion. Today only [`LayerId::root`] is meaningful; non-root
/// targets are rejected.
///
/// All variants funnel through [`text_edit`] splicers so the source
/// stays canonical (comments and formatting in untouched regions
/// survive). Inverses are recorded as the previous full source via
/// [`UsdOp::ReplaceSource`] — coarse but always correct, and good
/// enough until per-op tighter inverses become a profiling target.
#[derive(Debug, Clone, Reflect, serde::Serialize, serde::Deserialize)]
pub enum UsdOp {
    /// Replace the entire source buffer with `text`. Inverse is the
    /// previous source as another `ReplaceSource`. Used as the
    /// universal inverse for every other variant.
    ReplaceSource {
        /// Layer to write to. Today: always [`LayerId::root`].
        edit_target: LayerId,
        /// New full source for the layer.
        text: String,
    },
    /// Add a child prim under `parent_path` with the given prim
    /// `name` and optional schema `type_name` (`"Xform"`, `"Cube"`,
    /// …; `None` for an untyped prim). `parent_path == "/"` adds at
    /// the file root.
    AddPrim {
        /// Layer to write to.
        edit_target: LayerId,
        /// Parent prim path (`"/"` for top level).
        parent_path: String,
        /// Prim name — must be a valid USD identifier in the
        /// caller's hands; not validated here beyond what the
        /// splicer accepts.
        name: String,
        /// Optional schema type (`Xform`, `Cube`, `Mesh`, …).
        type_name: Option<String>,
    },
    /// Remove the prim at `path` together with its entire `def ... { }`
    /// block. The inverse re-establishes the prior full source.
    RemovePrim {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim to remove.
        path: String,
    },
    /// Set the `xformOp:translate` attribute on the prim at `path`.
    /// Inserts the property + `xformOpOrder` if absent; replaces the
    /// value otherwise.
    SetTranslate {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose translate to set.
        path: String,
        /// `[x, y, z]` in stage units.
        value: [f64; 3],
    },
    /// Set an arbitrary attribute on the prim at `path`.
    /// If the attribute is already authored, its right-hand side is replaced;
    /// otherwise a new attribute line is inserted at the top of the prim body
    /// with the given `type_name`.
    SetAttribute {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose attribute to set.
        path: String,
        /// The name of the attribute (e.g. `primvars:displayColor` or `inputs:roughness`).
        name: String,
        /// The USD type name of the attribute (e.g. `color3f` or `float`).
        type_name: String,
        /// The attribute value formatted as a USD-compliant string.
        value: String,
    },
}

impl Default for UsdOp {
    fn default() -> Self {
        // `Reflect`-derived enums need a Default. Pick the always-valid
        // identity variant: a no-op ReplaceSource of empty text. Real
        // callers always supply an explicit variant.
        UsdOp::ReplaceSource {
            edit_target: LayerId::root(),
            text: String::new(),
        }
    }
}

impl DocumentOp for UsdOp {}

// ─────────────────────────────────────────────────────────────────────
// UsdDocument
// ─────────────────────────────────────────────────────────────────────

/// The canonical Document representation of one USD source file.
///
/// Owns the source text + a [`lunco_doc::DocumentOrigin`] (where it came from,
/// whether it can be saved) + a generation counter that bumps on every
/// successful op. Parsed-stage caching is **deferred to Phase 4** —
/// the document layer holds text only; rendering/inspection layers
/// drive the parse and cache the `TextReader` themselves.
#[derive(Debug, Clone)]
pub struct UsdDocument {
    id: DocumentId,
    source: String,
    generation: u64,
    origin: DocumentOrigin,
    /// Generation at which the document was last persisted to disk.
    /// `None` = never saved (freshly created in-memory); `Some(g)` =
    /// last saved at generation `g`. Drives `is_dirty`.
    last_saved_generation: Option<u64>,
    /// Ring buffer of `(generation_after_change, change)` for catch-up
    /// reads. See [`changes_since`](Self::changes_since).
    changes: VecDeque<(u64, UsdChange)>,
}

impl UsdDocument {
    /// Build a fresh in-memory `UsdDocument` with the given source as
    /// an Untitled document. Starts dirty (never-saved).
    pub fn new(id: DocumentId, source: impl Into<String>) -> Self {
        Self::with_origin(
            id,
            source,
            DocumentOrigin::untitled(format!("Untitled-{}.usda", id.raw())),
        )
    }

    /// Build a `UsdDocument` with an explicit origin.
    ///
    /// On-disk origins start clean (source assumed to match disk at
    /// generation 0). Untitled origins start dirty.
    pub fn with_origin(
        id: DocumentId,
        source: impl Into<String>,
        origin: DocumentOrigin,
    ) -> Self {
        let source = source.into();
        let last_saved_generation = match &origin {
            DocumentOrigin::File { .. } => Some(0),
            DocumentOrigin::Untitled { .. } | DocumentOrigin::Bundled { .. } => None,
        };
        Self {
            id,
            source,
            generation: 0,
            origin,
            last_saved_generation,
            changes: VecDeque::with_capacity(CHANGE_HISTORY_CAPACITY),
        }
    }

    /// The current source text. Canonical representation; everything
    /// else (parsed stage, prim tree, viewport entities) is derived.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Where this document came from (drives save behaviour, tab
    /// title, read-only badge).
    pub fn origin(&self) -> &DocumentOrigin {
        &self.origin
    }

    /// Replace the origin in-place. Used by Save-As to rebind an
    /// Untitled document to a fresh on-disk path; bumps the
    /// last-saved-generation marker so the dirty flag clears.
    pub fn set_origin(&mut self, origin: DocumentOrigin) {
        self.origin = origin;
        self.last_saved_generation = Some(self.generation);
    }

    /// Whether the document has unsaved changes.
    ///
    /// Untitled docs are always dirty; on-disk docs are dirty iff the
    /// current generation is past the last-saved one.
    pub fn is_dirty(&self) -> bool {
        match self.last_saved_generation {
            None => true,
            Some(g) => self.generation > g,
        }
    }

    /// Mark the current state as the last-saved baseline. Called by
    /// the Save command after a successful disk write.
    pub fn mark_saved(&mut self) {
        self.last_saved_generation = Some(self.generation);
    }

    /// Suffix of the change ring strictly after `since_generation`.
    ///
    /// Views track their last-observed generation and pull only the
    /// new tail each frame, sidestepping per-frame full rescans
    /// (`AGENTS.md` §7.1).
    pub fn changes_since(&self, since_generation: u64) -> impl Iterator<Item = (u64, &UsdChange)> {
        self.changes
            .iter()
            .filter(move |(g, _)| *g > since_generation)
            .map(|(g, c)| (*g, c))
    }

    // ─── internal ──────────────────────────────────────────────────────

    /// Core mutation path. All ops funnel through here so generation
    /// bumps, change emission, and ring trimming happen in exactly one
    /// place.
    fn apply_text_replace(
        &mut self,
        range: Range<usize>,
        replacement: String,
        change: UsdChange,
    ) -> Result<UsdOp, DocumentError> {
        if range.start > range.end || range.end > self.source.len() {
            return Err(DocumentError::ValidationFailed(format!(
                "text range {}..{} out of bounds (len={})",
                range.start,
                range.end,
                self.source.len()
            )));
        }
        if !self.source.is_char_boundary(range.start)
            || !self.source.is_char_boundary(range.end)
        {
            return Err(DocumentError::ValidationFailed(format!(
                "text range {}..{} not on char boundaries",
                range.start, range.end
            )));
        }
        // Capture the previous source for the inverse op before mutating.
        let previous = self.source.clone();
        self.source.replace_range(range, &replacement);
        self.generation += 1;
        if self.changes.len() == CHANGE_HISTORY_CAPACITY {
            self.changes.pop_front();
        }
        self.changes.push_back((self.generation, change));
        // Phase 1: every op replaces full source, so the inverse is the
        // verbatim previous source as another ReplaceSource. When
        // typed prim ops land in Phase 5 they'll compute tighter
        // inverses (e.g. `RemovePrim` ↔ `AddPrim`).
        Ok(UsdOp::ReplaceSource {
            edit_target: LayerId::root(),
            text: previous,
        })
    }
}

impl Document for UsdDocument {
    type Op = UsdOp;

    fn id(&self) -> DocumentId {
        self.id
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn apply(&mut self, op: Self::Op) -> Result<Self::Op, DocumentError> {
        // The document is the single source of truth for its own
        // mutability — every dispatch path (UI, API, MCP, scripts)
        // gets the same `ReadOnly` error and surfaces it through
        // their normal error paths. No band-aid pre-checks in panels.
        if !self.origin.accepts_mutations() {
            return Err(DocumentError::ReadOnly);
        }
        // Edit-target gate is shared across all variants: today the
        // splicers operate on the root layer only.
        let edit_target = match &op {
            UsdOp::ReplaceSource { edit_target, .. }
            | UsdOp::AddPrim { edit_target, .. }
            | UsdOp::RemovePrim { edit_target, .. }
            | UsdOp::SetTranslate { edit_target, .. }
            | UsdOp::SetAttribute { edit_target, .. } => edit_target,
        };
        if !edit_target.is_root() {
            return Err(DocumentError::ValidationFailed(format!(
                "edit target {:?} not supported (root only)",
                edit_target
            )));
        }
        match op {
            UsdOp::ReplaceSource { text, .. } => {
                let range = 0..self.source.len();
                self.apply_text_replace(range, text, UsdChange::FullReload)
            }
            UsdOp::AddPrim {
                parent_path,
                name,
                type_name,
                ..
            } => {
                let new_source = text_edit::append_child_prim(
                    &self.source,
                    &parent_path,
                    type_name.as_deref(),
                    &name,
                )
                .ok_or_else(|| {
                    DocumentError::ValidationFailed(format!(
                        "AddPrim: parent path `{}` not found",
                        parent_path
                    ))
                })?;
                let added_path = if parent_path == "/" || parent_path.is_empty() {
                    format!("/{}", name)
                } else {
                    format!("{}/{}", parent_path.trim_end_matches('/'), name)
                };
                self.replace_full_source(new_source, UsdChange::Resync { path: added_path })
            }
            UsdOp::RemovePrim { path, .. } => {
                let new_source = text_edit::remove_prim(&self.source, &path).ok_or_else(
                    || {
                        DocumentError::ValidationFailed(format!(
                            "RemovePrim: path `{}` not found",
                            path
                        ))
                    },
                )?;
                self.replace_full_source(new_source, UsdChange::Resync { path })
            }
            UsdOp::SetTranslate { path, value, .. } => {
                let new_source = text_edit::set_translate(&self.source, &path, value)
                    .ok_or_else(|| {
                        DocumentError::ValidationFailed(format!(
                            "SetTranslate: path `{}` not found",
                            path
                        ))
                    })?;
                self.replace_full_source(
                    new_source,
                    UsdChange::InfoOnly {
                        path,
                        attr: "xformOp:translate".into(),
                    },
                )
            }
            UsdOp::SetAttribute { path, name, type_name, value, .. } => {
                let new_source = text_edit::set_attribute(&self.source, &path, &name, &type_name, &value)
                    .ok_or_else(|| {
                        DocumentError::ValidationFailed(format!(
                            "SetAttribute: path `{}` not found",
                            path
                        ))
                    })?;
                self.replace_full_source(
                    new_source,
                    UsdChange::InfoOnly {
                        path,
                        attr: name,
                    },
                )
            }
        }
    }
}

impl UsdDocument {
    /// Helper used by the prim-level ops — they all reduce to
    /// "replace the full source after a splice." Bumps the
    /// generation, emits the supplied change, and records the
    /// inverse as a verbatim previous-source `ReplaceSource`.
    fn replace_full_source(
        &mut self,
        new_source: String,
        change: UsdChange,
    ) -> Result<UsdOp, DocumentError> {
        let range = 0..self.source.len();
        self.apply_text_replace(range, new_source, change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_doc::{DocumentHost, Mutation};

    const TINY_USDA: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\ndef Xform \"World\"\n{\n}\n";

    #[test]
    fn untitled_starts_dirty_and_writable() {
        let doc = UsdDocument::new(DocumentId::new(1), TINY_USDA);
        assert!(doc.is_dirty());
        assert!(doc.origin().accepts_mutations());
        assert_eq!(doc.generation(), 0);
    }

    #[test]
    fn from_file_origin_starts_clean() {
        let doc = UsdDocument::with_origin(
            DocumentId::new(2),
            TINY_USDA,
            DocumentOrigin::writable_file("/tmp/scene.usda"),
        );
        assert!(!doc.is_dirty());
    }

    #[test]
    fn readonly_origin_rejects_ops() {
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(3),
            TINY_USDA,
            DocumentOrigin::readonly_file("/tmp/scene.usda"),
        );
        let err = doc
            .apply(UsdOp::ReplaceSource {
                edit_target: LayerId::root(),
                text: "broken".to_string(),
            })
            .unwrap_err();
        assert_eq!(err, DocumentError::ReadOnly);
        assert_eq!(doc.source(), TINY_USDA);
        assert_eq!(doc.generation(), 0);
    }

    #[test]
    fn replace_source_round_trips_via_undo_redo() {
        let mut host = DocumentHost::new(UsdDocument::new(DocumentId::new(4), TINY_USDA));
        let new_text = "#usda 1.0\ndef Xform \"Other\" {}\n";
        host.apply(Mutation::local(UsdOp::ReplaceSource {
            edit_target: LayerId::root(),
            text: new_text.to_string(),
        }))
        .unwrap();
        assert_eq!(host.document().source(), new_text);
        assert_eq!(host.generation(), 1);

        host.undo().unwrap();
        assert_eq!(host.document().source(), TINY_USDA);
        // Generation is monotonic: undo bumps it too.
        assert_eq!(host.generation(), 2);

        host.redo().unwrap();
        assert_eq!(host.document().source(), new_text);
        assert_eq!(host.generation(), 3);
    }

    #[test]
    fn mark_saved_clears_dirty() {
        let mut doc = UsdDocument::new(DocumentId::new(5), TINY_USDA);
        assert!(doc.is_dirty());
        doc.mark_saved();
        assert!(!doc.is_dirty());
        let _ = doc.apply(UsdOp::ReplaceSource {
            edit_target: LayerId::root(),
            text: "changed".to_string(),
        });
        assert!(doc.is_dirty());
    }

    #[test]
    fn changes_since_returns_only_new_tail() {
        let mut doc = UsdDocument::new(DocumentId::new(6), TINY_USDA);
        let _ = doc.apply(UsdOp::ReplaceSource {
            edit_target: LayerId::root(),
            text: "a".to_string(),
        });
        let after_first = doc.generation();
        let _ = doc.apply(UsdOp::ReplaceSource {
            edit_target: LayerId::root(),
            text: "b".to_string(),
        });
        let tail: Vec<_> = doc.changes_since(after_first).collect();
        assert_eq!(tail.len(), 1);
        assert!(matches!(tail[0].1, UsdChange::FullReload));
    }

    #[test]
    fn non_root_edit_target_is_rejected() {
        let mut doc = UsdDocument::new(DocumentId::new(7), TINY_USDA);
        let err = doc
            .apply(UsdOp::ReplaceSource {
                edit_target: LayerId::new("sub.usda"),
                text: "x".to_string(),
            })
            .unwrap_err();
        assert!(matches!(err, DocumentError::ValidationFailed(_)));
        assert_eq!(doc.generation(), 0);
    }

    #[test]
    fn add_prim_appends_at_root_and_undoes() {
        let mut host =
            DocumentHost::new(UsdDocument::new(DocumentId::new(8), TINY_USDA));
        host.apply(Mutation::local(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/".into(),
            name: "Rover".into(),
            type_name: Some("Xform".into()),
        }))
        .unwrap();
        assert!(host.document().source().contains("def Xform \"Rover\""));
        host.undo().unwrap();
        assert_eq!(host.document().source(), TINY_USDA);
    }

    #[test]
    fn add_prim_unknown_parent_validation_error() {
        let mut doc = UsdDocument::new(DocumentId::new(9), TINY_USDA);
        let err = doc
            .apply(UsdOp::AddPrim {
                edit_target: LayerId::root(),
                parent_path: "/Nope".into(),
                name: "Body".into(),
                type_name: Some("Cube".into()),
            })
            .unwrap_err();
        assert!(matches!(err, DocumentError::ValidationFailed(_)));
        assert_eq!(doc.generation(), 0);
    }

    #[test]
    fn rover_built_from_blank_round_trips_with_undo() {
        let blank = "#usda 1.0\n";
        let mut host = DocumentHost::new(UsdDocument::new(DocumentId::new(10), blank));

        host.apply(Mutation::local(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/".into(),
            name: "Rover".into(),
            type_name: Some("Xform".into()),
        }))
        .unwrap();
        host.apply(Mutation::local(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/Rover".into(),
            name: "WheelFL".into(),
            type_name: Some("Cube".into()),
        }))
        .unwrap();
        host.apply(Mutation::local(UsdOp::SetTranslate {
            edit_target: LayerId::root(),
            path: "/Rover/WheelFL".into(),
            value: [1.0, 0.0, 1.0],
        }))
        .unwrap();

        let final_src = host.document().source().to_string();
        assert!(final_src.contains("def Xform \"Rover\""));
        assert!(final_src.contains("def Cube \"WheelFL\""));
        assert!(final_src.contains("xformOp:translate = (1, 0, 1)"));

        // Undo every step → back to blank.
        host.undo().unwrap();
        host.undo().unwrap();
        host.undo().unwrap();
        assert_eq!(host.document().source(), blank);
    }

    #[test]
    fn remove_prim_drops_block_and_undoes() {
        let with_ball = "#usda 1.0\ndef Xform \"World\"\n{\n    def Sphere \"Ball\"\n    {\n    }\n}\n";
        let mut host = DocumentHost::new(UsdDocument::with_origin(
            DocumentId::new(11),
            with_ball,
            DocumentOrigin::writable_file("/tmp/x.usda"),
        ));
        host.apply(Mutation::local(UsdOp::RemovePrim {
            edit_target: LayerId::root(),
            path: "/World/Ball".into(),
        }))
        .unwrap();
        assert!(!host.document().source().contains("Ball"));
        host.undo().unwrap();
        assert!(host.document().source().contains("def Sphere \"Ball\""));
    }
}
