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

use lunco_terrain_core::{BrushModifier, Crater, FlattenModifier, HeightModifier, CRATER_REACH};

use super::{LayerAttrSource, LayerId, TerrainLayer};
use crate::oracle::HeightContribution;

/// One concrete terrain edit. A serializable, `Copy` value (not an `Arc<dyn …>`) so
/// the edits layer can be cloned + rebuilt cheaply and — later — journaled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EditKind {
    /// Smooth radial brush: `amplitude` m at the centre (− digs, + raises), 0 at `radius`.
    Brush {
        center: [f64; 2],
        radius: f64,
        amplitude: f64,
    },
    /// Flatten toward `target_y` within `radius`, blending back at the edge.
    Flatten {
        center: [f64; 2],
        radius: f64,
        target_y: f64,
    },
    /// One hand-placed impact crater: rim radius `radius` m, bowl `depth` m below
    /// the datum. Same analytic profile as the procedural crater field (fresh
    /// morphology, quasi-paraboloid bowl + rim lip + ejecta apron), so a manual
    /// crater is indistinguishable from an authored-field one in mesh, collider
    /// and derived maps alike.
    Crater {
        center: [f64; 2],
        radius: f64,
        depth: f64,
    },
}

/// The analytic shape a manual crater edit stamps — fresh-ish morphology with
/// realistic proportions (rim lip ≈ 18 % of depth, gently rounded).
fn manual_crater(center: [f64; 2], radius: f64, depth: f64) -> Crater {
    Crater {
        center,
        radius,
        depth,
        rim_height: 0.18 * depth,
        softness: 0.06,
        bowl_power: 2.2,
    }
}

impl EditKind {
    /// The edit's world footprint as `[min_x, min_z, max_x, max_z]` (terrain-local
    /// metres) — every edit is radial; a crater's ejecta apron reaches past its rim
    /// radius, so its footprint uses the crater reach.
    /// Drives the incremental region re-bake (only tiles overlapping this re-bake).
    pub fn aabb(&self) -> [f64; 4] {
        let (center, radius) = match *self {
            EditKind::Brush { center, radius, .. } => (center, radius),
            EditKind::Flatten { center, radius, .. } => (center, radius),
            EditKind::Crater { center, radius, .. } => (center, radius * CRATER_REACH),
        };
        [
            center[0] - radius,
            center[1] - radius,
            center[0] + radius,
            center[1] + radius,
        ]
    }

