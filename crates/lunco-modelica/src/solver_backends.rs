//! The rumoca backends, registered into [`lunco_experiments::solver`].
//!
//! This crate owns rumoca, so this is where rumoca's two-axis selection
//! (`SimSolverMode` family × `DiffsolMethod` tableau) is expressed — ONCE, in
//! [`rumoca_options`]. Above this line nobody knows those types exist: callers
//! state where the model runs and take back a [`SolverSpec`].
//!
//! Adding a backend is a registration, not an edit to an enum, a parser and a UI
//! list — see `lunco_experiments::solver` for why selection is data.

use lunco_experiments::solver::{
    self, SolverCaps, SolverError, SolverId, SolverParams, SolverSpec,
};

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
                // Adaptive: owns its own step sequence and its per-step cost is
                // unbounded, so it must not be driven by the frame loop.
                usable_live: false,
                fixed_step: false,
                deterministic: false,
            },
            // Highest-ranked implicit backend: rumoca's own default, and the
            // most robust general-purpose stiff solver of the three.
            rank: 30,
            label: "BDF (stiff, variable order)".into(),
        });
        solver::register(SolverSpec {
            id: SolverId::from("esdirk34"),
            caps: SolverCaps {
                // Adaptive: owns its own step sequence and its per-step cost is
                // unbounded, so it must not be driven by the frame loop.
                usable_live: false,
                fixed_step: false,
                deterministic: false,
            },
            rank: 20,
            label: "ESDIRK34 (stiff, sharp transitions)".into(),
        });
        solver::register(SolverSpec {
            id: SolverId::from("trbdf2"),
            caps: SolverCaps {
                // Adaptive: owns its own step sequence and its per-step cost is
                // unbounded, so it must not be driven by the frame loop.
                usable_live: false,
                fixed_step: false,
                deterministic: false,
            },
            rank: 25,
            label: "TR-BDF2 (stiff + events)".into(),
        });

        // The explicit family is frame-loop usable for authoritative live
        // co-simulation, but rumoca's `RkLike` is an embedded RK45 whose
        // internal step remains error-adapted. It therefore cannot serve a
        // client-predicted body. The qualified fixed-rk4 backend below owns that
        // prediction contract; this adaptive family remains authoritative-only.
        solver::register(SolverSpec {
            id: SolverId::from("rk45"),
            caps: SolverCaps {
                usable_live: true,
                fixed_step: false,
                deterministic: false,
            },
            rank: 10,
            label: "RK45 / Tsit45 (non-stiff, authoritative live)".into(),
        });

        // This is not Rumoca's adaptive RkLike session. The live construction
        // boundary dispatches this capability to lunco-modelica's explicit RK4
        // loop over Rumoca's lowered derivative runtime. Its qualified model
        // contract is continuous, event-free, and external-table-free.
        solver::register(SolverSpec {
            id: SolverId::from("fixed-rk4"),
            caps: SolverCaps {
                usable_live: true,
                fixed_step: true,
                deterministic: true,
            },
            // Ordinary authoritative live models continue to use RK45. This
            // backend is selected when prediction requires the stronger caps.
            rank: 5,
            label: "Fixed RK4 (prediction-safe continuous models)".into(),
        });
    });
}

/// The ONE place a resolved [`SolverSpec`] becomes rumoca's `SimOptions`.
///
/// An id with no rumoca mapping is an ERROR, never a silent degradation to BDF:
/// degrading turns a typo into a different solver with no diagnostic. A backend
/// registered by another crate that this mapping does not know about lands here
/// loudly.
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
        // The fixed-rk4 session owns the integration loop. These options only
        // carry the solver-neutral Rumoca lowering mode and tolerances into the
        // common DAE-to-SolveModel path.
        "fixedrk4" => (M::RkLike, D::Bdf),
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

    /// A LIVE model resolves to a frame-loop-safe backend. Handing the frame loop
    /// an adaptive implicit backend was MEASURED to stall the worker: only one of
    /// three islands finished compiling in 30 s of sim time, with no error.
    #[test]
    fn a_live_model_resolves_to_a_frame_loop_safe_backend() {
        ensure_builtin_solvers();
        let spec = solver::resolve(&SolverRequest {
            profile: RuntimeProfile {
                live: true,
                predicted: false,
            },
            authored: None,
        })
        .expect("a frame-loop-safe backend is registered");

        assert!(
            spec.caps.usable_live,
            "resolved `{}`, which is not usable inside the frame loop",
            spec.id
        );
    }

    /// A BATCH run gets the adaptive family — it owns its own time loop, so a
    /// backend that is not frame-loop safe is not merely allowed but wanted, and
    /// the highest-ranked of those wins.
    #[test]
    fn a_batch_model_resolves_to_the_adaptive_family() {
        ensure_builtin_solvers();
        let spec = solver::resolve(&SolverRequest {
            profile: RuntimeProfile {
                live: false,
                predicted: false,
            },
            authored: None,
        })
        .expect("the adaptive backends are registered");

        assert_eq!(
            spec.id,
            SolverId::from("bdf"),
            "batch resolved `{}` instead of the highest-ranked adaptive backend",
            spec.id
        );
    }

    /// Prediction selects the explicitly fixed-step backend. Adaptive RK45 is
    /// not admitted merely because it accepts a nominal output interval.
    #[test]
    fn a_predicted_model_selects_the_qualified_backend() {
        ensure_builtin_solvers();
        let spec = solver::resolve(&SolverRequest {
            profile: RuntimeProfile {
                live: true,
                predicted: true,
            },
            authored: None,
        })
        .expect("the fixed-step backend is valid for prediction");

        assert_eq!(spec.id, SolverId::from("fixed-rk4"));
        assert!(spec.caps.fixed_step);
        assert!(spec.caps.deterministic);
    }
}
