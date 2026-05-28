//! Architectural marker components for the big_space integration.
//!
//! These markers carry semantic intent that the raw big_space components
//! (`Grid`, `CellCoord`) don't express. They're the contract between the
//! coords helpers, the SOI plugin, the gizmo system, and the loaders.

use bevy::prelude::*;
use big_space::prelude::CellCoord;

/// A spatial entity that moves as a single unit — rover, ball, vessel, avatar,
/// terrain tile, scene-level light.
///
/// **Invariant**: a `GridAnchor` is a direct child of a big_space `Grid`. It
/// carries `CellCoord` (auto-inserted via `#[require]`) and its own
/// `Transform`. Its descendants are plain-`Transform` children whose
/// `GlobalTransform` propagates via big_space's `propagate_low_precision`.
///
/// Selection, dragging, possession, and SOI migration all operate on
/// `GridAnchor` entities — never on their descendants.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[require(CellCoord)]
#[reflect(Component)]
pub struct GridAnchor;

/// A `GridAnchor` that participates in cross-Grid SOI migration.
///
/// Rovers, spacecraft, free-flying probes — anything whose dominant
/// gravitational body can change at runtime. Static terrain and decoration
/// are explicitly *not* `SoiMigrant`.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct SoiMigrant;
