//! The rumoca backends, registered into [`lunco_experiments::solver`].
//!
//! This crate owns rumoca, so this is where rumoca's two-axis selection
//! (`SimSolverMode` family × `DiffsolMethod` tableau) is expressed — ONCE, in
//! [`rumoca_options`]. Above this line nobody knows those types exist: callers
//! state what the model needs and take back a [`SolverSpec`].
//!
//! Adding a backend is a registration, not an edit to an enum, a parser and a UI
//! list. That closed-enum shape is what let the live stepping path opt out of
//! selection entirely and hardcode the explicit family for every model — see
//! `lunco_experiments::solver` for the failure that produced.

use lunco_experiments::solver::{
    self, ModelCaps, SolverCaps, SolverError, SolverId, SolverParams, SolverSpec,
};

/// What the MODEL requires, read off the compiled DAE.
///
/// DERIVED, never authored and never guessed. A model carrying algebraic
/// unknowns needs a solver that can refresh them; handed an explicit stepper it
/// dies at step time with `algebraic refresh row N cannot be solved for …`,
/// which is precisely how every solar rover in the repo came to publish nothing.
pub fn model_caps(comp_res: &rumoca_compile::compile::DaeCompilationResult) -> ModelCaps {
    ModelCaps {
        implicit: !comp_res.dae.variables.algebraics.is_empty(),
    }
}

/// Register the built-in backends. Idempotent, and called from both option
/// builders so neither the ECS app, the worker thread, the wasm worker nor a CLI
/// binary can resolve against an empty registry because of plugin ordering.
pub fn ensure_builtin_solvers() {
    use std::sync::OnceLock;
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        // The three implicit tableaus. All are adaptive: they own their internal
        // step sequence, which is exactly right for a replicated model (the
        // server computes, clients receive) and exactly wrong inside a
        // client-prediction loop.
        solver::register(SolverSpec {
            id: SolverId::from("bdf"),
            caps: SolverCaps {
                implicit: true,
                fixed_step: false,
                deterministic: false,
                realtime_tolerated: false,
            },
            // Highest-ranked implicit backend: rumoca's own default, and the
            // most robust general-purpose stiff solver of the three.
            rank: 30,
            label: "BDF (stiff, variable order)".into(),
        });
        solver::register(SolverSpec {
            id: SolverId::from("esdirk34"),
            caps: SolverCaps {
                implicit: true,
                fixed_step: false,
                deterministic: false,
                realtime_tolerated: false,
            },
            rank: 20,
            label: "ESDIRK34 (stiff, sharp transitions)".into(),
        });
        solver::register(SolverSpec {
            id: SolverId::from("trbdf2"),
            caps: SolverCaps {
                implicit: true,
                fixed_step: false,
                deterministic: false,
                realtime_tolerated: false,
            },
            rank: 25,
            label: "TR-BDF2 (stiff + events)".into(),
        });

        // The explicit family. `realtime_tolerated` is the A4 concession, stated
        // as data: rumoca's `RkLike` is an EMBEDDED RK45 whose substep is still
        // error-adapted (`adapt_step(h, error_norm)`), so it is neither
        // fixed-step nor peer-deterministic, and it is the only thing available
        // for a client-predicted model today. Declaring that here — rather than
        // asserting it inside a stepper function — is what makes the gap
        // greppable and lets a real fixed-step tableau outrank it the day one is
        // registered, with no call-site edit. See TODO(A4) in
        // `docs/architecture/28-modelica-realtime-physics.md`.
        solver::register(SolverSpec {
            id: SolverId::from("rk45"),
            caps: SolverCaps {
                implicit: false,
                fixed_step: false,
                deterministic: false,
                realtime_tolerated: true,
            },
            rank: 10,
            label: "RK45 / Tsit45 (non-stiff, realtime-tolerated)".into(),
        });
    });
}

/// The ONE place a resolved [`SolverSpec`] becomes rumoca's `SimOptions`.
///
/// An id with no rumoca mapping is an ERROR, never a silent degradation to BDF —
/// the previous string parser's `unwrap_or` is how a typo became a different
/// solver with no diagnostic. A backend registered by another crate that this
/// mapping does not know about lands here loudly.
pub fn rumoca_options(
    spec: &SolverSpec,
    params: &SolverParams,
) -> Result<rumoca_sim::SimOptions, SolverError> {
    use rumoca_sim::{DiffsolMethod as D, SimSolverMode as M};

    let (mode, method) = match spec.id.as_str() {
        "bdf" => (M::Bdf, D::Bdf),
        "esdirk34" => (M::Bdf, D::Esdirk34),
        "trbdf2" => (M::Bdf, D::TrBdf2),
        "rk45" => (M::RkLike, D::Bdf),
        _ => {
            return Err(SolverError::Incapable {
                asked: spec.id.clone(),
                why: "registered, but no rumoca backend implements it — a solver \
                      outside the rumoca family needs its own options builder"
                    .into(),
            })
        }
    };

    let mut opts = rumoca_sim::SimOptions {
        solver_mode: mode,
        ..Default::default()
    };
    opts.diffsol_method = method;
    opts.atol = params.atol;
    opts.rtol = params.rtol;
    opts.dt = params.h0;
    opts.t_start = params.t_start;
    opts.t_end = params.t_end;
    Ok(opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_experiments::solver::{RuntimeProfile, SolverRequest};

    /// Every built-in registers, and each one maps to a rumoca backend. A spec
    /// that resolves but cannot be built is a runtime error nobody can act on.
    #[test]
    fn every_registered_builtin_maps_to_a_rumoca_backend() {
        ensure_builtin_solvers();
        let params = SolverParams {
            atol: 1e-6,
            rtol: 1e-6,
            h0: None,
            t_start: 0.0,
            t_end: 1.0,
        };
        for spec in solver::registered() {
            rumoca_options(&spec, &params)
                .unwrap_or_else(|e| panic!("`{}` is registered but unbuildable: {e}", spec.id));
        }
    }

    /// THE regression, end to end through the real backend table: a co-simulated
    /// electrical island (implicit, not predicted) must resolve to an implicit
    /// backend. With the explicit family it fails at step time with `algebraic
    /// refresh row 2 cannot be solved for …Battery.p.i` and silently publishes
    /// no ports — the bug that killed every solar rover in the repo.
    #[test]
    fn an_implicit_cosim_model_resolves_to_an_implicit_backend() {
        ensure_builtin_solvers();
        let spec = solver::resolve(&SolverRequest {
            needs: ModelCaps { implicit: true },
            profile: RuntimeProfile { predicted: false },
            authored: None,
        })
        .expect("the built-in implicit backends are registered");

        assert!(
            spec.caps.implicit,
            "resolved `{}` for an implicit model",
            spec.id
        );
    }

    /// A predicted model still gets the explicit stepper — the realtime class's
    /// behaviour is unchanged by this refactor, which is what keeps the drive
    /// laws and motor thermal models on exactly the stepper they run on today.
    #[test]
    fn a_predicted_model_still_gets_the_realtime_tolerated_backend() {
        ensure_builtin_solvers();
        let spec = solver::resolve(&SolverRequest {
            needs: ModelCaps { implicit: false },
            profile: RuntimeProfile { predicted: true },
            authored: None,
        })
        .expect("the realtime-tolerated backend is registered");

        assert_eq!(spec.id, SolverId::from("rk45"), "realtime class changed");
    }
}
