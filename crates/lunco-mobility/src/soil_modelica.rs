//! Prepared, state-free Modelica evaluation for a single wheel contact law.
//! The implicit axle solver may query this repeatedly without advancing a clock.
use rumoca_ir_solve::ScalarSlot;
use rumoca_solver::SolveRuntime;
use std::cell::{OnceCell, RefCell};

const INPUTS: [&str; 15] = [
    "normalLoad",
    "patchLoad",
    "radius",
    "width",
    "longitudinalVelocity",
    "lateralVelocity",
    "treadVelocity",
    "pressureGradient",
    "cohesion",
    "frictionAngle",
    "unitWeight",
    "shearLength",
    "treadFriction",
    "sideForceRatio",
    "measuredSpeed",
];
const OUTPUTS: [&str; 7] = [
    "longitudinalForce",
    "lateralForce",
    "sinkage",
    "lateralDissipation",
    "rollingMoment",
    "rollingMomentLimit",
    "treadGainBound",
];

/// One prepared algebraic kernel per simulation app, initialized only for soil.
/// State is entirely supplied by the native contact owner.
#[derive(Default)]
pub(crate) struct SoilModelicaCache(OnceCell<Result<SoilModelica, String>>);

impl SoilModelicaCache {
    pub(crate) fn get(&self) -> Option<&SoilModelica> {
        self.0
            .get_or_init(|| {
                let result = SoilModelica::compile();
                if let Err(error) = &result {
                    bevy::log::error!("Cannot prepare Modelica soil contact: {error}");
                }
                result
            })
            .as_ref()
            .ok()
    }
}

pub(crate) struct SoilModelica {
    runtime: SolveRuntime,
    inputs: [usize; INPUTS.len()],
    outputs: [usize; OUTPUTS.len()],
    scratch: RefCell<(Vec<f64>, Vec<f64>)>,
}

impl SoilModelica {
    pub(crate) fn compile() -> Result<Self, String> {
        let mut compiler = rumoca_compile::Session::new(Default::default());
        for (name, source) in [
            (
                "LunCo/package.mo",
                include_str!("../../../assets/models/LunCo/package.mo"),
            ),
            (
                "LunCo/Mobility/package.mo",
                include_str!("../../../assets/models/LunCo/Mobility/package.mo"),
            ),
            (
                "LunCo/Mobility/RegolithWheelContact.mo",
                include_str!("../../../assets/models/LunCo/Mobility/RegolithWheelContact.mo"),
            ),
        ] {
            compiler
                .add_document(name, source)
                .map_err(|e| e.to_string())?;
        }
        let compiled = compiler
            .compile_model("LunCo.Mobility.RegolithWheelContact")
            .map_err(|e| e.to_string())?;
        let model =
            rumoca_sim::lower_dae_for_native_preparation(&compiled.dae, &Default::default())
                .map_err(|e| e.to_string())?;
        let mut inputs = [0; INPUTS.len()];
        for (index, name) in INPUTS.iter().enumerate() {
            match model.problem.layout.binding(name) {
                Some(ScalarSlot::P { index: slot, .. }) => inputs[index] = slot,
                _ => {
                    return Err(format!(
                        "Modelica contact input {name} is not a parameter slot"
                    ));
                }
            }
        }
        let mut outputs = [0; OUTPUTS.len()];
        for (index, name) in OUTPUTS.iter().enumerate() {
            outputs[index] = model
                .problem
                .solver_index(name)
                .ok_or_else(|| format!("Modelica contact output {name} is not a solver slot"))?;
        }
        let params = model.parameters.to_vec();
        let runtime = SolveRuntime::new(&model).map_err(|e| e.to_string())?;
        Ok(Self {
            runtime,
            inputs,
            outputs,
            scratch: RefCell::new((params, Vec::new())),
        })
    }

    pub(crate) fn evaluate(
        &self,
        values: [f64; INPUTS.len()],
    ) -> Result<[f64; OUTPUTS.len()], String> {
        let mut scratch = self.scratch.borrow_mut();
        let (params, solver_y) = &mut *scratch;
        for (&slot, value) in self.inputs.iter().zip(values) {
            params[slot] = value;
        }
        // This constitutive component has no integrated states. Re-seed the
        // algebraic result at every trial, so an earlier bisection evaluation
        // cannot affect the accepted result.
        self.runtime
            .full_solver_y_into(0.0, &[], params, 1e-10, 32, solver_y)
            .map_err(|e| e.to_string())?;
        let result = self.outputs.map(|slot| solver_y[slot]);
        if !result.iter().all(|x| x.is_finite()) {
            return Err("nonfinite Modelica wheel reaction".into());
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prescribed_modelica_contact_is_pure_and_resists_slip() {
        let evaluator = SoilModelica::compile().unwrap();
        let mut values = [
            186.,
            186.,
            0.465,
            0.28,
            0.116,
            1.325,
            0.10,
            980000.,
            760.,
            38.5_f64.to_radians(),
            2468.88,
            0.025,
            0.60,
            1.266,
            1.07,
        ];
        let loaded = evaluator.evaluate(values).unwrap();
        assert!(
            loaded[0] < 0. && loaded[1] < -200. && loaded[2] > 0. && loaded[3] > 0.,
            "loaded = {loaded:?}"
        );
        values[5] = -values[5];
        let opposite = evaluator.evaluate(values).unwrap();
        assert_eq!(loaded[0], opposite[0]);
        assert_eq!(loaded[1], -opposite[1]);
        assert!(loaded[4] > 0. && loaded[4] <= loaded[5] && loaded[6] > 0.);
        values[5] = -values[5];
        assert_eq!(loaded, evaluator.evaluate(values).unwrap());
        values[1] = 0.;
        let unloaded = evaluator.evaluate(values).unwrap();
        assert_eq!((unloaded[0], unloaded[1], unloaded[3]), (0., 0., 0.));
    }
}
