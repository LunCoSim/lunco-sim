//! `UsdDocument` — the canonical Document representation of one USD
//! source file (`.usda` for now; `.usdc` deferred).
//!
//! ## Why data-canonical (Phase C2/C3)
//!
//! Earlier phases treated the `.usda` **source text** as canonical and
//! mutated it by splicing byte ranges ([`crate::text_edit`], now deleted).
//! That is the CQ-503 nested-child corruption class: editing
//! `/World/Box.radius` could clobber `/World/Box/Inner.radius` because the
//! splicer reasoned about text, not structure.
//!
//! The document now holds an [`sdf::Data`] — the **root layer's authored
//! specs** — as its canonical representation. This is *not* the flattened
//! composition: references, payloads, and sublayer opinions survive verbatim,
//! so the document still round-trips losslessly with external USD tools
//! (Omniverse, USDView, Blender). Edits route through openusd's authoring
//! engine: [`lunco_usd_bevy::author`] opens the data as a transient `Stage`,
//! authors the op **by SDF path** (which cannot touch a sibling/nested prim
//! that shares a name), and extracts the updated root layer back out.
//!
//! The serialized `.usda` text is produced on demand ([`UsdDocument::source`])
//! for saving to disk, the viewport preview, and session snapshots.
//!
//! ## Edit target
//!
//! Per the Omniverse pattern, every [`UsdOp`] carries an `edit_target:
//! LayerId` so future composition-aware editing can name *which layer*
//! receives an opinion. Today the document is a single root layer and every
//! op targets [`LayerId::root`]; non-root targets are rejected until
//! genuine multi-layer routing lands (Phase C4).

use std::collections::VecDeque;

use bevy::log::warn;
use bevy::reflect::Reflect;
use lunco_doc::{Document, DocumentError, DocumentId, DocumentOp, DocumentOrigin};
use lunco_usd_bevy::author::{
    self, extract_root_layer_data, open_doc_stage, parse_attribute_value, usda_to_data,
};
use lunco_usd_bevy::usd_data::UsdDataExt;
use openusd::sdf::{self, Path as SdfPath, SpecType};

/// How many recent changes to keep in the per-document ring buffer.
///
/// Views consume the suffix via [`UsdDocument::changes_since`]; 256 is
/// generous for realistic edit cadences without growing unbounded.
const CHANGE_HISTORY_CAPACITY: usize = 256;

/// Minimal valid USDA, used as the canonical-data fallback when a document's
/// source text fails to parse (see [`UsdDocument::with_origin`]).
const EMPTY_USDA: &str = "#usda 1.0\n";

// ─────────────────────────────────────────────────────────────────────
// LayerId — names a layer in a stage's layer stack
// ─────────────────────────────────────────────────────────────────────

/// Identifies one layer in a [`UsdDocument`]'s layer stack.
///
/// A document has two layers (Phase C4):
/// - [`LayerId::root`] — the **base** layer: the authored scene, serialized to
///   disk on Save.
/// - [`LayerId::runtime`] — the **runtime** layer: generated, ephemeral state
///   (obstacle fields, spawn transforms) that overlays the base for reads but
///   is **not** written to the authored file.
///
/// An op's `edit_target` names which layer receives the opinion; unknown
/// identifiers are rejected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Reflect, serde::Serialize, serde::Deserialize)]
pub struct LayerId(String);

impl LayerId {
    /// The base/root layer — the authored scene, saved to disk.
    pub fn root() -> Self {
        Self("@root@".to_string())
    }

    /// The runtime layer — generated, non-persisted overlay state.
    pub fn runtime() -> Self {
        Self("@runtime@".to_string())
    }

    /// Wrap an arbitrary layer identifier (path or anonymous handle).
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The raw identifier string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when this id refers to the document's base/root layer.
    pub fn is_root(&self) -> bool {
        self.0 == "@root@"
    }

