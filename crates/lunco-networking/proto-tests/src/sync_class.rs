//! Mechanism selection (M1–M7) from the four classifying axes. Pure logic.
//!
//! Mirrors `SYNC_ARCHITECTURE.md` §1–§3 and the selection procedure in
//! `MECHANISM_SELECTION.md`. The tests assert that the case matrix routes exactly
//! as documented and that axis contradictions are rejected.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mechanism {
    M1Content,
    M2State,
    M3Command,
    M4Input,
    M5Crdt,
    M6Clock, // substrate — not selected per-datum; here for completeness
    M7Local,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Temporal {
    Static,
    Continuous,
    Discrete,
    ConcurrentText,
    HighRateInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Authority {
    Server,
    ClientOwned,
    Shared,
    LocalOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Computability {
    Reconstructible,
    Predictable,
    Opaque,
}

/// M2 receiver role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Predicted,
    Interpolated,
    NotApplicable,
}

/// The declared/derived properties of a piece of state.
#[derive(Clone, Copy, Debug)]
pub struct Classification {
    pub temporal: Temporal,
    pub authority: Authority,
    pub computability: Computability,
    /// Provenance is Content or Derived (loaded identically on each peer).
    pub from_content: bool,
    /// Provenance is Local (per-peer, never networked).
    pub local_only: bool,
    /// Step 0.5: is this a pure function of already-synced state? → recompute (M7).
    pub pure_function_of_synced: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncError {
    /// Axes disagree (e.g. Local provenance but non-local authority).
    Contradiction(&'static str),
}

/// Route a classification to exactly one mechanism (+ role for M2).
pub fn classify(c: &Classification) -> Result<(Mechanism, Role), SyncError> {
    // --- contradiction checks (debug-panic in the real system) ---
    if c.local_only && !matches!(c.authority, Authority::LocalOnly) {
        return Err(SyncError::Contradiction(
            "Local provenance must have LocalOnly authority",
        ));
    }
    if matches!(c.authority, Authority::LocalOnly) && !c.local_only && !c.pure_function_of_synced {
        return Err(SyncError::Contradiction(
            "LocalOnly authority requires Local provenance or a pure-derived value",
        ));
    }

    // --- Step 0 / 0.5: anything local or recomputable stays off the wire ---
    if c.local_only || c.pure_function_of_synced {
        return Ok((Mechanism::M7Local, Role::NotApplicable));
    }

    // --- route by temporal character ---
    match c.temporal {
        Temporal::ConcurrentText => Ok((Mechanism::M5Crdt, Role::NotApplicable)),
        Temporal::HighRateInput => Ok((Mechanism::M4Input, Role::NotApplicable)),
        Temporal::Static => {
            if c.from_content {
                Ok((Mechanism::M1Content, Role::NotApplicable))
            } else {
                // runtime-born but static → one reliable command
                Ok((Mechanism::M3Command, Role::NotApplicable))
            }
        }
        Temporal::Discrete => Ok((Mechanism::M3Command, Role::NotApplicable)),
        Temporal::Continuous => {
            let role = match c.computability {
                Computability::Predictable => Role::Predicted,
                Computability::Opaque | Computability::Reconstructible => Role::Interpolated,
            };
            Ok((Mechanism::M2State, role))
        }
    }
}

/// Guard: you may only request prediction for something the receiver can compute.
/// Mirrors the "ownership ≠ predictability" rule (gap C).
pub fn validate_prediction(
    computability: Computability,
    requested: Role,
) -> Result<(), SyncError> {
    if requested == Role::Predicted && !matches!(computability, Computability::Predictable) {
        return Err(SyncError::Contradiction(
            "cannot predict an Opaque entity (e.g. cosim-force-driven)",
        ));
    }
    Ok(())
}