    /// Apply this edit to the accumulated height at `(x, z)`, band-limited for a
    /// consumer sampling every `min_wavelength` metres (0 = exact).
    #[inline]
    fn apply(&self, x: f64, z: f64, h_in: f64, min_wavelength: f64) -> f64 {
        match *self {
            EditKind::Brush {
                center,
                radius,
                amplitude,
            } => BrushModifier::new(center, radius, amplitude).apply(x, z, h_in),
            EditKind::Flatten {
                center,
                radius,
                target_y,
            } => FlattenModifier::new(center, radius, target_y).apply(x, z, h_in),
            EditKind::Crater {
                center,
                radius,
                depth,
            } => h_in + manual_crater(center, radius, depth).delta_at_limited(x, z, min_wavelength),
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
    /// Sampling wavelength this instance is band-limited for (0 = exact) — set
    /// by [`HeightModifier::with_min_wavelength`] on the per-consumer clone;
    /// NOT part of the layer's identity/content key.
    min_wavelength: f64,
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
        EditsLayer {
            edits,
            min_wavelength: self.min_wavelength,
        }
    }

    /// A copy with the edit identified by `id` removed; `None` if `id` isn't present.
    pub fn without(&self, id: &LayerId) -> Option<Self> {
        if !self.edits.iter().any(|(eid, _)| eid == id) {
            return None;
        }
        Some(EditsLayer {
            edits: self
                .edits
                .iter()
                .filter(|(eid, _)| eid != id)
                .cloned()
                .collect(),
            min_wavelength: self.min_wavelength,
        })
    }

    /// Build from a list of identified edits — the **projection** of the terrain
    /// document's edit prims (the parser feeds this). Fold order is CANONICALIZED to
    /// creation order (the monotonic `edit_<n>` suffix, then the full id as a
    /// tiebreak): the parser walks prims in storage order, which is neither authored
    /// nor stable across parses, and edits do not commute (brush → flatten ≠
    /// flatten → brush). Canonical order also keeps the layer's `content_key` a pure
    /// function of the document, so tile/derived-map caches hit across relaunches.
    pub fn from_edits(mut edits: Vec<(LayerId, EditKind)>) -> Self {
        fn trailing_num(id: &LayerId) -> u64 {
            let s = id.as_str();
            let digits = s
                .rfind(|c: char| !c.is_ascii_digit())
                .map_or(s, |i| &s[i + 1..]);
            digits.parse().unwrap_or(u64::MAX)
        }
        edits.sort_by(|(a, _), (b, _)| {
            trailing_num(a)
                .cmp(&trailing_num(b))
                .then_with(|| a.as_str().cmp(b.as_str()))
        });
        EditsLayer {
            edits,
            min_wavelength: 0.0,
        }
    }

    /// The world footprint of the edit identified by `id`, or `None` if it isn't in
    /// this layer. Used to scope an undo/remove to only the tiles it touched.
    pub fn edit_bounds(&self, id: &LayerId) -> Option<[f64; 4]> {
        self.edits
            .iter()
            .find(|(eid, _)| eid == id)
            .map(|(_, kind)| kind.aabb())
    }
}

/// The `LunCoTerrainEditAPI` properties of ONE edit, by **logical** name — this crate
/// is USD-free, so the USD adapter binds these to `lunco:edit:*`.
///
/// They used to be PACKED into one string (`lunco:edit = "crater 350 350 45 18"`).
/// That is legal USD and it is not USD: a private encoding inside a string is opaque
/// to the type system — nothing validates it, `allowedTokens` cannot constrain the
/// kind, and no other DCC can read it. Packing bought undo atomicity ("one attribute =
/// one undo step"), but `apply_ops_as_change_set` commits N ops as a single labelled
/// undo step, so the reason had outlived the encoding.
pub const EDIT_KIND: &str = "kind";
pub const EDIT_CENTER: &str = "center";
pub const EDIT_RADIUS: &str = "radius";
pub const EDIT_AMOUNT: &str = "amount";

/// Parse an edit prim's `LunCoTerrainEditAPI` attributes into an identified
/// [`EditKind`]. `id` is the prim's stable identity (its path). `None` if the prim
/// authors no edit kind (not an edit prim) or the kind is unknown — so the layer
/// walker can try normal layer parsing instead.
pub fn parse_edit(id: LayerId, a: &dyn LayerAttrSource) -> Option<(LayerId, EditKind)> {
    let kind = a.get_string(EDIT_KIND)?;
    let center = a.get_vec2(EDIT_CENTER)?;
    let radius = a.get_f64(EDIT_RADIUS)?;
    let amount = a.get_f64(EDIT_AMOUNT)?;
    let edit = match kind.as_str() {
        "brush" => EditKind::Brush {
            center,
            radius,
            amplitude: amount,
        },
        "flatten" => EditKind::Flatten {
            center,
            radius,
            target_y: amount,
        },
        "crater" => EditKind::Crater {
            center,
            radius,
            depth: amount,
        },
        _ => return None,
    };
    Some((id, edit))
}

/// The `SetAttribute` writes that author one edit — `(logical_name, type_name, value)`,
/// `value` a USD literal — so the authoring tier and [`parse_edit`] share one schema.
/// The USD-aware caller namespaces each name and commits them as ONE change set, so an
/// edit is still a single undo step.
pub fn edit_attr_writes(kind: &EditKind) -> Vec<(&'static str, &'static str, String)> {
    let (k, c, r, p) = match *kind {
        EditKind::Brush {
            center,
            radius,
            amplitude,
        } => ("brush", center, radius, amplitude),
        EditKind::Flatten {
            center,
            radius,
            target_y,
        } => ("flatten", center, radius, target_y),
        EditKind::Crater {
            center,
            radius,
            depth,
        } => ("crater", center, radius, depth),
    };
    vec![
        (EDIT_KIND, "token", format!("\"{k}\"")),
        (EDIT_CENTER, "double2", format!("({}, {})", c[0], c[1])),
        (EDIT_RADIUS, "double", r.to_string()),
        (EDIT_AMOUNT, "double", p.to_string()),
    ]
}

/// The edits list as one analytic [`HeightModifier`]: folds every edit in append
/// order at each sampled point (additive brush adds, flatten blends from the
/// accumulated height).
impl HeightModifier for EditsLayer {
    fn apply(&self, x: f64, z: f64, h_in: f64) -> f64 {
        let mut h = h_in;
        for (_, kind) in &self.edits {
            h = kind.apply(x, z, h, self.min_wavelength);
        }
        h
    }

    /// Manual craters band-limit exactly like field craters: a copy tagged with
    /// the consumer's sampling wavelength so a coarse far tile fades/softens a
    /// small placed crater instead of aliasing its rim.
    fn with_min_wavelength(&self, min_wavelength: f64) -> Option<Arc<dyn HeightModifier>> {
        if !self
            .edits
            .iter()
            .any(|(_, k)| matches!(k, EditKind::Crater { .. }))
        {
            return None; // brush/flatten are resolution-independent — use as-is
        }
        Some(Arc::new(EditsLayer {
            edits: self.edits.clone(),
            min_wavelength,
        }))
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
                EditKind::Brush {
                    center,
                    radius,
                    amplitude,
                } => {
                    key.write_u64(1);
                    key.write_u64(center[0].to_bits());
                    key.write_u64(center[1].to_bits());
                    key.write_u64(radius.to_bits());
                    key.write_u64(amplitude.to_bits());
                }
                EditKind::Flatten {
                    center,
                    radius,
                    target_y,
                } => {
                    key.write_u64(2);
                    key.write_u64(center[0].to_bits());
                    key.write_u64(center[1].to_bits());
                    key.write_u64(radius.to_bits());
                    key.write_u64(target_y.to_bits());
                }
                EditKind::Crater {
                    center,
                    radius,
                    depth,
                } => {
                    key.write_u64(3);
                    key.write_u64(center[0].to_bits());
                    key.write_u64(center[1].to_bits());
                    key.write_u64(radius.to_bits());
                    key.write_u64(depth.to_bits());
                }
            }
        }
        Some(HeightContribution {
            modifier: Arc::new(self.clone()),
            content_key: key.finish(),
        })
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }
}