    /// True when this id refers to the document's runtime layer.
    pub fn is_runtime(&self) -> bool {
        self.0 == "@runtime@"
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
/// Forward application routes through [`lunco_usd_bevy::author`] — the op is
/// authored by SDF path into a transient `Stage` and the updated root layer
/// is extracted back as [`sdf::Data`]. Inverses are typed where it is cheap
/// and exact (`AddPrim` ↔ `RemovePrim`) and fall back to a full-source
/// [`UsdOp::ReplaceSource`] snapshot otherwise — always correct.
#[derive(Debug, Clone, Reflect, serde::Serialize, serde::Deserialize)]
pub enum UsdOp {
    /// Replace the entire source buffer with `text`. Inverse is the
    /// previous source as another `ReplaceSource`. Used as the
    /// universal inverse fallback for the other variants.
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
        /// Prim name — must be a valid USD identifier.
        name: String,
        /// Optional schema type (`Xform`, `Cube`, `Mesh`, …).
        type_name: Option<String>,
        /// Optional asset reference (`@vessels/rover.usda@`, bare path, no `@`).
        /// `Some` authors a `references` arc so the prim instances that asset —
        /// this is how a runtime spawn persists (the referenced content + a
        /// local `xformOp` override compose into the rendered prim).
        reference: Option<String>,
    },
    /// Remove the prim at `path` together with its entire subtree. The
    /// inverse re-establishes the prior full source.
    RemovePrim {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim to remove.
        path: String,
    },
    /// Set the `xformOp:translate` attribute on the prim at `path`.
    /// Authors `xformOpOrder` too if the prim has none yet.
    SetTranslate {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose translate to set.
        path: String,
        /// `[x, y, z]` in stage units.
        value: [f64; 3],
    },
    /// Set an arbitrary attribute on the prim at `path`. Creates the
    /// attribute if absent, replaces its value otherwise.
    SetAttribute {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose attribute to set.
        path: String,
        /// The name of the attribute (e.g. `primvars:displayColor` or `inputs:roughness`).
        name: String,
        /// The USD type name of the attribute (e.g. `color3f` or `float`).
        type_name: String,
        /// The attribute value formatted as a USD-compliant string literal
        /// (e.g. `"(0.2, 0.2, 0.8)"`, `"0.5"`). Parsed into a typed
        /// [`sdf::Value`] by openusd's own parser at apply time.
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

/// Participation in the canonical Twin journal ([`lunco_twin_journal`]).
///
/// `UsdOp` derives `Serialize`, so the journal records the **real op**
/// (lossless) via `record_op` — no hand-written summary. `referenced_entities`
/// stays the default empty set: every variant knows the prim path it touches,
/// but an [`EntityRef`](lunco_twin_journal::EntityRef) also needs the owning
/// `DocumentId`, which the op alone doesn't carry. That enrichment lands with
/// the multi-user replication path.
impl lunco_twin_journal::OpPayload for UsdOp {
    fn domain(&self) -> lunco_twin_journal::DomainKind {
        lunco_twin_journal::DomainKind::Usd
    }
}

// ─────────────────────────────────────────────────────────────────────
// UsdDocument
// ─────────────────────────────────────────────────────────────────────

/// The canonical Document representation of one USD source file.
///
/// Owns the root layer's authored [`sdf::Data`] + a
/// [`lunco_doc::DocumentOrigin`] (where it came from, whether it can be saved)
/// + a generation counter that bumps on every successful op. The flattened,
/// composed scene (references resolved) is a *separate* derived artifact built
/// by the asset loader ([`lunco_usd_bevy::UsdStageAsset`]); the document layer
/// never holds it.
#[derive(Debug, Clone)]
pub struct UsdDocument {
    id: DocumentId,
    /// The **base** layer: the authored scene's specs (references intact). This
    /// is the canonical content [`source`](Self::source) serializes and Save
    /// writes to disk. Root-targeted ops edit this layer.
    base: sdf::Data,
    /// The **runtime** layer: generated, ephemeral overlay state authored by
    /// runtime-targeted ops (obstacle fields, spawn transforms). Kept separate
    /// so it never reaches the saved file. Starts empty; folding it into reads
    /// is deferred until a producer needs it (see [`runtime_data`](Self::runtime_data)).
    runtime: sdf::Data,
    /// Set only when the base source text failed to parse on construction:
    /// holds the verbatim source so [`source`](Self::source) and Save preserve
    /// the file rather than silently emptying it. While `Some`, structural ops
    /// are rejected; a base [`UsdOp::ReplaceSource`] clears it.
    parse_error: Option<String>,
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
    /// Build a fresh in-memory `UsdDocument` from USDA source as an Untitled
    /// document. Starts dirty (never-saved).
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
    /// generation 0). Untitled origins start dirty. If the source text doesn't
    /// parse as USDA the document still opens — the raw text is preserved (see
    /// [`parse_error`](Self::parse_error)) — but structural edits are blocked
    /// until a [`UsdOp::ReplaceSource`] supplies valid source.
    pub fn with_origin(id: DocumentId, source: impl Into<String>, origin: DocumentOrigin) -> Self {
        let source = source.into();
        let (base, parse_error) = match usda_to_data(&source) {
            Ok(data) => (data, None),
            Err(e) => {
                warn!(
                    "[usd] document {} source did not parse as USDA ({e}); \
                     keeping raw text, edits disabled until replaced",
                    id.raw()
                );
                (usda_to_data(EMPTY_USDA).unwrap_or_default(), Some(source))
            }
        };
        let last_saved_generation = match &origin {
            DocumentOrigin::File { .. } => Some(0),
            DocumentOrigin::Untitled { .. } | DocumentOrigin::Bundled { .. } => None,
        };
        Self {
            id,
            base,
            runtime: usda_to_data(EMPTY_USDA).unwrap_or_default(),
            parse_error,
            generation: 0,
            origin,
            last_saved_generation,
            changes: VecDeque::with_capacity(CHANGE_HISTORY_CAPACITY),
        }
    }

    /// The current source text, serialized from the **base** layer on demand.
    /// This is what Save writes to disk and what the viewport preview / session
    /// snapshot consume. The runtime overlay is deliberately excluded — sim
    /// state must never reach the authored file. Round-trips losslessly with
    /// [`new`](Self::new) (references and structure survive); only formatting is
    /// normalized.
    ///
    /// If the document was opened from un-parseable source, the verbatim
    /// original text is returned instead so the file is never corrupted.
    pub fn source(&self) -> String {
        if let Some(raw) = &self.parse_error {
            return raw.clone();
        }
        author::data_to_usda(&self.base).unwrap_or_else(|e| {
            warn!("[usd] failed to serialize document {}: {e}", self.id.raw());
            EMPTY_USDA.to_string()
        })
    }

    /// The authored **base** layer data (references intact). Query it with the
    /// [`UsdDataExt`](lunco_usd_bevy::usd_data::UsdDataExt) helpers. The runtime
    /// overlay is not folded in here — read it separately via
    /// [`runtime_data`](Self::runtime_data) until a consumer needs a composed
    /// view (deferred with the runtime-producer wiring).
    pub fn data(&self) -> &sdf::Data {
        &self.base
    }

    /// The **runtime** layer's overlay data — generated state authored by
    /// runtime-targeted ops, never persisted to the base file. Empty until a
    /// runtime op lands.
    pub fn runtime_data(&self) -> &sdf::Data {
        &self.runtime
    }

    /// The **composed** view: the runtime overlay merged over the base layer
    /// (runtime opinions win, runtime-only prims included). This is what the
    /// viewport renders — base authored content plus generated runtime state —
    /// whereas [`source`](Self::source) (Save) stays base-only. References
    /// survive as opinions; this is an sdf layer-stack merge, not render-time
    /// PCP composition.
    pub fn composed(&self) -> sdf::Data {
        author::compose_layers(&self.base, &self.runtime)
    }

    /// The composed view serialized to USDA text — the source the viewport
    /// re-parses so runtime-layer state becomes visible. Falls back to the raw
    /// (base) source when the base is un-parseable.
    pub fn composed_source(&self) -> String {
        if let Some(raw) = &self.parse_error {
            return raw.clone();
        }
        author::data_to_usda(&self.composed()).unwrap_or_else(|e| {
            warn!(
                "[usd] failed to serialize composed document {}: {e}",
                self.id.raw()
            );
            EMPTY_USDA.to_string()
        })
    }

    /// Replace the entire **runtime** layer with `data` — a session-restore
    /// load (the persisted `.lunco` runtime overlay), NOT an edit. Bumps the
    /// generation and records a [`UsdChange::FullReload`] so the viewport
    /// rebuilds, but routes through neither the op layer nor the journal: it
    /// *reconstructs* runtime state that was authored (and journaled) in a prior
    /// session, rather than authoring it anew. Preserves the base dirty flag, so
    /// reloading runtime state never makes a clean scene look unsaved.
    pub fn restore_runtime(&mut self, data: sdf::Data) {
        let was_dirty = self.is_dirty();
        self.commit(TargetLayer::Runtime, data, UsdChange::FullReload);
        if !was_dirty {
            self.last_saved_generation = Some(self.generation);
        }
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
    pub fn changes_since(&self, since_generation: u64) -> impl Iterator<Item = (u64, &UsdChange)> {
        self.changes
            .iter()
            .filter(move |(g, _)| *g > since_generation)
            .map(|(g, c)| (*g, c))
    }

    // ─── internal ──────────────────────────────────────────────────────

    /// Borrow the data for layer `t`.
    fn layer(&self, t: TargetLayer) -> &sdf::Data {
        match t {
            TargetLayer::Base => &self.base,
            TargetLayer::Runtime => &self.runtime,
        }
    }

    /// The current serialized source of layer `t` (base honors the
    /// un-parseable raw-text fallback; runtime is always real data).
    fn layer_source(&self, t: TargetLayer) -> String {
        match t {
            TargetLayer::Base => self.source(),
            TargetLayer::Runtime => author::data_to_usda(&self.runtime).unwrap_or_else(|e| {
                warn!("[usd] failed to serialize runtime layer {}: {e}", self.id.raw());
                EMPTY_USDA.to_string()
            }),
        }
    }

    /// Commit a freshly authored [`sdf::Data`] into layer `t`: swap it in, bump
    /// the generation, and record the change in the ring. The single place a
    /// successful op mutates state.
    fn commit(&mut self, t: TargetLayer, data: sdf::Data, change: UsdChange) {
        match t {
            TargetLayer::Base => self.base = data,
            TargetLayer::Runtime => self.runtime = data,
        }
        self.generation += 1;
        if self.changes.len() == CHANGE_HISTORY_CAPACITY {
            self.changes.pop_front();
        }
        self.changes.push_back((self.generation, change));
    }

    /// The always-correct coarse inverse: restore layer `t`'s current
    /// (pre-mutation) source verbatim via a `ReplaceSource` **targeting the
    /// same layer**, so undo routes back to the layer the forward op touched.
    /// Capture it *before* authoring the forward op.
    fn coarse_inverse(&self, t: TargetLayer, id: &LayerId) -> UsdOp {
        UsdOp::ReplaceSource {
            edit_target: id.clone(),
            text: self.layer_source(t),
        }
    }

    /// Validate that `path` names a prim present in **either** layer (base or
    /// runtime) — a runtime op may add a child or override an attribute under a
    /// base-authored prim. Returns the parsed [`SdfPath`].
    fn require_prim_anywhere(&self, path: &str) -> Result<SdfPath, DocumentError> {
        let sdf = parse_prim_path(path)?;
        if prim_in(&self.base, &sdf) || prim_in(&self.runtime, &sdf) {
            Ok(sdf)
        } else {
            Err(DocumentError::ValidationFailed(format!(
                "path `{path}` not found"
            )))
        }
    }

    /// Validate that `path` names a prim authored in **this specific layer** —
    /// you can only remove from a layer what that layer holds.
    fn require_prim_in(&self, t: TargetLayer, path: &str) -> Result<SdfPath, DocumentError> {
        let sdf = parse_prim_path(path)?;
        if prim_in(self.layer(t), &sdf) {
            Ok(sdf)
        } else {
            Err(DocumentError::ValidationFailed(format!(
                "path `{path}` not found in target layer"
            )))
        }
    }
}

/// Which of a document's two layers an op edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetLayer {
    /// The authored base layer (saved to disk).
    Base,
    /// The generated runtime overlay (not saved).
    Runtime,
}

impl TargetLayer {
    /// Resolve a [`LayerId`] to a concrete layer, or `None` for an unknown
    /// identifier.
    fn from_id(id: &LayerId) -> Option<Self> {
        if id.is_root() {
            Some(Self::Base)
        } else if id.is_runtime() {
            Some(Self::Runtime)
        } else {
            None
        }
    }
}

/// Parse a USD prim path string, mapping errors to a validation failure.
fn parse_prim_path(path: &str) -> Result<SdfPath, DocumentError> {
    SdfPath::new(path)
        .map_err(|e| DocumentError::ValidationFailed(format!("invalid prim path `{path}`: {e}")))
}

/// True when `data` holds a prim spec at `sdf`.
fn prim_in(data: &sdf::Data, sdf: &SdfPath) -> bool {
    matches!(data.spec(sdf), Some(s) if s.ty == SpecType::Prim)
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
        // The document is the single source of truth for its own mutability —
        // every dispatch path (UI, API, MCP, scripts) gets the same `ReadOnly`
        // error and surfaces it through their normal error paths.
        if !self.origin.accepts_mutations() {
            return Err(DocumentError::ReadOnly);
        }
        // Resolve the edit target to a concrete layer (base or runtime).
        // Unknown identifiers are rejected — no silent misrouting to root.
        let id = match &op {
            UsdOp::ReplaceSource { edit_target, .. }
            | UsdOp::AddPrim { edit_target, .. }
            | UsdOp::RemovePrim { edit_target, .. }
            | UsdOp::SetTranslate { edit_target, .. }
            | UsdOp::SetAttribute { edit_target, .. } => edit_target.clone(),
        };
        let target = TargetLayer::from_id(&id).ok_or_else(|| {
            DocumentError::ValidationFailed(format!(
                "edit target {id:?} not a known layer (root | runtime)"
            ))
        })?;
        // A document opened from un-parseable base source can only be repaired
        // wholesale; structural ops have no valid base to validate against.
        if self.parse_error.is_some() && !matches!(op, UsdOp::ReplaceSource { .. }) {
            return Err(DocumentError::ValidationFailed(
                "document source is un-parseable; replace it before editing".into(),
            ));
        }

        match op {
            UsdOp::ReplaceSource { text, .. } => {
                let new_data = usda_to_data(&text).map_err(|e| {
                    DocumentError::ValidationFailed(format!("ReplaceSource: {e}"))
                })?;
                let inverse = self.coarse_inverse(target, &id);
                // Replacing the base layer repairs an un-parseable document.
                if target == TargetLayer::Base {
                    self.parse_error = None;
                }
                self.commit(target, new_data, UsdChange::FullReload);
                Ok(inverse)
            }

            UsdOp::AddPrim {
                parent_path,
                name,
                type_name,
                reference,
                ..
            } => {
                // Parent must exist in either layer (root is implicit).
                if parent_path != "/" && !parent_path.is_empty() {
                    self.require_prim_anywhere(&parent_path)?;
                }
                let prim_path = if parent_path == "/" || parent_path.is_empty() {
                    format!("/{name}")
                } else {
                    format!("{}/{name}", parent_path.trim_end_matches('/'))
                };
                let prim_sdf = parse_prim_path(&prim_path)?;
                // "Already authored" is judged against the TARGET layer — that
                // is what the inverse will or won't be able to cleanly remove.
                let existed = self.layer(target).spec(&prim_sdf).is_some();

                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                let prim = stage.define_prim(prim_path.as_str()).map_err(author_err)?;
                if let Some(tn) = &type_name {
                    prim.set_type_name(tn.as_str()).map_err(author_err)?;
                }
                let mut new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                // Author the asset reference (Stage has no `add_reference`, so this
                // is set at the sdf level) — turns the prim into a runtime spawn.
                if let Some(asset_path) = &reference {
                    lunco_usd_bevy::author::author_reference(&mut new_data, &prim_sdf, asset_path)
                        .map_err(author_err)?;
                }

                // A brand-new prim in this layer is exactly undone by removing
                // it (from the same layer); otherwise fall back to the snapshot.
                let inverse = if existed {
                    self.coarse_inverse(target, &id)
                } else {
                    UsdOp::RemovePrim {
                        edit_target: id.clone(),
                        path: prim_path.clone(),
                    }
                };
                self.commit(target, new_data, UsdChange::Resync { path: prim_path });
                Ok(inverse)
            }

            UsdOp::RemovePrim { path, .. } => {
                // Can only remove what the target layer itself authored.
                self.require_prim_in(target, &path)?;
                let inverse = self.coarse_inverse(target, &id);
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage.remove_prim(path.as_str()).map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::Resync { path });
                Ok(inverse)
            }

            UsdOp::SetTranslate { path, value, .. } => {
                let prim_sdf = self.require_prim_anywhere(&path)?;
                // Pre-state is read from the TARGET layer: the inverse restores
                // that layer's opinion, and `xformOpOrder` we synthesize lands
                // there too.
                let layer = self.layer(target);
                let translate_existed = prim_sdf
                    .append_property("xformOp:translate")
                    .ok()
                    .and_then(|p| layer.spec(&p).map(|_| ()))
                    .is_some();
                let order_existed = prim_sdf
                    .append_property("xformOpOrder")
                    .ok()
                    .and_then(|p| layer.spec(&p).map(|_| ()))
                    .is_some();
                let old_translate = layer.prim_attribute_value::<[f64; 3]>(&prim_sdf, "xformOp:translate");

                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage
                    .create_attribute(format!("{path}.xformOp:translate"), "double3")
                    .map_err(author_err)?
                    .set(value)
                    .map_err(author_err)?;
                // Establish the op order only when the prim has none yet in this
                // layer — never clobber an existing xform stack.
                if !order_existed {
                    let order = parse_attribute_value("token[]", "[\"xformOp:translate\"]")
                        .map_err(author_err)?;
                    stage
                        .create_attribute(format!("{path}.xformOpOrder"), "token[]")
                        .map_err(author_err)?
                        .set(order)
                        .map_err(author_err)?;
                }
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;

                // Typed inverse only when this purely overwrote an existing
                // translate in this layer (no `xformOpOrder` was synthesized).
                let inverse = if translate_existed && order_existed {
                    old_translate
                        .map(|old| UsdOp::SetTranslate {
                            edit_target: id.clone(),
                            path: path.clone(),
                            value: old,
                        })
                        .unwrap_or_else(|| self.coarse_inverse(target, &id))
                } else {
                    self.coarse_inverse(target, &id)
                };
                self.commit(
                    target,
                    new_data,
                    UsdChange::InfoOnly {
                        path,
                        attr: "xformOp:translate".into(),
                    },
                );
                Ok(inverse)
            }

            UsdOp::SetAttribute {
                path,
                name,
                type_name,
                value,
                ..
            } => {
                self.require_prim_anywhere(&path)?;
                let val = parse_attribute_value(&type_name, &value).map_err(|e| {
                    DocumentError::ValidationFailed(format!(
                        "SetAttribute `{name}` ({type_name}): {e}"
                    ))
                })?;
                let inverse = self.coarse_inverse(target, &id);
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage
                    .create_attribute(format!("{path}.{name}"), type_name.as_str())
                    .map_err(author_err)?
                    .set(val)
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::InfoOnly { path, attr: name });
                Ok(inverse)
            }
        }
    }
}

