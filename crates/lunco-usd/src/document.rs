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
//! LayerId` naming *which layer* receives the opinion. The document composes
//! **`base ⊕ runtime`**: [`LayerId::root`] authors the persisted base layer,
//! [`LayerId::runtime`] the ephemeral, **non-persisted** overlay — so a tool can
//! edit non-destructively over the base and promote to persistent on save.
//! `apply` routes to the target layer via [`TargetLayer::from_id`]; unknown
//! identifiers are rejected (no silent misrouting to root).
//!
//! ## Two representations, and why both are permanent
//!
//! A running scene is held in **two** forms, and neither can absorb the other:
//!
//! - **This document** — the authored [`sdf::Data`] layers (`base` ⊕ `runtime`,
//!   read via [`UsdDocument::data`] / [`UsdDocument::runtime_data`]). Plain,
//!   `Send`, serializable. This is what Save writes, what the journal records, and
//!   what the networking layer ships. Reads are cheap and run off the main thread.
//! - **The `CanonicalStage`** (in `lunco_usd_bevy`) — the live, *composed*
//!   openusd `Stage` with references / sublayers / variants resolved. It is
//!   `Rc`-backed and therefore `!Send`: a main-thread `NonSend` resource. It is
//!   the projection engine — authoring onto it fires the openusd change sink that
//!   reconciles the ECS (see [`twin_projection`](crate::twin_projection) and
//!   [`live_consume`](crate::live_consume)).
//!
//! This split is **not** a Rust/`Send` workaround — it is USD's own data model.
//! Pixar's USD draws the same line between `SdfLayer` (flat authored opinions you
//! save) and `UsdStage` (the composed view). You always have both: a layer is
//! *source*, a stage is the *composition* of layers. Collapsing them would mean
//! serializing a fully reference-expanded graph on every Save — which defeats the
//! entire purpose of references. The `Send` / `!Send` boundary merely happens to
//! fall on that same seam, so the two representations stay **even if openusd ever
//! makes `Stage` `Send`**. The right operations land on the cheap side: Save,
//! journal, and net-sync touch the small serializable layer; composition (the
//! expensive, stateful, resolver-driven work) is isolated to the one stage owner.
//!
//! ## Author-once coherence invariant
//!
//! Two representations of the same edit can drift, so the **op itself** — not a
//! diff re-derived by reading the stage back — is the single description of each
//! delta, applied to *both* sides: [`apply`](Document::apply) mutates these layers
//! and records the typed op in the private `op_log`; the live-stage projector
//! replays that same op onto the stage. The invariant that keeps them honest:
//!
//! > **every generation bump records exactly one op-log entry.**
//!
//! The private `commit` is the only mutator, and both its callers maintain it:
//! `apply` records the real op on success; [`UsdDocument::restore_runtime`]
//! (a non-op state load) pushes a synthetic `ReplaceSource` marker. Crucially the
//! invariant is **fail-safe, not merely by-convention**: [`UsdDocument::ops_since`]
//! returns `None` whenever the op ring is shorter than the generation delta, so a
//! future `commit` caller that forgets to record degrades to a full rebuild
//! (correct, just slower) — never a silent projection lie. That fail-safe is the
//! reason `restore_runtime` needs the synthetic marker at all: without it, a
//! restore would bump the generation with no op, and every subsequent
//! `ops_since` would under-count and force needless rebuilds.

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
/// Every variant carries an `edit_target: LayerId` naming *which layer*
/// receives the opinion — [`LayerId::root`] (persisted base) or
/// [`LayerId::runtime`] (ephemeral, non-persisted overlay); `apply` routes to
/// each. Unknown identifiers are rejected.
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
        /// Layer to write to: [`LayerId::root`] (base) or [`LayerId::runtime`] (overlay).
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
    /// Set the `xformOp:rotateXYZ` attribute (Euler XYZ, **degrees**) on the
    /// prim at `path` — the rotation counterpart of [`UsdOp::SetTranslate`].
    /// Authors `xformOpOrder` too if the prim has none yet (like `SetTranslate`,
    /// it only synthesizes a fresh order — it never rewrites an existing xform
    /// stack). This is what lets a `SetEnvironmentLight` sun-direction tweak
    /// persist + journal (the sun's orientation is `xformOp:rotateXYZ`).
    SetRotate {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose rotation to set.
        path: String,
        /// `[x, y, z]` Euler angles in **degrees** (USD `xformOp:rotateXYZ`).
        value: [f64; 3],
    },
    /// Set an arbitrary attribute on the prim at `path`. Creates the
    /// attribute if absent, replaces its value otherwise.
    ///
    /// The `value` encoding depends on `type_name`, and this is the ONE place it is
    /// interpreted so no call site hand-escapes:
    /// - `type_name == "string"` → `value` is the **raw** string content, authored
    ///   verbatim as `Value::String`. USDA's lexer keeps raw bytes between delimiters
    ///   (it does not unescape) and the writer picks a delimiter the content can't
    ///   close, so backslashes/quotes/newlines round-trip — pass arbitrary text (a
    ///   whole rhai scenario source) directly. The one unserializable value, both
    ///   `"""` and `'''` present, is rejected at apply.
    /// - any other type → `value` is a USD **literal** (e.g. `"(0.2, 0.2, 0.8)"`,
    ///   `"0.5"`), parsed into a typed [`sdf::Value`] by openusd's parser.
    SetAttribute {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose attribute to set.
        path: String,
        /// The name of the attribute (e.g. `primvars:displayColor` or `inputs:roughness`).
        name: String,
        /// The USD type name of the attribute (e.g. `color3f`, `float`, `string`).
        type_name: String,
        /// The value: **raw content** when `type_name == "string"`, otherwise a
        /// USD-compliant literal. See the variant doc for the split.
        value: String,
    },
    /// Author one **time sample** of an attribute on the prim at `path` —
    /// the keyframe primitive. Creates the attribute if absent (just like
    /// [`UsdOp::SetAttribute`]) and writes `value` at stage time `time`
    /// instead of as the `default`. Repeated ops at distinct `time`s build
    /// up the animation curve; the translator interpolates between them
    /// when it evaluates the attribute at a clock time. The inverse is the
    /// coarse full-source snapshot (sample removal has no typed op yet).
    SetTimeSample {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose attribute to keyframe.
        path: String,
        /// The name of the attribute (e.g. `xformOp:translate`, `inputs:roughness`).
        name: String,
        /// The USD type name of the attribute (e.g. `double3` or `float`).
        type_name: String,
        /// Stage (composed) time code at which to author the sample.
        time: f64,
        /// The sample value formatted as a USD-compliant string literal,
        /// parsed into a typed [`sdf::Value`] by openusd at apply time.
        value: String,
    },
    /// Remove the single **time sample** at `time` from attribute `name` on the
    /// prim at `path` — the inverse primitive to [`UsdOp::SetTimeSample`]. When
    /// the last sample goes, the attribute's `timeSamples` field is cleared
    /// entirely (it round-trips as if never keyframed). Removing a sample that
    /// isn't there is an error, not a silent success, so a wrong `time` surfaces.
    /// The inverse restores the prior full source (re-authoring the exact removed
    /// value as a typed op would mean reserializing it back to a literal).
    RemoveTimeSample {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose attribute to de-keyframe.
        path: String,
        /// The name of the attribute (e.g. `xformOp:translate`).
        name: String,
        /// Stage (composed) time code of the sample to remove.
        time: f64,
    },
    /// Author a **relationship** `name` on the prim at `path`, pointing at
    /// `targets` (absolute prim/property paths). Relationships are how USD
    /// expresses non-hierarchical links — `material:binding`, collection
    /// membership, light linking, skeleton bindings. Replaces any existing
    /// target list (set-semantics, not append); an empty `targets` authors an
    /// explicitly-empty relationship. The inverse restores the prior source.
    SetRelationship {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim that owns the relationship.
        path: String,
        /// The relationship name (e.g. `material:binding`).
        name: String,
        /// Absolute target paths the relationship points at.
        targets: Vec<String>,
    },
    /// Author the attribute-**connection** targets (`connectionPaths`) of
    /// attribute `name` on the prim at `path`. Connections are USD's typed
    /// dataflow edge — the primitive UsdShade builds every input/output wire
    /// on, generalized beyond shading. This is how a port/SSP wiring cutover
    /// authors an edge: the consuming attribute (`inputs:voltage`, an FMI/SSP
    /// input connector) `.connect`s to a producing property (`outputs:…`).
    ///
    /// The attribute spec is created if absent (using `type_name`, exactly like
    /// [`UsdOp::SetAttribute`]), so a connection can be authored on a
    /// not-yet-materialised port. `sources` replaces any prior connection list
    /// (explicit list-op, set-semantics — not append); an **empty** `sources`
    /// authors an explicitly-empty list, i.e. clears the connection. The
    /// inverse restores the prior full source.
    SetConnection {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim that owns the attribute.
        path: String,
        /// The attribute name (e.g. `inputs:voltage`).
        name: String,
        /// The USD type name of the attribute (e.g. `float`), used to create
        /// the spec if it does not exist yet on the target layer.
        type_name: String,
        /// Absolute property paths this attribute connects to
        /// (e.g. `/Bus/Node.outputs:v`). Empty clears the connection.
        sources: Vec<String>,
    },
    /// Move the prim at `from_path` to `to_path` — one op covering both
    /// **rename** (same parent, new leaf) and **reparent** (new parent), since
    /// both are a namespace move. The destination parent must already exist. The
    /// inverse is the exact reverse move (`from`/`to` swapped), so undo is typed
    /// and cheap.
    MovePrim {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim to move.
        from_path: String,
        /// New absolute USD path for the prim.
        to_path: String,
    },
    /// Author the prim's **applied API schemas** (`apiSchemas`) — the list that
    /// turns a plain prim into a rigid body, a collider, an articulation root.
    /// Without this op a prim built at runtime can never be made physical, so
    /// "assemble a vehicle from parts" was authorable in USD text and nowhere else.
    ///
    /// `schemas` is the exact desired list (set-semantics, an *explicit* list op —
    /// not an append), mirroring [`UsdOp::SetRelationship`]. Since an explicit
    /// opinion on a stronger layer replaces weaker `prepend apiSchemas` opinions
    /// wholesale, callers must pass the full set they want composed, not just the
    /// delta. An empty list authors an explicitly-empty schema list.
    SetApiSchemas {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim.
        path: String,
        /// The exact applied-schema names (e.g. `["PhysicsRigidBodyAPI"]`).
        schemas: Vec<String>,
    },
    /// Select `variant` within the prim's `variant_set`.
    ///
    /// Variant sets are already authored across the vessel assets (a rover's
    /// `drivetrain` swaps `raycast` for a fully physical joint rig) and nothing
    /// could switch one at runtime. This is the op behind "reconfigure the rover".
    ///
    /// Read-modify-write: selections for *other* variant sets on the same prim are
    /// preserved. Changing a selection re-composes the prim's subtree, so the
    /// projector rebuilds rather than replaying it incrementally.
    SetVariantSelection {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim carrying the variant set.
        path: String,
        /// The variant set name (e.g. `drivetrain`).
        variant_set: String,
        /// The variant to select (e.g. `physical`).
        variant: String,
    },
    /// Author the prim's **payloads** — references that lazy composition may
    /// decline to traverse, i.e. the arc for heavy geometry that should not be
    /// loaded until needed. Set-semantics (explicit list op); empty clears.
    ///
    /// The counterpart of [`UsdOp::AddPrim`]'s `reference`, which composes eagerly.
    SetPayload {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim.
        path: String,
        /// Asset paths to payload (e.g. `["@meshes/hull.usdc@"]`). Empty clears.
        asset_paths: Vec<String>,
    },
    /// Activate or deactivate the prim. A deactivated prim and its whole subtree
    /// vanish from composition without being deleted — the non-destructive
    /// "disable this part" every assembly editor needs, and cheaply reversible
    /// (unlike [`UsdOp::RemovePrim`], which discards the authored opinions).
    SetActive {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim.
        path: String,
        /// `false` prunes the prim and its descendants from the composed stage.
        active: bool,
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
    /// Ring buffer of `(generation_after_change, op)` — the **typed op** that
    /// produced each generation. The live-stage projection replays these ops
    /// directly (author-once: the op is the single delta description, applied to
    /// both this save layer and the `!Send` projection stage), so it never has to
    /// re-derive an edit's value by reading it back out of [`composed`](Self::composed).
    /// Non-op state changes (e.g. [`restore_runtime`](Self::restore_runtime)) push
    /// a synthetic [`UsdOp::ReplaceSource`] marker so the projector still rebuilds.
    /// See [`ops_since`](Self::ops_since).
    op_log: VecDeque<(u64, UsdOp)>,
    /// Memoized `base ⊕ runtime` composition, keyed by the `generation` it was
    /// composed at. [`composed`](Self::composed) is O(stage) (a full layer-stack
    /// merge) and is called several times per edit (the twin overlay serialize AND
    /// the doc-backed terrain re-parse), so without this a single brush stroke
    /// recomposed a thousand-prim stage 2–3× on the main thread. `Arc<Mutex<…>>` so
    /// the field stays `Clone`; the inner `Arc<sdf::Data>` makes a cache hit a
    /// refcount bump, not a stage copy. Shared across `Clone`s is safe — it's keyed
    /// by generation, and equal generations mean equal content.
    composed_cache: std::sync::Arc<std::sync::Mutex<Option<(u64, std::sync::Arc<sdf::Data>)>>>,
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
            op_log: VecDeque::with_capacity(CHANGE_HISTORY_CAPACITY),
            composed_cache: Default::default(),
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
        (*self.composed_arc()).clone()
    }

    /// The composed view as a shared [`Arc`], memoized by [`generation`](Document::generation).
    /// Prefer this over [`composed`](Self::composed) on hot paths (the twin projection,
    /// the doc-backed terrain re-bake) — repeated calls within one edit share the same
    /// recompose instead of each paying a full O(stage) layer merge.
    pub fn composed_arc(&self) -> std::sync::Arc<sdf::Data> {
        let gen = self.generation;
        // Poison recovery: this is the hot compose path, hit every frame. A panic
        // anywhere under the lock would otherwise poison it permanently, turning
        // one glitch into an unrecoverable per-frame panic. The cache is a pure
        // memo of `(generation, composed)` — a stale or absent entry is always
        // safe (it just recomputes), so there is no invariant to protect.
        {
            let cache = self.composed_cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((cached_gen, data)) = &*cache {
                if *cached_gen == gen {
                    return data.clone();
                }
            }
        }
        let data = std::sync::Arc::new(author::compose_layers(&self.base, &self.runtime));
        *self
            .composed_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some((gen, data.clone()));
        data
    }

    /// The composed view serialized to USDA text — the source the viewport
    /// re-parses so runtime-layer state becomes visible. Falls back to the raw
    /// (base) source when the base is un-parseable.
    pub fn composed_source(&self) -> String {
        if let Some(raw) = &self.parse_error {
            return raw.clone();
        }
        author::data_to_usda(&self.composed_arc()).unwrap_or_else(|e| {
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
        // Not a typed op, but it did bump the generation — push a synthetic
        // whole-source marker so the op-replay projector accounts for this
        // generation (a full rebuild) instead of treating the op ring as short.
        self.record_op(UsdOp::ReplaceSource {
            edit_target: LayerId::runtime(),
            text: String::new(),
        });
        if !was_dirty {
            self.last_saved_generation = Some(self.generation);
        }
    }

    /// Replace the **base** layer with `source` re-read from disk — a RE-OPEN of
    /// a document that is still resident, NOT an edit. The runtime layer is kept
    /// (the caller restores it separately), the generation bumps and a
    /// [`UsdChange::FullReload`] is recorded so the viewport rebuilds.
    ///
    /// WHY THIS EXISTS. Opening a Twin whose document is already resident used to
    /// reuse the in-memory document as-is, so a `.usda` edited on disk between
    /// opens replayed the OLD scene and only an app restart picked the change up.
    /// The stale text was upstream of the twin overlay and the asset store, which
    /// is why clearing either never helped. Local sessions read disk; the document
    /// is a projection of the file, not a cache of it.
    ///
    /// The text came FROM disk, so the document is clean at the new generation.
    /// Returns `false` (leaving the layer untouched) if `source` doesn't parse —
    /// a half-applied base would be worse than a stale one.
    ///
    /// NOT PUBLIC ON PURPOSE — go through
    /// [`DocumentRegistry::<UsdDocument>::open_file`](crate::registry::DocumentRegistry::<UsdDocument>::open_file).
    /// This silently discards unsaved base edits and undo cannot bring them
    /// back, so the `is_dirty` check must not be a thing a caller can forget.
    pub(crate) fn reload_base(&mut self, source: &str) -> bool {
        match usda_to_data(source) {
            Ok(data) => {
                self.commit(TargetLayer::Base, data, UsdChange::FullReload);
                // The commit bumped the generation WITHOUT going through a typed
                // op, so record a synthetic whole-source marker. The op-replay
                // projector accounts for generations via the op ring; a
                // generation with no op makes the ring look SHORT and it replays
                // from the wrong point. Same reason and same shape as
                // `restore_runtime` — the base layer is `LayerId::root()`.
                self.record_op(UsdOp::ReplaceSource {
                    edit_target: LayerId::root(),
                    text: String::new(),
                });
                self.parse_error = None;
                // Matches disk as of this generation ⇒ clean.
                self.last_saved_generation = Some(self.generation);
                true
            }
            Err(e) => {
                warn!(
                    "[usd] document {} re-read from disk did not parse as USDA ({e}); \
                     keeping the resident base layer",
                    self.id.raw()
                );
                false
            }
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

    /// The typed ops applied strictly after `since_generation`, in order — the
    /// live-stage projection replays these directly onto the `!Send` stage
    /// (author-once). If the op ring dropped entries (more edits than its capacity
    /// since `since_generation`), returns `None` so the caller falls back to a
    /// full rebuild rather than silently missing deltas.
    pub fn ops_since(&self, since_generation: u64) -> Option<Vec<UsdOp>> {
        let expected = self.generation.saturating_sub(since_generation);
        let ops: Vec<UsdOp> = self
            .op_log
            .iter()
            .filter(|(g, _)| *g > since_generation)
            .map(|(_, op)| op.clone())
            .collect();
        (ops.len() as u64 >= expected).then_some(ops)
    }

    /// Record the typed op that produced the current generation, for
    /// [`ops_since`](Self::ops_since). Called right after a successful
    /// [`commit`](Self::commit). Non-op state changes push a synthetic marker.
    fn record_op(&mut self, op: UsdOp) {
        if self.op_log.len() == CHANGE_HISTORY_CAPACITY {
            self.op_log.pop_front();
        }
        self.op_log.push_back((self.generation, op));
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

/// The `xformOpOrder` tokens `data` holds for `prim`, flattening any list-op
/// authoring. Empty when unauthored.
fn xform_op_order_tokens(data: &sdf::Data, prim: &SdfPath) -> Vec<String> {
    let Ok(attr) = prim.append_property("xformOpOrder") else {
        return Vec::new();
    };
    match data.field(&attr, "default").cloned() {
        Some(sdf::Value::TokenVec(v)) => v.into_iter().map(Into::into).collect(),
        Some(sdf::Value::StringVec(v)) => v,
        Some(sdf::Value::TokenListOp(op)) => op.flatten().into_iter().map(Into::into).collect(),
        Some(sdf::Value::StringListOp(op)) => op.flatten(),
        _ => Vec::new(),
    }
}

/// The identity contract: one document per file, content refreshed from disk,
/// unsaved edits never clobbered. Everything here already existed as inherent
/// methods — this just hands them to the generic
/// [`DocumentRegistry`](lunco_doc_bevy::DocumentRegistry) so USD stops carrying
/// its own copy of the open-by-path rule.
impl lunco_doc::FileBacked for UsdDocument {
    fn with_origin(id: DocumentId, source: String, origin: DocumentOrigin) -> Self {
        UsdDocument::with_origin(id, source, origin)
    }

    fn origin(&self) -> &DocumentOrigin {
        &self.origin
    }

    fn is_dirty(&self) -> bool {
        UsdDocument::is_dirty(self)
    }

    fn reload_base(&mut self, source: &str) -> bool {
        UsdDocument::reload_base(self, source)
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
            | UsdOp::SetRotate { edit_target, .. }
            | UsdOp::SetAttribute { edit_target, .. }
            | UsdOp::SetTimeSample { edit_target, .. }
            | UsdOp::RemoveTimeSample { edit_target, .. }
            | UsdOp::SetRelationship { edit_target, .. }
            | UsdOp::SetConnection { edit_target, .. }
            | UsdOp::MovePrim { edit_target, .. }
            | UsdOp::SetApiSchemas { edit_target, .. }
            | UsdOp::SetVariantSelection { edit_target, .. }
            | UsdOp::SetPayload { edit_target, .. }
            | UsdOp::SetActive { edit_target, .. } => edit_target.clone(),
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

        // Author-once: remember the exact typed op so the live-stage projector
        // replays it verbatim (no re-deriving the delta from `composed`). Recorded
        // only on success — a rejected op never bumps the generation.
        let logged_op = op.clone();
        let result = match op {
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
                // that layer's opinion, and `xformOpOrder` we author lands
                // there too.
                let layer = self.layer(target);
                let translate_existed = prim_sdf
                    .append_property("xformOp:translate")
                    .ok()
                    .and_then(|p| layer.spec(&p).map(|_| ()))
                    .is_some();
                let old_translate = layer.prim_attribute_value::<[f64; 3]>(&prim_sdf, "xformOp:translate");
                // The op order is checked against the COMPOSED opinion: a weaker
                // layer may already list ops this edit must not discard. When
                // the op is missing, materialise that order plus the new op into
                // the target layer — append, never clobber.
                let composed_order = xform_op_order_tokens(&self.composed_arc(), &prim_sdf);
                let append_op = !composed_order.iter().any(|t| t == "xformOp:translate");

                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage
                    .create_attribute(format!("{path}.xformOp:translate"), "double3")
                    .map_err(author_err)?
                    .set(value)
                    .map_err(author_err)?;
                if append_op {
                    let mut order = composed_order;
                    order.push("xformOp:translate".into());
                    stage
                        .create_attribute(format!("{path}.xformOpOrder"), "token[]")
                        .map_err(author_err)?
                        .set(sdf::Value::token_vec(order))
                        .map_err(author_err)?;
                }
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;

                // Typed inverse only when this purely overwrote an existing
                // translate in this layer (no `xformOpOrder` was authored).
                let inverse = if translate_existed && !append_op {
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

            UsdOp::SetRotate { path, value, .. } => {
                // Direct mirror of `SetTranslate` for `xformOp:rotateXYZ`
                // (Euler XYZ degrees). Same target-layer pre-state read, same
                // composed-order append rule.
                let prim_sdf = self.require_prim_anywhere(&path)?;
                let layer = self.layer(target);
                let rotate_existed = prim_sdf
                    .append_property("xformOp:rotateXYZ")
                    .ok()
                    .and_then(|p| layer.spec(&p).map(|_| ()))
                    .is_some();
                let old_rotate = layer.prim_attribute_value::<[f64; 3]>(&prim_sdf, "xformOp:rotateXYZ");
                let composed_order = xform_op_order_tokens(&self.composed_arc(), &prim_sdf);
                let append_op = !composed_order.iter().any(|t| t == "xformOp:rotateXYZ");

                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage
                    .create_attribute(format!("{path}.xformOp:rotateXYZ"), "double3")
                    .map_err(author_err)?
                    .set(value)
                    .map_err(author_err)?;
                if append_op {
                    let mut order = composed_order;
                    order.push("xformOp:rotateXYZ".into());
                    stage
                        .create_attribute(format!("{path}.xformOpOrder"), "token[]")
                        .map_err(author_err)?
                        .set(sdf::Value::token_vec(order))
                        .map_err(author_err)?;
                }
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;

                let inverse = if rotate_existed && !append_op {
                    old_rotate
                        .map(|old| UsdOp::SetRotate {
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
                        attr: "xformOp:rotateXYZ".into(),
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
                let prim_sdf = self.require_prim_anywhere(&path)?;

                // The single place attribute values are turned into USD values, so
                // NO call site ever hand-escapes. Two rules by type:
                //   • `string` → the value is RAW content, authored as `Value::String`
                //     with no literal parsing. USDA's lexer keeps raw bytes between
                //     delimiters (it does not unescape), and the writer picks a
                //     delimiter the content can't close — so backslashes, quotes and
                //     newlines round-trip verbatim. The one thing USDA cannot delimit
                //     is a value containing BOTH `"""` and `'''`; reject that here, at
                //     apply, not at save (a stranded unsavable document is worse).
                //   • everything else → the value is a USD literal we parse.
                let is_string = type_name == "string";
                let val = if is_string {
                    if value.contains("\"\"\"") && value.contains("'''") {
                        return Err(DocumentError::ValidationFailed(format!(
                            "SetAttribute `{name}` (string): value contains both `\"\"\"` and \
                             `'''`, which USDA cannot delimit (its lexer does not unescape)"
                        )));
                    }
                    openusd::sdf::Value::String(value.clone())
                } else {
                    parse_attribute_value(&type_name, &value).map_err(|e| {
                        DocumentError::ValidationFailed(format!(
                            "SetAttribute `{name}` ({type_name}): {e}"
                        ))
                    })?
                };

                // Typed inverse: restore the attribute's prior value in THIS layer,
                // so undo replays incrementally (the projector's `apply_incremental_
                // op_to_stage` path) instead of a `ReplaceSource` that forces a
                // whole-layer rebuild. Only when the attribute already had a value
                // here that round-trips; a newly-authored attribute (or an
                // un-recoverable literal) falls back to the always-correct whole-
                // source snapshot — which also correctly *removes* the new opinion on
                // undo, something a typed `SetAttribute` cannot express. For a string
                // the prior value is recovered RAW (matching the raw author above);
                // for other types via `value_to_literal`.
                let prior = prim_sdf
                    .append_property(name.as_str())
                    .ok()
                    .and_then(|attr| self.layer(target).field(&attr, "default").cloned());
                let recovered = if is_string {
                    match prior {
                        Some(openusd::sdf::Value::String(s)) => Some(s),
                        _ => None,
                    }
                } else {
                    prior.and_then(|old| author::value_to_literal(&type_name, old))
                };
                let inverse = match recovered {
                    Some(v) => UsdOp::SetAttribute {
                        edit_target: id.clone(),
                        path: path.clone(),
                        name: name.clone(),
                        type_name: type_name.clone(),
                        value: v,
                    },
                    None => self.coarse_inverse(target, &id),
                };
                // Variability and `custom` are declared by the SCHEMA, not by the
                // call site — see `crate::schema`. Deciding them here, in the one
                // place attributes are authored, is what makes it impossible for a
                // caller to author `info:id` as `varying` (which is how it *was*
                // authored, because nothing knew better) or to omit `custom` on a
                // per-model `lunco:` param that no schema declares.
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage
                    .create_attribute(format!("{path}.{name}"), type_name.as_str())
                    .map_err(author_err)?
                    .set_variability(crate::schema::variability_of(&name))
                    .map_err(author_err)?
                    .set_custom(crate::schema::is_custom(&name))
                    .map_err(author_err)?
                    .set(val)
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::InfoOnly { path, attr: name });
                Ok(inverse)
            }

            UsdOp::SetTimeSample {
                path,
                name,
                type_name,
                time,
                value,
                ..
            } => {
                let prim_sdf = self.require_prim_anywhere(&path)?;
                let val = parse_attribute_value(&type_name, &value).map_err(|e| {
                    DocumentError::ValidationFailed(format!(
                        "SetTimeSample `{name}` ({type_name}) @ {time}: {e}"
                    ))
                })?;
                // Authoring a brand-new sample (no prior opinion at this exact
                // time, in this layer) is exactly undone by removing it — a typed,
                // cheap inverse. Overwriting an existing sample needs the prior
                // value back, so fall back to the full-source snapshot.
                let overwrote_existing = prim_sdf
                    .append_property(name.as_str())
                    .ok()
                    .and_then(|attr| self.layer(target).field(&attr, "timeSamples").cloned())
                    .map(|v| {
                        matches!(v, sdf::Value::TimeSamples(ref m)
                            if m.iter().any(|(t, _)| t.total_cmp(&time).is_eq()))
                    })
                    .unwrap_or(false);
                let inverse = if overwrote_existing {
                    self.coarse_inverse(target, &id)
                } else {
                    UsdOp::RemoveTimeSample {
                        edit_target: id.clone(),
                        path: path.clone(),
                        name: name.clone(),
                        time,
                    }
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage
                    .create_attribute(format!("{path}.{name}"), type_name.as_str())
                    .map_err(author_err)?
                    .set_at(val, openusd::usd::TimeCode::new(time))
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::InfoOnly { path, attr: name });
                Ok(inverse)
            }

            UsdOp::RemoveTimeSample {
                path, name, time, ..
            } => {
                let prim_sdf = self.require_prim_anywhere(&path)?;
                let attr_sdf = prim_sdf.append_property(name.as_str()).map_err(|e| {
                    DocumentError::ValidationFailed(format!(
                        "RemoveTimeSample: bad attribute `{name}`: {e}"
                    ))
                })?;
                // The full-source snapshot restores the removed sample's value
                // (reserializing it to a typed `SetTimeSample` literal isn't worth
                // it). Capture it before mutating.
                let inverse = self.coarse_inverse(target, &id);
                let mut new_data = self.layer(target).clone();
                let removed = author::remove_time_sample(&mut new_data, &attr_sdf, time)
                    .map_err(author_err)?;
                if removed.is_none() {
                    return Err(DocumentError::ValidationFailed(format!(
                        "RemoveTimeSample: no sample on `{path}.{name}` at time {time}"
                    )));
                }
                self.commit(target, new_data, UsdChange::InfoOnly { path, attr: name });
                Ok(inverse)
            }

            UsdOp::SetRelationship {
                path,
                name,
                targets,
                ..
            } => {
                self.require_prim_anywhere(&path)?;
                let target_paths = targets
                    .iter()
                    .map(|t| {
                        SdfPath::new(t).map_err(|e| {
                            DocumentError::ValidationFailed(format!(
                                "SetRelationship `{name}`: invalid target `{t}`: {e}"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let inverse = self.coarse_inverse(target, &id);
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage
                    .create_relationship(format!("{path}.{name}"))
                    .map_err(author_err)?
                    .set_targets(target_paths)
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::InfoOnly { path, attr: name });
                Ok(inverse)
            }

            UsdOp::SetConnection {
                path,
                name,
                type_name,
                sources,
                ..
            } => {
                self.require_prim_anywhere(&path)?;
                let source_paths = sources
                    .iter()
                    .map(|s| {
                        SdfPath::new(s).map_err(|e| {
                            DocumentError::ValidationFailed(format!(
                                "SetConnection `{name}`: invalid source `{s}`: {e}"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let inverse = self.coarse_inverse(target, &id);
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                // Create-if-absent (like SetAttribute) so a connection can be
                // authored on a not-yet-materialised port, then author the
                // `connectionPaths` list op (explicit; empty clears).
                stage
                    .create_attribute(format!("{path}.{name}"), type_name.as_str())
                    .map_err(author_err)?
                    .set_connections(source_paths)
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::InfoOnly { path, attr: name });
                Ok(inverse)
            }

            UsdOp::MovePrim {
                from_path,
                to_path,
                ..
            } => {
                // Only move what the target layer itself authored.
                self.require_prim_in(target, &from_path)?;
                let from_sdf = parse_prim_path(&from_path)?;
                let to_sdf = parse_prim_path(&to_path)?;
                // Exact reverse move — a typed, cheap inverse.
                let inverse = UsdOp::MovePrim {
                    edit_target: id.clone(),
                    from_path: to_path.clone(),
                    to_path: from_path.clone(),
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                let mut editor = openusd::usd::NamespaceEditor::new(&stage);
                editor.move_prim(from_sdf, to_sdf);
                editor.apply().map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                // A move changes prim paths on both ends; the translator re-keys
                // entities by path, so a full reload is the honest change kind.
                self.commit(target, new_data, UsdChange::FullReload);
                Ok(inverse)
            }

            UsdOp::SetApiSchemas { path, schemas, .. } => {
                self.require_prim_anywhere(&path)?;
                let inverse = self.coarse_inverse(target, &id);
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                let tokens: Vec<openusd::tf::Token> =
                    schemas.iter().map(openusd::tf::Token::from).collect();
                stage
                    .prim(path.as_str())
                    .set_metadata(
                        openusd::sdf::FieldKey::ApiSchemas.as_str(),
                        openusd::sdf::Value::TokenListOp(openusd::sdf::TokenListOp::explicit(
                            tokens,
                        )),
                    )
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                // Applied schemas decide which ECS components the translator
                // attaches (rigid body, collider) — the prim must be re-projected.
                self.commit(target, new_data, UsdChange::Resync { path });
                Ok(inverse)
            }

            UsdOp::SetVariantSelection {
                path,
                variant_set,
                variant,
                ..
            } => {
                self.require_prim_anywhere(&path)?;
                let inverse = self.coarse_inverse(target, &id);
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                // Read-modify-write the selection map so selecting `drivetrain`
                // doesn't silently drop a sibling variant set's selection.
                stage
                    .prim(path.as_str())
                    .update_metadata(
                        openusd::sdf::FieldKey::VariantSelection.as_str(),
                        |current| {
                            let mut map = match current {
                                Some(openusd::sdf::Value::VariantSelectionMap(m)) => m,
                                _ => Default::default(),
                            };
                            map.insert(variant_set.clone(), variant.clone());
                            openusd::sdf::Value::VariantSelectionMap(map)
                        },
                    )
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::Resync { path });
                Ok(inverse)
            }

            UsdOp::SetPayload {
                path, asset_paths, ..
            } => {
                self.require_prim_anywhere(&path)?;
                let inverse = self.coarse_inverse(target, &id);
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                let payloads: Vec<openusd::sdf::Payload> = asset_paths
                    .iter()
                    .map(|a| openusd::sdf::Payload {
                        asset_path: a.clone(),
                        ..Default::default()
                    })
                    .collect();
                stage
                    .prim(path.as_str())
                    .set_metadata(
                        openusd::sdf::FieldKey::Payload.as_str(),
                        openusd::sdf::Value::PayloadListOp(
                            openusd::sdf::PayloadListOp::explicit(payloads),
                        ),
                    )
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::Resync { path });
                Ok(inverse)
            }

            UsdOp::SetActive { path, active, .. } => {
                self.require_prim_anywhere(&path)?;
                // NOT `SetActive { active: !active }`: that assumes the prim was in
                // the opposite state. Deactivating an already-inactive prim would
                // then "undo" into activating it. The snapshot inverse restores the
                // target layer's real prior opinion, including *unauthored*.
                let inverse = self.coarse_inverse(target, &id);
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage
                    .prim(path.as_str())
                    .set_active(active)
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::Resync { path });
                Ok(inverse)
            }

        };
        if result.is_ok() {
            self.record_op(logged_op);
        }
        result
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

    // TODO(usd-read-migration): these assertions read the flattened `sdf::Data`
    // via the legacy `UsdDataExt` (`prim_attribute_value`/`prim_type_name`). Switch
    // to the generic `UsdRead` surface (`scalar`/`type_name`) to match production
    // (doc 21). Time-sampled reads (`prim_attribute_value_at`) → `scalar_at`.

    const TINY_USDA: &str =
        "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\ndef Xform \"World\"\n{\n}\n";

    fn prim_type(doc: &UsdDocument, path: &str) -> Option<String> {
        doc.data().prim_type_name(&SdfPath::new(path).unwrap())
    }
    fn prim_exists(doc: &UsdDocument, path: &str) -> bool {
        doc.data().spec(&SdfPath::new(path).unwrap()).is_some()
    }

    /// Whether a doc's serialized source **reparses cleanly** — the check that
    /// catches malformed metadata a substring assertion misses (e.g. a payload
    /// asset path wrapped `@@…@@` still `contains("hull")` but won't parse). A
    /// fresh document from un-parseable source blocks every structural op but
    /// `ReplaceSource`, so a probe `AddPrim` succeeding proves the source parsed.
    fn reparses_cleanly(doc: &UsdDocument) -> bool {
        let mut d2 = UsdDocument::with_origin(
            DocumentId::new(9999),
            doc.source(),
            DocumentOrigin::writable_file("/tmp/roundtrip.usda"),
        );
        d2.apply(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/".into(),
            name: "RtProbe".into(),
            type_name: Some("Xform".into()),
            reference: None,
        })
        .is_ok()
    }

    #[test]
    fn attach_component_sequence_applies_end_to_end() {
        // The attach lowering's op *shape* is unit-tested in `crate::attach`; this
        // proves the whole sequence actually APPLIES in order onto a real document —
        // the joint prim is defined before its relationships target it, the point3f
        // anchors author, and the result composes into a jointed assembly.
        use crate::attach::{attach_component_ops, AttachJoint, AttachSpec, Axis};

        let scene = "#usda 1.0\ndef Xform \"Rig\"\n{\n    def Xform \"Chassis\"\n    {\n    }\n}\n";
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(50),
            scene,
            DocumentOrigin::writable_file("/tmp/attach.usda"),
        );

        let spec = AttachSpec::new(
            LayerId::root(),
            "/Rig/Chassis",
            "Wheel",
            "components/mobility/wheel.usda",
            [0.5, -0.3, 1.2],
            AttachJoint::Revolute { axis: Axis::X },
        );
        for op in attach_component_ops(&spec) {
            doc.apply(op).expect("each attach op applies in sequence");
        }

        // The part and the joint are both authored…
        assert!(prim_exists(&doc, "/Rig/Chassis/Wheel"), "part referenced in");
        assert_eq!(
            prim_type(&doc, "/Rig/Chassis/Wheel_Joint").as_deref(),
            Some("PhysicsRevoluteJoint"),
            "joint prim defined with the requested type"
        );
        // …and the joint relates the two bodies, with the anchor derived from the
        // placement (localPos0) — the whole point of the lowering.
        let src = doc.source();
        assert!(src.contains("physics:body0") && src.contains("/Rig/Chassis"), "body0 → host");
        assert!(src.contains("physics:body1") && src.contains("/Rig/Chassis/Wheel"), "body1 → part");
        assert!(src.contains("physics:localPos0"), "anchor authored");
        assert!(src.contains("physics:axis"), "revolute axis authored");
    }

    #[test]
    fn set_attribute_string_round_trips_realistic_rhai_verbatim() {
        // `SetAttribute` with type `string` authors the value RAW: a rhai scenario's
        // source must survive serialize→reparse byte-for-byte without the caller
        // hand-escaping a USD literal. This is what real rhai looks like — embedded
        // double quotes, backslashes, and newlines. (The openusd USDA lexer keeps raw
        // bytes between triple-quote delimiters, so `\"` and `\` pass through verbatim
        // — no escape processing to corrupt them.)
        let src = "fn on_tick(me) {\n    let s = \"he said \\\"hi\\\"\";\n    let path = \"C:\\\\rover\";\n    notify(s + path, \"info\");\n}\n";
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(60),
            "#usda 1.0\ndef Xform \"Rover\"\n{\n}\n",
            DocumentOrigin::writable_file("/tmp/script.usda"),
        );
        doc.apply(UsdOp::SetAttribute {
            edit_target: LayerId::root(),
            path: "/Rover".into(),
            name: "lunco:script".into(),
            type_name: "string".into(),
            value: src.to_string(),
        })
        .unwrap();

        // Serialize, then reparse from scratch — the true round-trip a save+reload
        // does. The recovered value must equal the original verbatim.
        let reparsed = UsdDocument::with_origin(
            DocumentId::new(61),
            doc.source(),
            DocumentOrigin::writable_file("/tmp/script2.usda"),
        );
        let got = reparsed
            .data()
            .prim_attribute_value::<String>(&SdfPath::new("/Rover").unwrap(), "lunco:script");
        assert_eq!(
            got.as_deref(),
            Some(src),
            "real rhai source must round-trip verbatim.\nserialized:\n{}",
            doc.source()
        );
    }

    #[test]
    fn set_attribute_string_rejects_unserializable_both_triple_delimiters() {
        // The one thing USDA cannot delimit: a value containing BOTH `"""` and
        // `'''` (its lexer does not unescape, so neither triple-quote is safe). We
        // reject at apply, not at save — a stranded unsavable document is worse than
        // a clear up-front error. Real rhai never produces this.
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(63),
            "#usda 1.0\ndef Xform \"Rover\"\n{\n}\n",
            DocumentOrigin::writable_file("/tmp/script3.usda"),
        );
        let err = doc.apply(UsdOp::SetAttribute {
            edit_target: LayerId::root(),
            path: "/Rover".into(),
            name: "lunco:script".into(),
            type_name: "string".into(),
            value: "a \"\"\" b ''' c".into(),
        });
        assert!(
            matches!(err, Err(DocumentError::ValidationFailed(_))),
            "both-triple-delimiter content must be rejected at apply, got {err:?}"
        );
        // And the document is untouched — the rejected op left no partial edit.
        assert!(
            !doc.source().contains("lunco:script"),
            "a rejected op must not partially author"
        );
    }

    #[test]
    fn set_attribute_string_undoes() {
        let mut host = DocumentHost::new(UsdDocument::with_origin(
            DocumentId::new(62),
            "#usda 1.0\ndef Xform \"Rover\"\n{\n}\n",
            DocumentOrigin::writable_file("/tmp/s.usda"),
        ));
        host.apply(Mutation::local(UsdOp::SetAttribute {
            edit_target: LayerId::root(),
            path: "/Rover".into(),
            name: "lunco:script".into(),
            type_name: "string".into(),
            value: "fn on_tick(me) {}".into(),
        }))
        .unwrap();
        assert!(host.document().source().contains("on_tick"), "authored");
        assert!(reparses_cleanly(host.document()), "authored string reparses cleanly");
        host.undo().unwrap();
        assert!(
            !host.document().source().contains("on_tick"),
            "undo removes the newly-authored attribute: {}",
            host.document().source()
        );
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

    /// Author-once: `ops_since` returns the exact typed ops the live-stage
    /// projector replays — the suffix strictly after a generation, in order, with
    /// each op verbatim (so the projector never re-derives the delta from state).
    #[test]
    fn ops_since_returns_typed_op_suffix() {
        let mut doc = UsdDocument::new(DocumentId::new(30), TINY_USDA);
        doc.apply(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/World".into(),
            name: "Box".into(),
            type_name: Some("Cube".into()),
            reference: None,
        })
        .unwrap();
        let after_spawn = doc.generation();
        doc.apply(UsdOp::SetTranslate {
            edit_target: LayerId::root(),
            path: "/World/Box".into(),
            value: [1.0, 2.0, 3.0],
        })
        .unwrap();

        // From the start: both ops, in order.
        let all = doc.ops_since(0).expect("ring not overflowed");
        assert_eq!(all.len(), 2);
        assert!(matches!(all[0], UsdOp::AddPrim { ref name, .. } if name == "Box"));
        assert!(matches!(all[1], UsdOp::SetTranslate { value, .. } if value == [1.0, 2.0, 3.0]));

        // Strictly after the spawn: just the translate (verbatim value).
        let tail = doc.ops_since(after_spawn).expect("ring not overflowed");
        assert_eq!(tail.len(), 1);
        assert!(matches!(tail[0], UsdOp::SetTranslate { value, .. } if value == [1.0, 2.0, 3.0]));

        // A `since` far below current with entries dropped can't be trusted → None.
        assert!(doc.ops_since(0).is_some(), "no overflow for a short history");
    }

    /// A rejected op neither bumps the generation nor records into the op log, so
    /// the projector never replays a no-op.
    #[test]
    fn rejected_op_is_not_logged() {
        let mut doc = UsdDocument::new(DocumentId::new(31), TINY_USDA);
        // Unknown parent → validation failure, no commit.
        let _ = doc.apply(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/Nope".into(),
            name: "X".into(),
            type_name: Some("Xform".into()),
            reference: None,
        });
        assert_eq!(doc.generation(), 0);
        assert_eq!(doc.ops_since(0).unwrap().len(), 0, "rejected op is not in the op log");
    }

    /// Author-once's load-bearing invariant: **every generation bump records
    /// exactly one op-log entry**, so `ops_since(0).len() == generation`. This
    /// must hold across the *non-op* path too — [`restore_runtime`] bumps the
    /// generation without a typed op, and relies on the synthetic marker to stay
    /// in lockstep. If a future `commit` caller breaks this, `ops_since` under-
    /// counts and the projector falls back to a full rebuild (fail-safe) rather
    /// than under-applying — this test pins the lockstep so that stays a
    /// deliberate choice, not an accident.
    #[test]
    fn op_log_stays_in_lockstep_with_generation() {
        let mut doc = UsdDocument::new(DocumentId::new(32), TINY_USDA);
        doc.apply(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/World".into(),
            name: "a".into(),
            type_name: Some("Xform".into()),
            reference: None,
        })
        .unwrap();
        doc.apply(UsdOp::SetTranslate {
            edit_target: LayerId::root(),
            path: "/World/a".into(),
            value: [1.0, 2.0, 3.0],
        })
        .unwrap();
        // A non-op runtime restore also bumps the generation — the synthetic
        // marker must keep the op log one-per-generation.
        doc.restore_runtime(usda_to_data(TINY_USDA).unwrap());

        let ops = doc.ops_since(0).expect("op ring holds an entry for every generation");
        assert_eq!(
            ops.len() as u64,
            doc.generation(),
            "one op-log entry per generation bump (incl. the restore_runtime marker)"
        );
    }

    /// Overwriting an **existing** attribute inverts to a *typed* `SetAttribute`
    /// carrying the prior value — so undo replays incrementally rather than
    /// forcing a whole-layer `ReplaceSource` rebuild. Applying that inverse
    /// restores the original value.
    #[test]
    fn set_attribute_overwrite_inverts_to_typed_op() {
        const SCENE: &str = "#usda 1.0\ndef Sphere \"Ball\"\n{\n    double radius = 1\n}\n";
        let mut doc = UsdDocument::new(DocumentId::new(40), SCENE);
        let ball = SdfPath::new("/Ball").unwrap();

        let inverse = doc
            .apply(UsdOp::SetAttribute {
                edit_target: LayerId::root(),
                path: "/Ball".into(),
                name: "radius".into(),
                type_name: "double".into(),
                value: "5".into(),
            })
            .unwrap();
        assert_eq!(doc.data().prim_attribute_value::<f64>(&ball, "radius"), Some(5.0));
        assert!(
            matches!(&inverse, UsdOp::SetAttribute { name, .. } if name == "radius"),
            "overwrite of an existing attribute must invert to a typed SetAttribute, got {inverse:?}"
        );

        // Replaying the inverse restores the prior value incrementally.
        doc.apply(inverse).unwrap();
        assert_eq!(doc.data().prim_attribute_value::<f64>(&ball, "radius"), Some(1.0));
    }

    /// Authoring a **brand-new** attribute has no prior value to restore, so it
    /// inverts to the always-correct whole-source snapshot — which also *removes*
    /// the new opinion on undo (something a typed `SetAttribute` cannot express).
    #[test]
    fn set_attribute_create_inverts_to_coarse_snapshot() {
        const SCENE: &str = "#usda 1.0\ndef Sphere \"Ball\"\n{\n}\n";
        let mut doc = UsdDocument::new(DocumentId::new(41), SCENE);
        let ball = SdfPath::new("/Ball").unwrap();

        let inverse = doc
            .apply(UsdOp::SetAttribute {
                edit_target: LayerId::root(),
                path: "/Ball".into(),
                name: "radius".into(),
                type_name: "double".into(),
                value: "5".into(),
            })
            .unwrap();
        assert!(
            matches!(inverse, UsdOp::ReplaceSource { .. }),
            "a newly-authored attribute inverts to a whole-source snapshot, got {inverse:?}"
        );

        // Undo removes the attribute entirely.
        doc.apply(inverse).unwrap();
        assert_eq!(
            doc.data().prim_attribute_value::<f64>(&ball, "radius"),
            None,
            "undo of a newly-authored attribute removes it"
        );
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

    /// `xformOpOrder` ACCUMULATES: authoring a second xform op appends to the
    /// order in author order — it must not replace the list with a one-element
    /// order, which silently discards the first op at composition time even
    /// though its value attribute survives.
    #[test]
    fn set_translate_then_rotate_lists_both_ops_in_author_order() {
        let scene = "#usda 1.0\ndef Xform \"Rig\"\n{\n}\n";
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(60),
            scene,
            DocumentOrigin::writable_file("/tmp/order_tr.usda"),
        );
        doc.apply(UsdOp::SetTranslate {
            edit_target: LayerId::root(),
            path: "/Rig".into(),
            value: [1.0, 2.0, 3.0],
        })
        .unwrap();
        doc.apply(UsdOp::SetRotate {
            edit_target: LayerId::root(),
            path: "/Rig".into(),
            value: [0.0, 90.0, 0.0],
        })
        .unwrap();

        let rig = SdfPath::new("/Rig").unwrap();
        assert_eq!(
            xform_op_order_tokens(&doc.composed_arc(), &rig),
            vec!["xformOp:translate".to_string(), "xformOp:rotateXYZ".to_string()],
            "both ops listed, in author order"
        );
        // Both value attributes were authored too.
        assert_eq!(
            doc.data().prim_attribute_value::<[f64; 3]>(&rig, "xformOp:translate"),
            Some([1.0, 2.0, 3.0])
        );
        assert_eq!(
            doc.data().prim_attribute_value::<[f64; 3]>(&rig, "xformOp:rotateXYZ"),
            Some([0.0, 90.0, 0.0])
        );

        // Re-setting an op already in the order overwrites the value WITHOUT
        // duplicating its order entry.
        doc.apply(UsdOp::SetRotate {
            edit_target: LayerId::root(),
            path: "/Rig".into(),
            value: [0.0, 45.0, 0.0],
        })
        .unwrap();
        assert_eq!(
            xform_op_order_tokens(&doc.composed_arc(), &rig),
            vec!["xformOp:translate".to_string(), "xformOp:rotateXYZ".to_string()],
            "re-set of an existing op must not duplicate its xformOpOrder entry"
        );
        assert_eq!(
            doc.data().prim_attribute_value::<[f64; 3]>(&rig, "xformOp:rotateXYZ"),
            Some([0.0, 45.0, 0.0])
        );
    }

    /// The order is the AUTHOR order, not a canonical translate-first order:
    /// rotate authored first stays first.
    #[test]
    fn xform_op_order_is_author_order_not_canonical() {
        let scene = "#usda 1.0\ndef Xform \"Rig\"\n{\n}\n";
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(61),
            scene,
            DocumentOrigin::writable_file("/tmp/order_rt.usda"),
        );
        doc.apply(UsdOp::SetRotate {
            edit_target: LayerId::root(),
            path: "/Rig".into(),
            value: [0.0, 90.0, 0.0],
        })
        .unwrap();
        doc.apply(UsdOp::SetTranslate {
            edit_target: LayerId::root(),
            path: "/Rig".into(),
            value: [1.0, 2.0, 3.0],
        })
        .unwrap();
        assert_eq!(
            xform_op_order_tokens(&doc.composed_arc(), &SdfPath::new("/Rig").unwrap()),
            vec!["xformOp:rotateXYZ".to_string(), "xformOp:translate".to_string()],
            "rotate-first authoring lists rotate first"
        );
    }

    /// The referenced-asset clobber case: the prim's composed `xformOpOrder`
    /// already lists ops this edit did not author (an asset's own rotate/scale).
    /// Authoring a translate must APPEND to that composed order — clobbering it
    /// leaves the rotate/scale value attributes orphaned (authored but no longer
    /// applied), which is exactly the silent visual regression this pins.
    #[test]
    fn set_translate_preserves_preexisting_composed_op_order() {
        let scene = "#usda 1.0\ndef Xform \"Part\"\n{\n    double3 xformOp:rotateXYZ = (0, 45, 0)\n    double3 xformOp:scale = (2, 2, 2)\n    uniform token[] xformOpOrder = [\"xformOp:rotateXYZ\", \"xformOp:scale\"]\n}\n";
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(62),
            scene,
            DocumentOrigin::writable_file("/tmp/order_ref.usda"),
        );
        doc.apply(UsdOp::SetTranslate {
            edit_target: LayerId::root(),
            path: "/Part".into(),
            value: [10.0, 0.0, 0.0],
        })
        .unwrap();

        let part = SdfPath::new("/Part").unwrap();
        assert_eq!(
            xform_op_order_tokens(&doc.composed_arc(), &part),
            vec![
                "xformOp:rotateXYZ".to_string(),
                "xformOp:scale".to_string(),
                "xformOp:translate".to_string(),
            ],
            "translate appends AFTER the pre-existing ops, none dropped"
        );
        // The pre-existing op values are untouched.
        assert_eq!(
            doc.data().prim_attribute_value::<[f64; 3]>(&part, "xformOp:rotateXYZ"),
            Some([0.0, 45.0, 0.0])
        );
        assert_eq!(
            doc.data().prim_attribute_value::<[f64; 3]>(&part, "xformOp:scale"),
            Some([2.0, 2.0, 2.0])
        );
    }

    /// Cross-layer variant of the clobber case: the composed order comes from the
    /// BASE layer, and the edit targets the (stronger) RUNTIME layer. Since the
    /// runtime layer's `xformOpOrder` opinion WINS composition wholesale, the op
    /// must materialise base's order PLUS the new op into the runtime layer — a
    /// bare `[rotateXYZ]` runtime order would discard the base translate.
    #[test]
    fn runtime_layer_rotate_materialises_base_order_plus_new_op() {
        let scene = "#usda 1.0\ndef Xform \"Part\"\n{\n    double3 xformOp:translate = (1, 2, 3)\n    uniform token[] xformOpOrder = [\"xformOp:translate\"]\n}\n";
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(63),
            scene,
            DocumentOrigin::writable_file("/tmp/order_rt_layer.usda"),
        );
        doc.apply(UsdOp::SetRotate {
            edit_target: LayerId::runtime(),
            path: "/Part".into(),
            value: [0.0, 30.0, 0.0],
        })
        .unwrap();

        let part = SdfPath::new("/Part").unwrap();
        assert_eq!(
            xform_op_order_tokens(&doc.composed_arc(), &part),
            vec!["xformOp:translate".to_string(), "xformOp:rotateXYZ".to_string()],
            "composed order keeps base's translate and appends the runtime rotate"
        );
        // The base layer's own opinion is untouched (Save serializes base only).
        assert_eq!(
            xform_op_order_tokens(doc.data(), &part),
            vec!["xformOp:translate".to_string()],
            "runtime edit must not rewrite the base layer's xformOpOrder"
        );
        assert_eq!(
            doc.data().prim_attribute_value::<[f64; 3]>(&part, "xformOp:translate"),
            Some([1.0, 2.0, 3.0])
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
    fn set_time_sample_authors_keyframes_and_interpolates() {
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(14),
            "#usda 1.0\ndef Xform \"Mover\"\n{\n}\n",
            DocumentOrigin::writable_file("/tmp/anim.usda"),
        );
        // Two keyframes of the translate, authored as time samples. Each fresh
        // keyframe inverts to a typed `RemoveTimeSample` at the same time.
        let mut inverses = Vec::new();
        for (t, x) in [(0.0_f64, 0.0_f64), (10.0, 10.0)] {
            inverses.push(
                doc.apply(UsdOp::SetTimeSample {
                    edit_target: LayerId::root(),
                    path: "/Mover".into(),
                    name: "xformOp:translate".into(),
                    type_name: "double3".into(),
                    time: t,
                    value: format!("({x}, 0, 0)"),
                })
                .unwrap(),
            );
        }
        assert!(
            matches!(inverses[0], UsdOp::RemoveTimeSample { time, .. } if time == 0.0),
            "a fresh keyframe inverts to a typed RemoveTimeSample"
        );
        assert!(matches!(inverses[1], UsdOp::RemoveTimeSample { time, .. } if time == 10.0));
        let mover = SdfPath::new("/Mover").unwrap();
        // Time-aware read interpolates the authored curve.
        assert_eq!(
            doc.data().prim_attribute_value_at::<[f64; 3]>(&mover, "xformOp:translate", 5.0),
            Some([5.0, 0.0, 0.0]),
            "midpoint must linearly interpolate the two keyframes"
        );
        assert_eq!(
            doc.data().prim_attribute_value_at::<[f64; 3]>(&mover, "xformOp:translate", 10.0),
            Some([10.0, 0.0, 0.0])
        );
        // A sample-only attribute has no `default` opinion.
        assert_eq!(
            doc.data().prim_attribute_value::<[f64; 3]>(&mover, "xformOp:translate"),
            None,
            "time samples must not leak into the default opinion"
        );
        // Undo LIFO: replaying the typed inverses removes both samples, and the
        // attribute round-trips to having no samples at all.
        while let Some(inv) = inverses.pop() {
            doc.apply(inv).unwrap();
        }
        assert_eq!(
            doc.data().prim_attribute_value_at::<[f64; 3]>(&mover, "xformOp:translate", 5.0),
            None,
            "keyframes undone by the typed RemoveTimeSample inverses"
        );
    }

    #[test]
    fn move_prim_renames_and_reparents_with_typed_inverse() {
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(20),
            "#usda 1.0\ndef Xform \"A\"\n{\n}\ndef Xform \"B\"\n{\n}\n",
            DocumentOrigin::writable_file("/tmp/move.usda"),
        );
        let exists = |doc: &UsdDocument, p: &str| doc.data().spec(&SdfPath::new(p).unwrap()).is_some();

        // Reparent /A under /B → /B/A.
        let inverse = doc
            .apply(UsdOp::MovePrim {
                edit_target: LayerId::root(),
                from_path: "/A".into(),
                to_path: "/B/A".into(),
            })
            .unwrap();
        assert!(!exists(&doc, "/A"), "source path is vacated");
        assert!(exists(&doc, "/B/A"), "prim now lives under its new parent");
        // The typed inverse is the exact reverse move.
        assert!(matches!(
            &inverse,
            UsdOp::MovePrim { from_path, to_path, .. } if from_path == "/B/A" && to_path == "/A"
        ));
        doc.apply(inverse).unwrap();
        assert!(exists(&doc, "/A") && !exists(&doc, "/B/A"), "inverse restores the original tree");
    }

    #[test]
    fn set_relationship_authors_targets() {
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(21),
            "#usda 1.0\ndef Xform \"Geom\"\n{\n}\ndef Material \"Red\"\n{\n}\n",
            DocumentOrigin::writable_file("/tmp/rel.usda"),
        );
        doc.apply(UsdOp::SetRelationship {
            edit_target: LayerId::root(),
            path: "/Geom".into(),
            name: "material:binding".into(),
            targets: vec!["/Red".into()],
        })
        .unwrap();
        // The relationship spec is authored under the prim.
        let rel = SdfPath::new("/Geom.material:binding").unwrap();
        assert!(
            doc.data().spec(&rel).is_some(),
            "material:binding relationship authored on /Geom"
        );
    }

    #[test]
    fn set_connection_authors_and_clears_connection_paths() {
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(23),
            "#usda 1.0\ndef Xform \"Load\"\n{\n}\ndef Xform \"Bus\"\n{\n}\n",
            DocumentOrigin::writable_file("/tmp/conn.usda"),
        );
        // Wire the consuming input to a producing output. The attribute spec
        // does not exist yet — the op must create it (create-if-absent).
        doc.apply(UsdOp::SetConnection {
            edit_target: LayerId::root(),
            path: "/Load".into(),
            name: "inputs:voltage".into(),
            type_name: "float".into(),
            sources: vec!["/Bus.outputs:v".into()],
        })
        .unwrap();
        let attr = SdfPath::new("/Load.inputs:voltage").unwrap();
        let conns = |doc: &UsdDocument| -> Vec<String> {
            match doc.data().spec(&attr).and_then(|s| s.get("connectionPaths")) {
                Some(sdf::Value::PathListOp(op)) => op
                    .explicit_items
                    .iter()
                    .map(|p| p.as_str().to_string())
                    .collect(),
                _ => Vec::new(),
            }
        };
        assert_eq!(
            conns(&doc),
            vec!["/Bus.outputs:v".to_string()],
            "connectionPaths authored on the consuming input"
        );
        // Empty `sources` clears the connection (same op, one canonical form).
        doc.apply(UsdOp::SetConnection {
            edit_target: LayerId::root(),
            path: "/Load".into(),
            name: "inputs:voltage".into(),
            type_name: "float".into(),
            sources: vec![],
        })
        .unwrap();
        assert!(
            conns(&doc).is_empty(),
            "empty sources clears the connection"
        );
    }

    #[test]
    fn remove_time_sample_errors_when_absent() {
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(22),
            "#usda 1.0\ndef Xform \"Mover\"\n{\n}\n",
            DocumentOrigin::writable_file("/tmp/rm.usda"),
        );
        // Author one keyframe, then remove the wrong time → error, not silent.
        doc.apply(UsdOp::SetTimeSample {
            edit_target: LayerId::root(),
            path: "/Mover".into(),
            name: "xformOp:translate".into(),
            type_name: "double3".into(),
            time: 0.0,
            value: "(0, 0, 0)".into(),
        })
        .unwrap();
        assert!(doc
            .apply(UsdOp::RemoveTimeSample {
                edit_target: LayerId::root(),
                path: "/Mover".into(),
                name: "xformOp:translate".into(),
                time: 99.0,
            })
            .is_err());
        // Removing the right time succeeds and clears the curve.
        doc.apply(UsdOp::RemoveTimeSample {
            edit_target: LayerId::root(),
            path: "/Mover".into(),
            name: "xformOp:translate".into(),
            time: 0.0,
        })
        .unwrap();
        let mover = SdfPath::new("/Mover").unwrap();
        assert_eq!(
            doc.data().prim_attribute_value_at::<[f64; 3]>(&mover, "xformOp:translate", 0.0),
            None,
            "the only sample was removed, so nothing resolves"
        );
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

    /// Repro for the doc-backed live-edit path (E1b): a runtime-layer
    /// `SetAttribute` that OVERRIDES an existing base attribute on a DEEPLY
    /// NESTED prim must win in the composed view — this is exactly the
    /// `SetObjectProperty`→USD authoring case (e.g. terrain crater `density`).
    #[test]
    fn runtime_set_attribute_overrides_nested_base_attr_in_composed() {
        let base = "#usda 1.0\n(\n    defaultPrim = \"Root\"\n)\ndef Xform \"Root\"\n{\n    def Xform \"Mid\"\n    {\n        def Xform \"Leaf\"\n        {\n            custom float density = 1.5\n        }\n    }\n}\n";
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(40),
            base,
            DocumentOrigin::writable_file("/tmp/nested.usda"),
        );
        doc.apply(UsdOp::SetAttribute {
            edit_target: LayerId::runtime(),
            path: "/Root/Mid/Leaf".into(),
            name: "density".into(),
            type_name: "float".into(),
            value: "4.0".into(),
        })
        .unwrap();
        let composed = doc.composed();
        assert_eq!(
            composed.prim_attribute_value::<f32>(&SdfPath::new("/Root/Mid/Leaf").unwrap(), "density"),
            Some(4.0),
            "runtime override must win in the composed sdf::Data"
        );
        assert!(
            doc.composed_source().contains("density = 4"),
            "composed USDA source must carry the override:\n{}",
            doc.composed_source()
        );
    }

    /// Repro: a runtime-layer `AddPrim` under a NESTED (non-root) parent must
    /// appear in the composed view — the runtime spawn case for a doc-backed scene.
    #[test]
    fn runtime_add_prim_under_nested_parent_in_composed() {
        let base = "#usda 1.0\n(\n    defaultPrim = \"Root\"\n)\ndef Xform \"Root\"\n{\n    def Xform \"Mid\"\n    {\n    }\n}\n";
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(41),
            base,
            DocumentOrigin::writable_file("/tmp/nested2.usda"),
        );
        doc.apply(UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/Root/Mid".into(),
            name: "Probe".into(),
            type_name: Some("Cube".into()),
            reference: None,
        })
        .unwrap();
        let composed = doc.composed();
        assert_eq!(
            composed.prim_type_name(&SdfPath::new("/Root/Mid/Probe").unwrap()).as_deref(),
            Some("Cube"),
            "runtime child under a nested parent must appear in the composed view"
        );
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

    /// Variability and `custom` come from the schema, not the call site — so the
    /// SAME `SetAttribute` op yields `uniform` for one attribute and `varying` for
    /// another, and no caller has to know which. This is the fix for `info:id` and
    /// `physics:axis` having been authored `varying`: they are `uniform` in their
    /// schemas, nothing at the call site knew that, and the value silently diverged.
    #[test]
    fn set_attribute_authors_variability_and_custom_from_the_schema() {
        let mut host = DocumentHost::new(UsdDocument::with_origin(
            DocumentId::new(41),
            "#usda 1.0\ndef Shader \"Surface\"\n{\n}\n",
            DocumentOrigin::writable_file("/tmp/var.usda"),
        ));
        let set = |host: &mut DocumentHost<UsdDocument>, name: &str, ty: &str, value: &str| {
            host.apply(Mutation::local(UsdOp::SetAttribute {
                edit_target: LayerId::root(),
                path: "/Surface".into(),
                name: name.into(),
                type_name: ty.into(),
                value: value.into(),
            }))
            .unwrap();
        };

        // Core USD, declared `uniform` by UsdShadeShader.
        set(&mut host, "info:id", "token", "\"UsdPreviewSurface\"");
        // Ours, declared `uniform` by luncoSchema.
        set(&mut host, "lunco:cameraMode", "token", "\"orbit\"");
        // Ours, declared `varying` by luncoSchema.
        set(&mut host, "lunco:env:exposureEv100", "float", "12.5");
        // Ours, declared by NO schema — a per-model Modelica param, genuinely custom.
        set(&mut host, "lunco:voltage", "float", "28.0");

        let src = host.document().source();
        assert!(
            src.contains("uniform token info:id"),
            "info:id is uniform per UsdShadeShader: {src}"
        );
        assert!(
            src.contains("uniform token lunco:cameraMode"),
            "lunco:cameraMode is uniform per luncoSchema: {src}"
        );
        assert!(
            src.contains("float lunco:env:exposureEv100") && !src.contains("uniform float lunco:env"),
            "lunco:env:exposureEv100 is varying per luncoSchema: {src}"
        );
        assert!(
            src.contains("custom float lunco:voltage"),
            "a lunco: attr no schema declares must be authored `custom`: {src}"
        );
        // A core attr we have no schema for must NOT be claimed custom — that would
        // be a lie about a perfectly ordinary schema property.
        assert!(
            !src.contains("custom token info:id"),
            "info:id is a schema property, not custom: {src}"
        );
        assert!(reparses_cleanly(host.document()), "authored variability must reparse");
    }

    #[test]
    fn set_api_schemas_authors_and_undoes() {
        let mut host = DocumentHost::new(UsdDocument::with_origin(
            DocumentId::new(40),
            "#usda 1.0\ndef Xform \"Body\"\n{\n}\n",
            DocumentOrigin::writable_file("/tmp/api.usda"),
        ));
        host.apply(Mutation::local(UsdOp::SetApiSchemas {
            edit_target: LayerId::root(),
            path: "/Body".into(),
            schemas: vec!["PhysicsRigidBodyAPI".into(), "PhysicsCollisionAPI".into()],
        }))
        .unwrap();
        assert!(
            host.document().source().contains("PhysicsRigidBodyAPI")
                && host.document().source().contains("PhysicsCollisionAPI"),
            "apiSchemas authored: {}",
            host.document().source()
        );
        assert!(reparses_cleanly(host.document()), "authored apiSchemas must reparse cleanly");
        host.undo().unwrap();
        assert!(
            !host.document().source().contains("PhysicsRigidBodyAPI"),
            "undo removes the schemas: {}",
            host.document().source()
        );
    }

    #[test]
    fn set_active_false_then_undo_restores_absence() {
        // The subtle one: undoing a deactivation must NOT author `active = true`
        // (a `!active` inverse would). It restores the prior *unauthored* opinion.
        let mut host = DocumentHost::new(UsdDocument::with_origin(
            DocumentId::new(41),
            "#usda 1.0\ndef Xform \"Part\"\n{\n}\n",
            DocumentOrigin::writable_file("/tmp/active.usda"),
        ));
        host.apply(Mutation::local(UsdOp::SetActive {
            edit_target: LayerId::root(),
            path: "/Part".into(),
            active: false,
        }))
        .unwrap();
        assert!(
            host.document().source().contains("active = false"),
            "deactivation authored: {}",
            host.document().source()
        );
        host.undo().unwrap();
        assert!(
            !host.document().source().contains("active"),
            "undo restores the unauthored (neither true nor false) opinion: {}",
            host.document().source()
        );
    }

    #[test]
    fn set_variant_selection_preserves_sibling_set() {
        // A prim carrying two variant sets: selecting one must not drop the other.
        let src = "#usda 1.0\ndef Xform \"Rover\" (\n    variants = {\n        string color = \"red\"\n    }\n)\n{\n}\n";
        let mut doc = UsdDocument::with_origin(
            DocumentId::new(42),
            src,
            DocumentOrigin::writable_file("/tmp/var.usda"),
        );
        doc.apply(UsdOp::SetVariantSelection {
            edit_target: LayerId::root(),
            path: "/Rover".into(),
            variant_set: "drivetrain".into(),
            variant: "physical".into(),
        })
        .unwrap();
        let s = doc.source();
        assert!(s.contains("drivetrain") && s.contains("physical"), "new selection: {s}");
        assert!(
            s.contains("color") && s.contains("red"),
            "sibling variant selection preserved (read-modify-write): {s}"
        );
        assert!(reparses_cleanly(&doc), "authored variant selection must reparse cleanly: {s}");
    }

    #[test]
    fn set_payload_authors_and_undoes() {
        let mut host = DocumentHost::new(UsdDocument::with_origin(
            DocumentId::new(43),
            "#usda 1.0\ndef Xform \"Heavy\"\n{\n}\n",
            DocumentOrigin::writable_file("/tmp/pl.usda"),
        ));
        host.apply(Mutation::local(UsdOp::SetPayload {
            edit_target: LayerId::root(),
            path: "/Heavy".into(),
            // RAW path, no `@…@` (those are USDA delimiters the writer adds) — same
            // contract as AddPrim's reference. The `@…@` form serializes to `@@…@@`.
            asset_paths: vec!["meshes/hull.usdc".into()],
        }))
        .unwrap();
        assert!(
            host.document().source().contains("hull.usdc"),
            "payload authored: {}",
            host.document().source()
        );
        // The substring check above passes even for a malformed `@@…@@` path; THIS
        // is what proves the payload serialized to parseable USDA.
        assert!(
            reparses_cleanly(host.document()),
            "authored payload must reparse cleanly: {}",
            host.document().source()
        );
        host.undo().unwrap();
        assert!(
            !host.document().source().contains("hull.usdc"),
            "undo clears the payload: {}",
            host.document().source()
        );
    }
}
