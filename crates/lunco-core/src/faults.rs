//! Runtime simulation faults shared by the engine layers.
//!
//! A malformed physics state is a simulation failure, not a UI warning. The
//! producer that first observes it records the structured fact here; physics,
//! sensors, camera drivers, and recording can then stop at their own boundary
//! without depending on one another.

use bevy::prelude::*;

/// The first terminal runtime failure in the current session.
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

/// Session-scoped terminal runtime faults.
///
/// This resource is intentionally first-fault-wins. Later systems must not
/// overwrite the first causal boundary with a downstream NaN or raycast error;
/// they may still add their own logs, but the verdict remains attributable.
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

    /// Explain why scene mutation is no longer valid in this session.
    ///
    /// A terminal runtime fault is session-scoped and first-fault-wins. Scene
    /// replacement after that point would tear down the evidence that caused
    /// the fault while leaving the rest of the session in an unknown state, so
    /// every scene/workspace entry point uses this one policy boundary.
    pub fn scene_mutation_rejection(&self, operation: &str) -> Option<String> {
        let fault = self.first.as_ref()?;
        Some(format!(
            "cannot {operation}: session has terminal runtime fault `{}` on {}: {}; restart the process",
            fault.kind, fault.subject, fault.detail
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_mutations_are_allowed_until_a_terminal_fault_exists() {
        let mut faults = RuntimeFaults::default();
        assert_eq!(faults.scene_mutation_rejection("load a scene"), None);

        assert!(faults.raise("physics-invalid", None, "rover", "non-finite pose"));
        let rejection = faults
            .scene_mutation_rejection("load a scene")
            .expect("an active terminal fault must reject scene mutation");
        assert!(rejection.contains("physics-invalid"));
        assert!(rejection.contains("restart the process"));
    }
}