/// Map an authoring error (`anyhow`/openusd `StageAuthoringError`) to a
/// document validation failure.
fn author_err<E: std::fmt::Display>(e: E) -> DocumentError {
    DocumentError::ValidationFailed(format!("authoring failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_doc::{DocumentHost, Mutation};

    const TINY_USDA: &str =
        "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\ndef Xform \"World\"\n{\n}\n";

    fn prim_type(doc: &UsdDocument, path: &str) -> Option<String> {
        doc.data().prim_type_name(&SdfPath::new(path).unwrap())
    }
    fn prim_exists(doc: &UsdDocument, path: &str) -> bool {
        doc.data().spec(&SdfPath::new(path).unwrap()).is_some()
    }

    #[test]
    fn untitled_starts_dirty_and_writable() {
        let doc = UsdDocument::new(DocumentId::new(1), TINY_USDA);
        assert!(doc.is_dirty());
        assert!(doc.origin().accepts_mutations());
        assert_eq!(doc.generation(), 0);
        // Source serializes from canonical data and preserves structure.
        assert!(doc.source().contains("def Xform \"World\""));
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
                text: "#usda 1.0\n".to_string(),
            })
            .unwrap_err();
        assert_eq!(err, DocumentError::ReadOnly);
        assert_eq!(doc.generation(), 0);
    }

    #[test]
    fn replace_source_round_trips_via_undo_redo() {
        let mut host = DocumentHost::new(UsdDocument::new(DocumentId::new(4), TINY_USDA));
        let new_text = "#usda 1.0\ndef Xform \"Other\"\n{\n}\n";
        host.apply(Mutation::local(UsdOp::ReplaceSource {
            edit_target: LayerId::root(),
            text: new_text.to_string(),
        }))
        .unwrap();
        assert!(prim_exists(host.document(), "/Other"));
        assert!(!prim_exists(host.document(), "/World"));
        assert_eq!(host.generation(), 1);

        host.undo().unwrap();
        assert!(prim_exists(host.document(), "/World"));
        assert!(!prim_exists(host.document(), "/Other"));
        assert_eq!(host.generation(), 2);

        host.redo().unwrap();
        assert!(prim_exists(host.document(), "/Other"));
        assert_eq!(host.generation(), 3);
    }

    #[test]
    fn mark_saved_clears_dirty() {
        let mut doc = UsdDocument::new(DocumentId::new(5), TINY_USDA);
        assert!(doc.is_dirty());
        doc.mark_saved();
        assert!(!doc.is_dirty());
        doc.apply(UsdOp::ReplaceSource {
            edit_target: LayerId::root(),
            text: "#usda 1.0\n".to_string(),
        })
        .unwrap();
        assert!(doc.is_dirty());
    }

    #[test]
    fn changes_since_returns_only_new_tail() {
        let mut doc = UsdDocument::new(DocumentId::new(6), TINY_USDA);
        doc.apply(UsdOp::ReplaceSource {
            edit_target: LayerId::root(),
            text: "#usda 1.0\n".to_string(),
        })
        .unwrap();
        let after_first = doc.generation();
        doc.apply(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/".into(),
            name: "Thing".into(),
            type_name: Some("Xform".into()),
            reference: None,
        })
        .unwrap();
        let tail: Vec<_> = doc.changes_since(after_first).collect();
        assert_eq!(tail.len(), 1);
        assert!(matches!(tail[0].1, UsdChange::Resync { .. }));
    }

    #[test]
    fn unknown_edit_target_is_rejected() {
        let mut doc = UsdDocument::new(DocumentId::new(7), TINY_USDA);
        let err = doc
            .apply(UsdOp::ReplaceSource {
                edit_target: LayerId::new("sub.usda"),
                text: "#usda 1.0\n".to_string(),
            })
            .unwrap_err();
        assert!(matches!(err, DocumentError::ValidationFailed(_)));
        assert_eq!(doc.generation(), 0);
    }

    #[test]
    fn add_prim_appends_at_root_and_undoes() {
        let mut host = DocumentHost::new(UsdDocument::new(DocumentId::new(8), TINY_USDA));
        host.apply(Mutation::local(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/".into(),
            name: "Rover".into(),
            type_name: Some("Xform".into()),
            reference: None,
        }))
        .unwrap();
        assert_eq!(prim_type(host.document(), "/Rover").as_deref(), Some("Xform"));
        // Typed inverse: AddPrim → RemovePrim removes exactly the new prim.
        host.undo().unwrap();
        assert!(!prim_exists(host.document(), "/Rover"));
        assert!(prim_exists(host.document(), "/World"));
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
                reference: None,
            })
            .unwrap_err();
        assert!(matches!(err, DocumentError::ValidationFailed(_)));
        assert_eq!(doc.generation(), 0);
    }

    #[test]
    fn rover_built_from_blank_round_trips_with_undo() {
        let mut host = DocumentHost::new(UsdDocument::new(DocumentId::new(10), EMPTY_USDA));

        host.apply(Mutation::local(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/".into(),
            name: "Rover".into(),
            type_name: Some("Xform".into()),
            reference: None,
        }))
        .unwrap();
        host.apply(Mutation::local(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/Rover".into(),
            name: "WheelFL".into(),
            type_name: Some("Cube".into()),
            reference: None,
        }))
        .unwrap();
        host.apply(Mutation::local(UsdOp::SetTranslate {
            edit_target: LayerId::root(),
            path: "/Rover/WheelFL".into(),
            value: [1.0, 0.0, 1.0],
        }))
        .unwrap();

        let doc = host.document();
        assert_eq!(prim_type(doc, "/Rover").as_deref(), Some("Xform"));
        assert_eq!(prim_type(doc, "/Rover/WheelFL").as_deref(), Some("Cube"));
        assert_eq!(
            doc.data()
                .prim_attribute_value::<[f64; 3]>(&SdfPath::new("/Rover/WheelFL").unwrap(), "xformOp:translate"),
            Some([1.0, 0.0, 1.0])
        );

        // Undo every step → back to blank (no prims).
        host.undo().unwrap();
        host.undo().unwrap();
        host.undo().unwrap();
        assert!(!prim_exists(host.document(), "/Rover"));
        assert!(!prim_exists(host.document(), "/Rover/WheelFL"));
    }

    #[test]
    fn set_translate_does_not_clobber_nested_child_translate() {
        // CQ-503: nested prims with the same attribute. Editing the parent's
        // translate must leave the child's translate untouched.
        let nested = "#usda 1.0\ndef Xform \"A\"\n{\n    double3 xformOp:translate = (5, 5, 5)\n    uniform token[] xformOpOrder = [\"xformOp:translate\"]\n    def Xform \"B\"\n    {\n        double3 xformOp:translate = (9, 9, 9)\n        uniform token[] xformOpOrder = [\"xformOp:translate\"]\n    }\n}\n";
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(20),
            nested,
            DocumentOrigin::writable_file("/tmp/n.usda"),
        );
        doc.apply(UsdOp::SetTranslate {
            edit_target: LayerId::root(),
            path: "/A".into(),
            value: [1.0, 2.0, 3.0],
        })
        .unwrap();
        assert_eq!(
            doc.data()
                .prim_attribute_value::<[f64; 3]>(&SdfPath::new("/A").unwrap(), "xformOp:translate"),
            Some([1.0, 2.0, 3.0])
        );
        assert_eq!(
            doc.data()
                .prim_attribute_value::<[f64; 3]>(&SdfPath::new("/A/B").unwrap(), "xformOp:translate"),
            Some([9.0, 9.0, 9.0]),
            "nested child translate must be untouched (CQ-503)"
        );
    }

    #[test]
    fn remove_prim_drops_block_and_undoes() {
        let with_ball =
            "#usda 1.0\ndef Xform \"World\"\n{\n    def Sphere \"Ball\"\n    {\n    }\n}\n";
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
        assert!(!prim_exists(host.document(), "/World/Ball"));
        host.undo().unwrap();
        assert_eq!(
            prim_type(host.document(), "/World/Ball").as_deref(),
            Some("Sphere")
        );
    }

    #[test]
    fn set_attribute_creates_and_records_typed_value() {
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(12),
            "#usda 1.0\ndef Sphere \"Ball\"\n{\n}\n",
            DocumentOrigin::writable_file("/tmp/a.usda"),
        );
        doc.apply(UsdOp::SetAttribute {
            edit_target: LayerId::root(),
            path: "/Ball".into(),
            name: "primvars:displayColor".into(),
            type_name: "color3f".into(),
            value: "(0.2, 0.4, 0.8)".into(),
        })
        .unwrap();
        let color = doc
            .data()
            .prim_attribute_value::<[f32; 3]>(&SdfPath::new("/Ball").unwrap(), "primvars:displayColor");
        assert_eq!(color, Some([0.2, 0.4, 0.8]));
    }

    #[test]
    fn unparseable_source_preserved_and_edits_blocked() {
        let garbage = "this is not valid usda {{{";
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(13),
            garbage,
            DocumentOrigin::writable_file("/tmp/bad.usda"),
        );
        // Raw text preserved for save.
        assert_eq!(doc.source(), garbage);
        // Structural edits blocked.
        let err = doc
            .apply(UsdOp::AddPrim {
                edit_target: LayerId::root(),
                parent_path: "/".into(),
                name: "X".into(),
                type_name: Some("Xform".into()),
                reference: None,
            })
            .unwrap_err();
        assert!(matches!(err, DocumentError::ValidationFailed(_)));
        // ReplaceSource repairs it.
        doc.apply(UsdOp::ReplaceSource {
            edit_target: LayerId::root(),
            text: TINY_USDA.to_string(),
        })
        .unwrap();
        assert!(prim_exists(&doc, "/World"));
    }

    // ─── C4: runtime layer ──────────────────────────────────────────────

    fn runtime_prim_exists(doc: &UsdDocument, path: &str) -> bool {
        doc.runtime_data()
            .spec(&SdfPath::new(path).unwrap())
            .is_some()
    }

    #[test]
    fn runtime_op_lands_in_runtime_layer_and_leaves_base_untouched() {
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(30),
            TINY_USDA,
            DocumentOrigin::writable_file("/tmp/r.usda"),
        );
        // Add a child under the base-authored /World, targeting the runtime layer.
        doc.apply(UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/World".into(),
            name: "Obstacle".into(),
            type_name: Some("Sphere".into()),
            reference: None,
        })
        .unwrap();

        // Prim is in the runtime layer...
        assert!(runtime_prim_exists(&doc, "/World/Obstacle"));
        // ...and NOT in the base layer.
        assert!(!prim_exists(&doc, "/World/Obstacle"));
    }

    #[test]
    fn save_serializes_base_only_excluding_runtime_state() {
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(31),
            TINY_USDA,
            DocumentOrigin::writable_file("/tmp/r.usda"),
        );
        doc.apply(UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/World".into(),
            name: "SpawnedRock".into(),
            type_name: Some("Cube".into()),
            reference: None,
        })
        .unwrap();
        // The saved source (base layer) must NOT contain the runtime prim.
        let saved = doc.source();
        assert!(!saved.contains("SpawnedRock"), "runtime state leaked into save:\n{saved}");
        assert!(saved.contains("World"));
    }

    #[test]
    fn runtime_op_undo_restores_runtime_not_base() {
        let mut host = DocumentHost::new(UsdDocument::with_origin(
            DocumentId::new(32),
            TINY_USDA,
            DocumentOrigin::writable_file("/tmp/r.usda"),
        ));
        host.apply(Mutation::local(UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/World".into(),
            name: "Obstacle".into(),
            type_name: Some("Sphere".into()),
            reference: None,
        }))
        .unwrap();
        assert!(runtime_prim_exists(host.document(), "/World/Obstacle"));

        // Undo: the typed inverse is a RemovePrim TARGETING the runtime layer,
        // so it removes from runtime and never touches base.
        host.undo().unwrap();
        assert!(!runtime_prim_exists(host.document(), "/World/Obstacle"));
        assert!(prim_exists(host.document(), "/World"), "base layer intact across runtime undo");
    }

    #[test]
    fn composed_view_includes_runtime_but_source_excludes_it() {
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(34),
            TINY_USDA,
            DocumentOrigin::writable_file("/tmp/r.usda"),
        );
        doc.apply(UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/World".into(),
            name: "Obstacle".into(),
            type_name: Some("Sphere".into()),
            reference: None,
        })
        .unwrap();

        // The composed view (what the viewport renders) sees the runtime prim.
        let composed = doc.composed();
        assert_eq!(
            composed.prim_type_name(&SdfPath::new("/World/Obstacle").unwrap()).as_deref(),
            Some("Sphere")
        );
        assert!(doc.composed_source().contains("Obstacle"));
        // The saved source (base) does not.
        assert!(!doc.source().contains("Obstacle"));
    }

    #[test]
    fn spawn_op_authors_runtime_reference_excluded_from_save() {
        // C4b spawn producer: a spawn = a runtime prim that `references` its
        // asset (type comes from the reference, so `type_name: None`).
        let mut host = DocumentHost::new(UsdDocument::with_origin(
            DocumentId::new(36),
            TINY_USDA,
            DocumentOrigin::writable_file("/tmp/r.usda"),
        ));
        host.apply(Mutation::local(UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/World".into(),
            name: "rover_1".into(),
            type_name: None,
            reference: Some("vessels/rovers/skid_rover.usda".into()),
        }))
        .unwrap();

        // The reference opinion lives in the RUNTIME layer, not the base.
        assert!(runtime_prim_exists(host.document(), "/World/rover_1"));
        assert!(!prim_exists(host.document(), "/World/rover_1"), "spawn must not touch base");
        // It rides into the composed view (what the viewport renders /
        // re-instantiates) as a resolvable reference opinion...
        let composed = host.document().composed_source();
        assert!(
            composed.contains("@vessels/rovers/skid_rover.usda@"),
            "composed view must carry the spawn reference:\n{composed}"
        );
        // ...and is EXCLUDED from Save (base only).
        assert!(
            !host.document().source().contains("skid_rover"),
            "spawn leaked into the saved base layer:\n{}",
            host.document().source()
        );

        // Undo removes the spawn from runtime (typed AddPrim→RemovePrim inverse),
        // leaving the base untouched.
        host.undo().unwrap();
        assert!(!runtime_prim_exists(host.document(), "/World/rover_1"));
        assert!(prim_exists(host.document(), "/World"), "base intact across spawn undo");
    }

    #[test]
    fn base_and_runtime_ops_are_independent() {
        let mut doc = UsdDocument::new(DocumentId::new(33), TINY_USDA);
        // Author into base.
        doc.apply(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/".into(),
            name: "Rover".into(),
            type_name: Some("Xform".into()),
            reference: None,
        })
        .unwrap();
        // Author into runtime.
        doc.apply(UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/World".into(),
            name: "Obstacle".into(),
            type_name: Some("Sphere".into()),
            reference: None,
        })
        .unwrap();

        // Base has Rover but not Obstacle; runtime has Obstacle but not Rover.
        assert!(prim_exists(&doc, "/Rover"));
        assert!(!prim_exists(&doc, "/World/Obstacle"));
        assert!(runtime_prim_exists(&doc, "/World/Obstacle"));
        assert!(!runtime_prim_exists(&doc, "/Rover"));
    }
}
