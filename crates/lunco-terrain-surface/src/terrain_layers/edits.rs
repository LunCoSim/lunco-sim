//! Built-in **edits** layer — the single consolidated runtime-edit layer.
//!
//! Live tool edits (dig, raise, flatten) do NOT each spawn a new terrain layer —
//! that would grow the stack unboundedly. Instead every edit folds into **one**
//! [`EditsLayer`] on the [`TerrainLayerStack`](super::TerrainLayerStack): the layer
//! holds an ordered list of [`EditKind`]s, each tagged with a stable
//! [`LayerId`](super::LayerId), and contributes them as ONE **analytic** height
//! modifier on the terrain's [`SurfaceOracle`](crate::oracle::SurfaceOracle). The
//! visual tiles and the heightfield collider both sample the one composed surface,
//! so the edit the rover drives is exactly the edit you see — at whatever
//! resolution each consumer samples. Edits are addressable — remove one by its id
//! (undo a specific stroke); the layer re-composes via `Changed<TerrainLayerStack>`.
//!
//! The granular history (which edit, in what order, how to invert) is designed to
//! live in the twin journal — this layer is the *projection* of that op stream. See
//! `docs/architecture/command-journal.md`. Until the journal wiring lands, edits are
//! appended directly with an explicit id.

use std::any::Any;
use std::sync::Arc;

use lunco_terrain_core::{BrushModifier, FlattenModifier, HeightModifier};

use super::{LayerAttrSource, LayerId, TerrainLayer};
use crate::oracle::HeightContribution;

/// One concrete terrain edit. A serializable, `Copy` value (not an `Arc<dyn …>`) so
/// the edits layer can be cloned + rebuilt cheaply and — later — journaled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EditKind {
    /// Smooth radial brush: `amplitude` m at the centre (− digs, + raises), 0 at `radius`.
    Brush { center: [f64; 2], radius: f64, amplitude: f64 },
    /// Flatten toward `target_y` within `radius`, blending back at the edge.
    Flatten { center: [f64; 2], radius: f64, target_y: f64 },
}

impl EditKind {
    /// Apply this edit to the accumulated height at `(x, z)`.
    #[inline]
    fn apply(&self, x: f64, z: f64, h_in: f64) -> f64 {
        match *self {
            EditKind::Brush { center, radius, amplitude } => {
                BrushModifier::new(center, radius, amplitude).apply(x, z, h_in)
            }
            EditKind::Flatten { center, radius, target_y } => {
                FlattenModifier::new(center, radius, target_y).apply(x, z, h_in)
            }
        }
    }
}

/// The single consolidated runtime-edits layer: an ordered list of identified edits,
/// stamped in one pass. Immutable in the stack (rebuilt via [`with_edit`](Self::with_edit)
/// / [`without`](Self::without)) so change detection and the off-thread bake see a
/// clean swap.
#[derive(Clone, Default)]
pub struct EditsLayer {
    edits: Vec<(LayerId, EditKind)>,
}

impl EditsLayer {
    /// Number of edits held.
    pub fn len(&self) -> usize {
        self.edits.len()
    }

    /// Whether there are no edits (the layer should then be dropped from the stack).
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// A copy with `kind` appended under `id` (edits fold in append order).
    pub fn with_edit(&self, id: LayerId, kind: EditKind) -> Self {
        let mut edits = self.edits.clone();
        edits.push((id, kind));
        EditsLayer { edits }
    }

    /// A copy with the edit identified by `id` removed; `None` if `id` isn't present.
    pub fn without(&self, id: &LayerId) -> Option<Self> {
        if !self.edits.iter().any(|(eid, _)| eid == id) {
            return None;
        }
        Some(EditsLayer { edits: self.edits.iter().filter(|(eid, _)| eid != id).cloned().collect() })
    }

    /// Build from a list of identified edits, in fold order — the **projection** of
    /// the terrain document's edit prims (the parser feeds this).
    pub fn from_edits(edits: Vec<(LayerId, EditKind)>) -> Self {
        EditsLayer { edits }
    }
}

