//! Solver selection — capabilities, registry, and the ONE resolution rule.
//!
//! ## Why this is a registry and not an enum
//!
//! A closed enum makes adding or choosing a solver mean editing the enum, its
//! mapping function, the parser and the UI list — so a caller with a special case
//! writes a constant instead of participating, and the live and batch paths drift
//! apart without either one being wrong on its face. Here a solver is registered
//! data and **no caller names one**: callers state what the model needs and what
//! the runtime demands, and [`resolve`] answers.
//!
//! An explicit ODE stepper handed an implicit system (a rover's battery + solar
//! panel island is one) fails per-step with `rk45 backend only supports a narrow
//! explicit ODE subset: algebraic refresh row 2 cannot be solved`, publishes no
//! ports, and surfaces nothing a driver would see. Capability is checked here so
//! that mismatch is a refusal at selection time instead.
//!
//! ## The two facts selection is made of
//!
//! * [`ModelCaps`] — what the MODEL requires. Derived from the compiled DAE
//!   (`algebraics` non-empty ⇒ implicit), never authored, never guessed.
//! * [`RuntimeProfile`] — what the CONTEXT demands. Whether this model drives a
//!   client-predicted body, which is already declared in USD as
//!   `lunco:program:realtimeSafe` and carried as `lunco_cosim::RealtimeSafe`.
//!
//! `docs/architecture/28-modelica-realtime-physics.md` §2 draws exactly this
//! line: predicted models need a fixed-step deterministic stepper; everything
//! replicated (thermal, **power/battery**, chemistry) is the "sweet spot" where
//! an adaptive implicit solver *belongs*. The electrical island is the doc's own
//! example.
//!
//! ## The A4 concession is data here, not a hidden constant
//!
//! No registered backend is honestly fixed-step and deterministic today —
//! rumoca's `RkLike` is an embedded RK45 whose substep is error-adapted. Rather
//! than let a function body quietly assert otherwise, a backend that may be used
//! for predicted models without meeting the bar declares
//! [`SolverCaps::realtime_tolerated`]. The gap is then one greppable field with
//! an owner instead of an invisible default, and the day a real fixed-step
//! tableau is registered it outranks the concession with no call-site edit.

use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};

/// A registered solver's name. Free-form so a backend can be added without
/// touching this crate — the point of the registry.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SolverId(pub String);

impl SolverId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SolverId {
    /// Normalizes case, `-`, `_` and spaces, so `"TR-BDF2"`, `"tr_bdf2"` and
    /// `"trbdf2"` are one id and cannot register twice under three spellings.
    fn from(s: &str) -> Self {
        Self(s.trim().to_ascii_lowercase().replace(['-', '_', ' '], ""))
    }
}

impl std::fmt::Display for SolverId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a solver can do. Every field is a claim the backend makes about itself
/// and that [`resolve`] checks against a requirement — nothing here is inferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SolverCaps {
    /// Solves systems carrying algebraic unknowns (a constrained/implicit DAE),
    /// not just an explicit ODE. The capability the electrical island needs and
    /// the explicit RK family lacks.
    pub implicit: bool,
    /// Integrates on a step sequence fixed by the caller, with no internal
    /// error-adapted substepping. Required for a client-predicted model: two
    /// peers must take the SAME steps.
    pub fixed_step: bool,
    /// Produces identical results on every peer for identical inputs. Distinct
    /// from `fixed_step`: a fixed-step method can still diverge through
    /// non-reproducible arithmetic.
    pub deterministic: bool,
    /// May serve a predicted model despite failing the bar above. A DECLARED
    /// concession, not a fallback the resolver invents — see the module docs on
    /// A4. Ranked below any backend that genuinely qualifies.
    pub realtime_tolerated: bool,
}

/// A registered solver.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolverSpec {
    pub id: SolverId,
    pub caps: SolverCaps,
    /// Preference among equally-capable backends; higher wins. Only a tiebreak —
    /// it can never make an incapable solver eligible.
    pub rank: u8,
    /// One line for the UI. The registry is the combo box's source, so a newly
    /// registered backend appears without a list to update.
    pub label: String,
}

/// What the MODEL requires, derived from the compiled DAE.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ModelCaps {
    /// The model carries algebraic unknowns. Read straight off the compiled
    /// DAE's `variables.algebraics`; a caller must not guess it.
    pub implicit: bool,
}

/// What the CONTEXT demands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RuntimeProfile {
    /// This model drives a client-predicted body — i.e. it declared
    /// `lunco:program:realtimeSafe`. Co-simulated prims are stamped
    /// `NotPredictable`, so for them this is false and an adaptive implicit
    /// solver is not merely allowed but correct.
    pub predicted: bool,
}

/// A selection request. Both facts plus the optional authored override.
#[derive(Clone, Debug, Default)]
pub struct SolverRequest {
    pub needs: ModelCaps,
    pub profile: RuntimeProfile,
    /// An explicit choice, from the experiments UI via `RunBounds::solver`. The
    /// live path never sets it.
    /// Honoured when capable and REFUSED when not — never silently replaced, so
    /// a typo is an error rather than a different solver.
    pub authored: Option<SolverId>,
}

