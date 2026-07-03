//! Mobility — the source-agnostic **declared** motion class of a body.
//!
//! A body's mobility is *structure*: whether the source (USD physics schema, a
//! rhai script, a Modelica model) declares it as world-fixed, externally posed,
//! or force-integrated. The live avian [`RigidBody`](avian3d::prelude::RigidBody)
//! is the *state* projected from it — and that projection is not always 1:1: a
//! `Dynamic`-declared body spawns transiently `Kinematic` while its joints settle
//! (`ShouldBeDynamic`), and an animated body is demoted to `Kinematic` so the
//! sampler owns its pose. Recording the *declared* class separately keeps that
//! stable intent queryable even while the engine body type is mid-transition.
//!
//! `lunco-core` owns the classifier (no avian dependency, so every source and
//! reader can set it downward); the avian-aware crate projects it onto a
//! `RigidBody`. This is the same substrate shape as [`crate::ports`]: a neutral
//! declaration below every participant, projected by the engine above.

use bevy::prelude::*;

/// The declared motion class of a physics body — set by whichever source spawns
/// it (USD schema, script, Modelica), projected onto the live `RigidBody`.
///
/// Distinct from the engine body type: this is the *intent* ("this rover IS a
/// dynamic body"), stable across the transient `Kinematic` a `Dynamic` body wears
/// while joints settle. Consumers that want "what is this meant to be" (network
/// prediction eligibility, UI, gravity gating) should read this, not the
/// possibly-mid-transition `RigidBody`.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[reflect(Component)]
pub enum Mobility {
    /// World-fixed: never integrated, never posed by animation. Terrain, fixed
    /// props, geofence volumes.
    Static,
    /// Externally posed (animation / script / gizmo), collides and drives joints
    /// but is not integrated from forces.
    Kinematic,
    /// Force-integrated by the solver — the free rigid bodies (rovers, landers,
    /// dropped objects).
    Dynamic,
}
