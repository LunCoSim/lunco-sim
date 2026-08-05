//! Runtime simulation faults shared by the engine layers.
//!
//! A malformed physics state is a simulation failure, not a UI warning. The
//! producer that first observes it records the structured fact here; physics,
//! sensors, camera drivers, and recording can then stop at their own boundary
//! without depending on one another. The record belongs to the active scene
//! and is reset by the scene-teardown owner before the replacement scene runs.

use bevy::prelude::*;

/// The first terminal runtime failure in the current scene.
#[derive(Debug, Clone)]
pub struct RuntimeFault {
    /// Stable category used by diagnostics and recording verdicts.
    pub kind: &'static str,
    /// Entity that first exposed the invalid state, when there is one.
    pub entity: Option<Entity>,
    /// Human-readable prim/entity name or subsystem label.
    pub subject: String,
    /// State values and the owning invariant that failed.
    pub detail: String,
}

/// Scene-scoped terminal runtime faults.
///
/// This resource is intentionally first-fault-wins within one scene. Later
/// systems must not overwrite the first causal boundary with a downstream NaN
/// or raycast error; they may still add their own logs, but the verdict remains
/// attributable. The scene lifecycle owner clears it before a replacement
/// scene is allowed to integrate.
#[derive(Resource, Debug, Default, Clone)]
pub struct RuntimeFaults {
    pub first: Option<RuntimeFault>,
}

impl RuntimeFaults {
    /// Record the first fault and return whether this call won the race.
    pub fn raise(
        &mut self,
        kind: &'static str,
        entity: Option<Entity>,
        subject: impl Into<String>,
        detail: impl Into<String>,
    ) -> bool {
        if self.first.is_some() {
            return false;
        }
        self.first = Some(RuntimeFault {
            kind,
            entity,
            subject: subject.into(),
            detail: detail.into(),
        });
        true
    }

    #[inline]
    pub fn active(&self) -> bool {
        self.first.is_some()
    }

    /// End the current scene's faulted runtime state at its teardown boundary.
    pub fn clear(&mut self) {
        self.first = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fault_can_be_cleared_for_the_next_scene() {
        let mut faults = RuntimeFaults::default();
        assert!(faults.raise("physics-invalid", None, "rover", "non-finite pose"));
        assert!(faults.active());

        faults.clear();
        assert!(!faults.active());
    }
}