/// Why a request could not be served. Every variant is reportable to a user:
/// the failure this whole module exists to prevent was unattributed silence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SolverError {
    /// The authored id is not registered.
    Unknown { asked: SolverId, known: Vec<SolverId> },
    /// The authored id exists but cannot serve this model or context.
    Incapable { asked: SolverId, why: String },
    /// Nothing registered satisfies the requirement.
    NoCapableSolver { why: String },
}

impl std::fmt::Display for SolverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown { asked, known } => write!(
                f,
                "unknown solver `{asked}`; registered: {}",
                known
                    .iter()
                    .map(SolverId::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Incapable { asked, why } => write!(f, "solver `{asked}` cannot serve this model: {why}"),
            Self::NoCapableSolver { why } => write!(f, "no registered solver can serve this model: {why}"),
        }
    }
}

impl std::error::Error for SolverError {}

/// Whether `spec` can serve `req`, and why not when it cannot.
///
/// The two rules, in full:
/// 1. An implicit model needs an implicit-capable solver.
/// 2. A predicted model needs a solver that is fixed-step AND deterministic, or
///    one that explicitly declares the A4 concession.
fn satisfies(spec: &SolverSpec, req: &SolverRequest) -> Result<(), String> {
    if req.needs.implicit && !spec.caps.implicit {
        return Err(format!(
            "the model has algebraic unknowns and `{}` solves explicit ODEs only",
            spec.id
        ));
    }
    if req.profile.predicted
        && !(spec.caps.fixed_step && spec.caps.deterministic)
        && !spec.caps.realtime_tolerated
    {
        return Err(format!(
            "the model drives a client-predicted body and `{}` is neither fixed-step \
             deterministic nor declared realtime-tolerated",
            spec.id
        ));
    }
    Ok(())
}

/// Preference score among capable candidates. A genuinely realtime-qualified
/// backend must beat a merely tolerated one for a predicted model, so that the
/// concession retires by itself the day a real fixed-step tableau registers.
fn score(spec: &SolverSpec, req: &SolverRequest) -> u16 {
    let qualified = req.profile.predicted && spec.caps.fixed_step && spec.caps.deterministic;
    u16::from(qualified) * 256 + u16::from(spec.rank)
}

/// The registry. Global rather than a Bevy `Resource` because both the ECS and
/// the background Modelica worker thread resolve against it, and two copies is
/// the divergence this module exists to remove.
fn registry() -> &'static RwLock<Vec<SolverSpec>> {
    static REGISTRY: OnceLock<RwLock<Vec<SolverSpec>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(Vec::new()))
}

/// Register (or replace, by id) a solver. Idempotent, so a plugin that registers
/// on every app build does not accumulate duplicates.
pub fn register(spec: SolverSpec) {
    let mut specs = registry().write().expect("solver registry poisoned");
    match specs.iter_mut().find(|s| s.id == spec.id) {
        Some(existing) => *existing = spec,
        None => specs.push(spec),
    }
}

/// Every registered solver, id order. The UI's list and the API's vocabulary
/// both read this — there is no second table to fall out of sync.
pub fn registered() -> Vec<SolverSpec> {
    let mut specs = registry().read().expect("solver registry poisoned").clone();
    specs.sort_by(|a, b| a.id.cmp(&b.id));
    specs
}

/// Look up one registered solver by id.
pub fn get(id: &SolverId) -> Option<SolverSpec> {
    registry()
        .read()
        .expect("solver registry poisoned")
        .iter()
        .find(|s| &s.id == id)
        .cloned()
}

/// THE selection rule. Live stepping and batch runs both come through here, so
/// they cannot pick different families for the same model.
pub fn resolve(req: &SolverRequest) -> Result<SolverSpec, SolverError> {
    resolve_in(&registered(), req)
}

/// [`resolve`] against an explicit candidate set — the pure rule, with no global
/// state. `resolve` is this plus the registry, and the tests use this directly so
/// they neither depend on registration order nor pollute a shared registry.
pub fn resolve_in(specs: &[SolverSpec], req: &SolverRequest) -> Result<SolverSpec, SolverError> {
    if let Some(asked) = &req.authored {
        let Some(spec) = specs.iter().find(|s| &s.id == asked) else {
            return Err(SolverError::Unknown {
                asked: asked.clone(),
                known: specs.iter().map(|s| s.id.clone()).collect(),
            });
        };
        return match satisfies(spec, req) {
            Ok(()) => Ok(spec.clone()),
            Err(why) => Err(SolverError::Incapable {
                asked: asked.clone(),
                why,
            }),
        };
    }

    let mut best: Option<&SolverSpec> = None;
    let mut rejections = Vec::new();
    for spec in specs {
        match satisfies(spec, req) {
            Err(why) => rejections.push(why),
            Ok(()) => {
                if best.is_none_or(|b| score(spec, req) > score(b, req)) {
                    best = Some(spec);
                }
            }
        }
    }
    best.cloned().ok_or_else(|| SolverError::NoCapableSolver {
        why: if rejections.is_empty() {
            "no solver is registered at all".to_string()
        } else {
            rejections.join("; ")
        },
    })
}

