//! Render-free geometry substrate used by USD projection.
//!
//! This package owns reusable NURBS evaluation, trimmed-domain tessellation,
//! and rotation-minimizing curve sweeps. It produces mesh data and contains no
//! USD stage reader, ECS projection policy, physics, or renderer. Keeping the
//! numeric/mesh-heavy code behind a package boundary means changing an
//! evaluator does not recompile the USD stage loader and its runtime systems.

pub mod curve_sweep;
pub mod nurbs;
pub mod trim;
