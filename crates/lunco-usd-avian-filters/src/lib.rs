//! OpenUSD collision filtering on Avian's collision-layer and hook boundaries.
//!
//! This package is intentionally separate from the USD rigid-body, collider,
//! and joint projector. It owns only the generic collision-filter mechanisms:
//! authored `PhysicsFilteredPairsAPI`/`PhysicsCollisionGroup` interpretation,
//! transient joint pair suppression, and Avian contact-hook registration.
pub mod collision_groups;
pub mod filtered_pairs;
