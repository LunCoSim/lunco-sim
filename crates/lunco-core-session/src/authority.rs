//! Generic control-authority transitions.
//!
//! [`SessionRegistry`] is the single owner table. This module provides the
//! reusable transition around it; avatar possession and other controller
//! policies compose the transition with their own producer binding.

use crate::session::{SessionRbac, SessionRegistry};
use bevy::prelude::*;
use lunco_command_contracts::SessionId;
use std::fmt;

/// The result of a control-authority transition.
///
/// The event carries stable global ids so backend adapters can react without
/// importing avatar policy. `target` is the newly claimed endpoint, or `None`
/// for a release.
#[derive(Event, Clone, Debug, PartialEq, Eq)]
pub struct ControlAuthorityChanged {
    /// Session whose authority changed.
    pub session: SessionId,
    /// Endpoint newly claimed by the session, if this was a claim.
    pub target: Option<u64>,
    /// Endpoints that must stop accepting the previous owner's control.
    pub released: Vec<u64>,
}

/// Why a session could not claim or release an endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlAuthorityError {
    /// The endpoint is owned by another session and the authored takeover
    /// policy did not approve the transition.
    NotAllowed {
        /// Endpoint being claimed or released.
        target: u64,
        /// Current owner, when the registry has one.
        owner: Option<SessionId>,
    },
}

impl fmt::Display for ControlAuthorityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAllowed { target, owner } => {
                write!(f, "control authority transition for {target} refused")?;
                if let Some(owner) = owner {
                    write!(f, "; current owner is {owner}")?;
                }
                Ok(())
            }
        }
    }
}

/// Claim one stable endpoint for `session` and release the session's previous
/// endpoint. A takeover also releases every claim belonging to the previous
/// owner, preserving the one-active-control relationship represented by the
/// existing [`SessionRegistry`].
pub fn claim_control(
    registry: &mut SessionRegistry,
    rbac: &SessionRbac,
    session: SessionId,
    target: u64,
) -> Result<ControlAuthorityChanged, ControlAuthorityError> {
    if !crate::session::may_control(registry, rbac, session, target) {
        return Err(ControlAuthorityError::NotAllowed {
            target,
            owner: registry.owner_of(target),
        });
    }

    let mut released = registry
        .owner_of(target)
        .filter(|&owner| owner != session)
        .map(|owner| registry.release_session(owner))
        .unwrap_or_default();
    released.extend(registry.release_session_except(session, target));
    if registry.claim(session, target).is_err() {
        return Err(ControlAuthorityError::NotAllowed {
            target,
            owner: registry.owner_of(target),
        });
    }

    Ok(ControlAuthorityChanged {
        session,
        target: Some(target),
        released,
    })
}

/// Release every endpoint currently owned by `session`.
pub fn release_control(
    registry: &mut SessionRegistry,
    session: SessionId,
) -> ControlAuthorityChanged {
    ControlAuthorityChanged {
        session,
        target: None,
        released: registry.release_session(session),
    }
}

/// Release one endpoint when it is currently owned by `session`.
pub fn release_control_target(
    registry: &mut SessionRegistry,
    session: SessionId,
    target: u64,
) -> Result<ControlAuthorityChanged, ControlAuthorityError> {
    if registry.owner_of(target) != Some(session) {
        return Err(ControlAuthorityError::NotAllowed {
            target,
            owner: registry.owner_of(target),
        });
    }
    registry.clear_gid(target);
    Ok(ControlAuthorityChanged {
        session,
        target: None,
        released: vec![target],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SessionId = SessionId(1);
    const B: SessionId = SessionId(2);
    const R1: u64 = 0xA1;
    const R2: u64 = 0xB2;

    #[test]
    fn claim_transition_keeps_one_target_and_reports_released_ids() {
        let mut registry = SessionRegistry::default();
        let rbac = SessionRbac::default();

        let first = claim_control(&mut registry, &rbac, A, R1).unwrap();
        assert_eq!(first.target, Some(R1));
        assert!(first.released.is_empty());

        let second = claim_control(&mut registry, &rbac, A, R2).unwrap();
        assert_eq!(second.released, vec![R1]);
        assert_eq!(registry.owner_of(R1), None);
        assert_eq!(registry.owner_of(R2), Some(A));
    }

    #[test]
    fn exclusive_claim_refuses_another_session() {
        let mut registry = SessionRegistry::default();
        let rbac = SessionRbac::default();
        claim_control(&mut registry, &rbac, A, R1).unwrap();

        assert_eq!(
            claim_control(&mut registry, &rbac, B, R1),
            Err(ControlAuthorityError::NotAllowed {
                target: R1,
                owner: Some(A),
            })
        );
    }
}