/// The single **packed** attribute that carries an edit on its prim:
/// `lunco:edit = "<kind> <cx> <cz> <radius> <param>"`. One prim per edit (USD
/// authoring tier); they fold into the single [`EditsLayer`] (runtime tier). Packing
/// keeps an edit to **two ops** (`AddPrim` + one `SetAttribute`) so undoing the
/// attribute alone already drops the edit from the projection — no change-set needed
/// for a clean single-step undo — and needs no array-typed attribute reader.
pub const EDIT_ATTR: &str = "lunco:edit";

/// Parse an edit prim's packed [`EDIT_ATTR`] into an identified [`EditKind`]. `id` is
/// the prim's stable identity (its path). `None` if the attribute is absent (not an
/// edit prim) or malformed — so the layer walker can try normal layer parsing instead.
pub fn parse_edit(id: LayerId, a: &dyn LayerAttrSource) -> Option<(LayerId, EditKind)> {
    let packed = a.get_string(EDIT_ATTR)?;
    let mut it = packed.split_whitespace();
    let kind = it.next()?;
    let cx: f64 = it.next()?.parse().ok()?;
    let cz: f64 = it.next()?.parse().ok()?;
    let radius: f64 = it.next()?.parse().ok()?;
    let param: f64 = it.next()?.parse().ok()?;
    let edit = match kind {
        "brush" => EditKind::Brush { center: [cx, cz], radius, amplitude: param },
        "flatten" => EditKind::Flatten { center: [cx, cz], radius, target_y: param },
        _ => return None,
    };
    Some((id, edit))
}

/// The single `SetAttribute` write that authors an edit — `(name, type_name, value)`,
/// `value` a USD-compliant string literal — so the authoring tier and [`parse_edit`]
/// share one schema. Pairs with an `AddPrim` for the edit prim.
pub fn edit_attr_write(kind: &EditKind) -> (&'static str, &'static str, String) {
    let (k, c, r, p) = match *kind {
        EditKind::Brush { center, radius, amplitude } => ("brush", center, radius, amplitude),
        EditKind::Flatten { center, radius, target_y } => ("flatten", center, radius, target_y),
    };
    (EDIT_ATTR, "string", format!("\"{k} {} {} {r} {p}\"", c[0], c[1]))
}

/// The edits list as one analytic [`HeightModifier`]: folds every edit in append
/// order at each sampled point (additive brush adds, flatten blends from the
/// accumulated height).
impl HeightModifier for EditsLayer {
    fn apply(&self, x: f64, z: f64, h_in: f64) -> f64 {
        let mut h = h_in;
        for (_, kind) in &self.edits {
            h = kind.apply(x, z, h);
        }
        h
    }
}

impl TerrainLayer for EditsLayer {
    fn id(&self) -> &'static str {
        super::EDITS_LAYER_ID
    }

    fn height_modifier(&self, _half_extent: f32) -> Option<HeightContribution> {
        if self.edits.is_empty() {
            return None;
        }
        // Content key: every edit's identity + parameters, in fold order.
        let mut key = lunco_precompute::Fnv1a::new();
        for (id, kind) in &self.edits {
            for b in id.as_str().as_bytes() {
                key.write_u64(*b as u64);
            }
            match *kind {
                EditKind::Brush { center, radius, amplitude } => {
                    key.write_u64(1);
                    key.write_u64(center[0].to_bits());
                    key.write_u64(center[1].to_bits());
                    key.write_u64(radius.to_bits());
                    key.write_u64(amplitude.to_bits());
                }
                EditKind::Flatten { center, radius, target_y } => {
                    key.write_u64(2);
                    key.write_u64(center[0].to_bits());
                    key.write_u64(center[1].to_bits());
                    key.write_u64(radius.to_bits());
                    key.write_u64(target_y.to_bits());
                }
            }
        }
        Some(HeightContribution { modifier: Arc::new(self.clone()), content_key: key.finish() })
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }
}
