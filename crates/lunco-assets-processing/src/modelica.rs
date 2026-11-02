//! Modelica artifact stages reuse the admitted compiler and shared batch runner.

use crate::process::ProcessControl;
use lunco_assets_datasets::ProcessConfig;
use lunco_experiments::{ParamPath, ParamValue, RunBounds, RunResult, RunResultLimits, RunUpdate};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn invalid(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into())
}

pub(crate) fn process(
    input: &Path,
    output: &Path,
    config: &ProcessConfig,
    values: &BTreeMap<String, PathBuf>,
    control: &ProcessControl,
) -> std::io::Result<()> {
    let class = config
        .parameters
        .get("class")
        .and_then(toml::Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("modelica step requires class"))?;
    let bounds: RunBounds = config
        .parameters
        .get("bounds")
        .ok_or_else(|| invalid("modelica step requires bounds"))?
        .clone()
        .try_into()
        .map_err(|e| invalid(format!("Modelica run bounds: {e}")))?;
    let timeout = config
        .parameters
        .get("timeout_s")
        .and_then(toml::Value::as_integer)
        .filter(|s| *s > 0)
        .ok_or_else(|| invalid("modelica step requires a positive timeout_s"))?
        as u64;
    let source = std::fs::read_to_string(input)?;
    let mut extras = Vec::new();
    for (id, path) in values {
        if path != input && path.extension().and_then(|s| s.to_str()) == Some("mo") {
            extras.push((format!("{id}.mo"), std::fs::read_to_string(path)?));
        }
    }
    control.check()?;
    let mut compiler = lunco_modelica_compiler::ModelicaCompiler::new();
    let compiled = compiler
        .compile_str_multi_after_source_root_admission(class, &source, "pipeline.mo", &extras, &[])
        .map_err(invalid)?;
    let limits: RunResultLimits = match config.parameters.get("limits") {
        Some(value) => value
            .clone()
            .try_into()
            .map_err(|e| invalid(format!("Modelica result limits: {e}")))?,
        None => RunResultLimits::default(),
    };
    let inputs = run_values(config, "inputs")?;
    let overrides = run_values(config, "overrides")?;
    let mut sink = Sink {
        control,
        started: Instant::now(),
        timeout: Duration::from_secs(timeout),
        result: None,
        error: None,
    };
    lunco_modelica_runner::drive_run(
        &compiled.dae,
        &bounds,
        &inputs,
        &overrides,
        limits,
        web_time::Instant::now(),
        &mut sink,
    );
    if let Some(error) = sink.error {
        return Err(invalid(error));
    }
    let result = sink
        .result
        .ok_or_else(|| invalid("Modelica run ended without an artifact result"))?;
    result
        .validate_complete(limits)
        .map_err(|e| invalid(e.to_string()))?;
    let text = toml::to_string(&result).map_err(|e| invalid(e.to_string()))?;
    if text.len() > limits.max_artifact_bytes {
        return Err(invalid(
            "Modelica artifact exceeds the configured byte budget",
        ));
    }
    control.check()?;
    std::fs::write(output, text)
}

fn run_values(
    config: &ProcessConfig,
    field: &str,
) -> std::io::Result<BTreeMap<ParamPath, ParamValue>> {
    let Some(value) = config.parameters.get(field) else {
        return Ok(BTreeMap::new());
    };
    let values = value
        .as_table()
        .ok_or_else(|| invalid(format!("Modelica {field} must be a table")))?;
    values
        .iter()
        .map(|(key, value)| {
            let value = value
                .as_float()
                .or_else(|| value.as_integer().map(|v| v as f64))
                .filter(|v| v.is_finite())
                .ok_or_else(|| {
                    invalid(format!(
                        "Modelica {field}.{key} requires a finite Real value"
                    ))
                })?;
            Ok((ParamPath(key.clone()), ParamValue::Real(value)))
        })
        .collect()
}

struct Sink<'a> {
    control: &'a ProcessControl,
    started: Instant,
    timeout: Duration,
    result: Option<RunResult>,
    error: Option<String>,
}
impl lunco_modelica_runner::RunSink for Sink<'_> {
    fn is_cancelled(&mut self) -> bool {
        self.control.check().is_err() || self.started.elapsed() > self.timeout
    }
    fn emit(&mut self, update: RunUpdate) {
        match update {
            RunUpdate::Completed(result) => self.result = Some(result),
            RunUpdate::Failed { error } => self.error = Some(error),
            RunUpdate::Cancelled => {
                self.error = Some("Modelica processing cancelled or timed out".into())
            }
            RunUpdate::Progress { .. } => {}
        }
    }
}
