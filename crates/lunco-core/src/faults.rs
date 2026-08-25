//! Runtime simulation faults shared by the engine layers.
//!
//! A malformed physics state is a simulation failure, not a UI warning. The
//! producer that first observes it records the structured fact here; physics,
//! sensors, camera drivers, and recording can then stop at their own boundary
//! without depending on one another. The record belongs to the active scene
//! and is reset by the scene-teardown owner before the replacement scene runs.

use bevy::prelude::*;

/// Severity of a scene/runtime diagnostic that is not itself a terminal
/// simulation fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

impl DiagnosticSeverity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// One owning-boundary diagnostic. Unlike a log line, this remains available
/// to API/UI/lint consumers until the scene teardown boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDiagnostic {
    /// Stable producer-owned rule/code.
    pub code: String,
    pub severity: DiagnosticSeverity,
    /// Subsystem that owns the failed contract.
    pub producer: String,
    /// Authored prim or runtime subject, when known.
    pub subject: String,
    pub message: String,
}

/// Scene-scoped non-terminal diagnostics. Producers replace their own code so
/// a repaired scene clears its highlight without erasing another subsystem's
/// finding. The resource is intentionally separate from [`RuntimeFaults`]: a
/// missing camera contract should reject presentation, but it is not a NaN
/// physics failure.
#[derive(Resource, Debug, Default, Clone)]
pub struct RuntimeDiagnostics {
    pub findings: Vec<RuntimeDiagnostic>,
}

impl RuntimeDiagnostics {
    /// Replace all findings owned by `producer` and return the changed state.
    pub fn replace_producer(
        &mut self,
        producer: impl Into<String>,
        findings: impl IntoIterator<Item = RuntimeDiagnostic>,
    ) {
        let producer = producer.into();
        self.findings.retain(|finding| finding.producer != producer);
        self.findings.extend(
            findings
                .into_iter()
                .filter(|finding| finding.producer == producer),
        );
    }

    pub fn clear(&mut self) {
        self.findings.clear();
    }
}

/// Scene lifecycle owner for non-terminal diagnostics.
pub fn clear_runtime_diagnostics(mut diagnostics: ResMut<RuntimeDiagnostics>) {
    diagnostics.clear();
}

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

    #[test]
    fn diagnostics_replace_only_their_own_producer() {
        let mut diagnostics = RuntimeDiagnostics::default();
        diagnostics.replace_producer(
            "camera",
            [RuntimeDiagnostic {
                code: "camera-contract".to_string(),
                severity: DiagnosticSeverity::Error,
                producer: "camera".to_string(),
                subject: "scene-camera".to_string(),
                message: "camera missing".to_string(),
            }],
        );
        diagnostics.replace_producer(
            "physics",
            [RuntimeDiagnostic {
                code: "physics-frame".to_string(),
                severity: DiagnosticSeverity::Error,
                producer: "physics".to_string(),
                subject: "world".to_string(),
                message: "frame missing".to_string(),
            }],
        );

        diagnostics.replace_producer("camera", std::iter::empty());
        assert_eq!(diagnostics.findings.len(), 1);
        assert_eq!(diagnostics.findings[0].producer, "physics");

        diagnostics.clear();
        assert!(diagnostics.findings.is_empty());
    }
}