/// Numerical settings handed to whichever backend was resolved. Backend-neutral
/// on purpose: this crate must not depend on rumoca, so the *translation* to a
/// concrete backend's options lives with the backend that owns it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolverParams {
    pub atol: f64,
    pub rtol: f64,
    /// Initial/maximum internal step (h0). On the live path this is the fixed
    /// micro-step, which is what makes the macro stop-times identical everywhere.
    pub h0: Option<f64>,
    pub t_start: f64,
    pub t_end: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, caps: SolverCaps, rank: u8) -> SolverSpec {
        SolverSpec {
            id: SolverId::from(id),
            caps,
            rank,
            label: id.to_string(),
        }
    }

    const IMPLICIT: SolverCaps = SolverCaps {
        implicit: true,
        fixed_step: false,
        deterministic: false,
        realtime_tolerated: false,
    };
    const EXPLICIT_TOLERATED: SolverCaps = SolverCaps {
        implicit: false,
        fixed_step: false,
        deterministic: false,
        realtime_tolerated: true,
    };

    /// The two-backend world every test below reasons about: one implicit
    /// solver, and one explicit solver ranked far higher so a resolver that
    /// ignored capability would pick it.
    fn candidates() -> Vec<SolverSpec> {
        vec![
            spec("testbdf", IMPLICIT, 10),
            spec("testrk", EXPLICIT_TOLERATED, 200),
        ]
    }

    /// An implicit model must never be handed the explicit family.
    #[test]
    fn an_implicit_model_never_resolves_to_an_explicit_solver() {
        let chosen = resolve_in(
            &candidates(),
            &SolverRequest {
                needs: ModelCaps { implicit: true },
                profile: RuntimeProfile { predicted: false },
                ..Default::default()
            },
        )
        .expect("an implicit-capable solver is registered");

        assert!(
            chosen.caps.implicit,
            "resolved `{}`, which cannot solve algebraic unknowns — the rk45 \
             `algebraic refresh row 2` failure, reintroduced",
            chosen.id
        );
    }

    /// An authored choice that cannot serve the model is REFUSED, not quietly
    /// swapped for one that can — silent substitution hides an authoring error.
    #[test]
    fn an_incapable_authored_solver_is_refused_not_substituted() {
        let err = resolve_in(
            &candidates(),
            &SolverRequest {
                needs: ModelCaps { implicit: true },
                profile: RuntimeProfile::default(),
                authored: Some(SolverId::from("testrk")),
            },
        )
        .expect_err("an explicit solver cannot serve an implicit model");

        assert!(matches!(err, SolverError::Incapable { .. }), "got {err:?}");
    }

    #[test]
    fn an_unknown_authored_solver_names_what_is_registered() {
        let err = resolve_in(
            &candidates(),
            &SolverRequest {
                authored: Some(SolverId::from("nope")),
                ..Default::default()
            },
        )
        .expect_err("unknown ids must not fall back");
        assert!(matches!(err, SolverError::Unknown { .. }), "got {err:?}");
    }

    /// An implicit model with nothing implicit registered must ERROR at
    /// selection, not take the explicit solver and fail per-step in a log line.
    #[test]
    fn an_implicit_model_with_no_capable_backend_errors() {
        let err = resolve_in(
            &[spec("testrk", EXPLICIT_TOLERATED, 200)],
            &SolverRequest {
                needs: ModelCaps { implicit: true },
                ..Default::default()
            },
        )
        .expect_err("nothing registered can solve algebraic unknowns");
        assert!(
            matches!(err, SolverError::NoCapableSolver { .. }),
            "got {err:?}"
        );
    }

    /// A genuinely fixed-step deterministic backend must outrank the A4
    /// concession for a predicted model, however high the concession's rank —
    /// otherwise registering the real thing upstream would change nothing.
    #[test]
    fn a_qualified_realtime_solver_outranks_the_tolerated_concession() {
        let specs = vec![
            spec("testrk", EXPLICIT_TOLERATED, 200),
            spec(
                "testfixed",
                SolverCaps {
                    implicit: false,
                    fixed_step: true,
                    deterministic: true,
                    realtime_tolerated: false,
                },
                1,
            ),
        ];

        let chosen = resolve_in(
            &specs,
            &SolverRequest {
                needs: ModelCaps::default(),
                profile: RuntimeProfile { predicted: true },
                ..Default::default()
            },
        )
        .expect("both candidates serve a non-implicit predicted model");

        assert_eq!(
            chosen.id,
            SolverId::from("testfixed"),
            "the tolerated concession won over a qualified backend"
        );
    }

    #[test]
    fn ids_normalize_so_one_solver_cannot_register_under_three_spellings() {
        assert_eq!(SolverId::from("TR-BDF2"), SolverId::from("tr_bdf2"));
        assert_eq!(SolverId::from("tr bdf2"), SolverId::from("trbdf2"));
    }
}
