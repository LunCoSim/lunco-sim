//! Portable completed-history artifacts and bounded JSON admission.
//! Runtime owner identities and filesystem roots are deliberately not serialized.

use crate::{
    Experiment, ExperimentId, ModelRef, ParamPath, ParamValue, RunBounds, RunResult,
    RunResultLimits,
};
use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

pub const RUN_ARTIFACT_VERSION: u32 = 1;

/// Compiler-owned identity of the actual seated source contributions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceContentIdentity {
    Available { cid: lunco_hash::content::Cid },
    Unavailable { reason: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExperimentDefinition {
    pub model_ref: ModelRef,
    pub overrides: BTreeMap<ParamPath, ParamValue>,
    pub inputs: BTreeMap<ParamPath, ParamValue>,
    pub bounds: RunBounds,
}
impl From<&Experiment> for ExperimentDefinition {
    fn from(experiment: &Experiment) -> Self {
        Self {
            model_ref: experiment.model_ref.clone(),
            overrides: experiment.overrides.clone(),
            inputs: experiment.inputs.clone(),
            bounds: experiment.bounds.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunArtifact {
    pub version: u32,
    pub experiment_id: ExperimentId,
    pub definition: Arc<ExperimentDefinition>,
    pub name: String,
    pub color_hint: u8,
    pub created_at: web_time::SystemTime,
    pub result: Arc<RunResult>,
}
impl RunArtifact {
    pub fn from_experiment(experiment: &Experiment) -> Result<Self, String> {
        let result = experiment
            .result
            .clone()
            .ok_or("completed artifact has no trajectory")?;
        if !matches!(
            result.meta.source_content,
            Some(SourceContentIdentity::Available { .. })
        ) {
            return Err(match &result.meta.source_content {
                Some(SourceContentIdentity::Unavailable { reason }) => {
                    format!("completed result is not persistable: {reason}")
                }
                _ => "completed result is not persistable: compiler source identity is missing"
                    .into(),
            });
        }
        Ok(Self {
            version: RUN_ARTIFACT_VERSION,
            experiment_id: experiment.id,
            definition: Arc::new(experiment.into()),
            name: experiment.name.clone(),
            color_hint: experiment.color_hint,
            created_at: experiment.created_at,
            result,
        })
    }
    pub fn validate(&self, limits: RunResultLimits) -> Result<(), String> {
        if self.version != RUN_ARTIFACT_VERSION {
            return Err(format!(
                "unsupported experiment artifact version {}",
                self.version
            ));
        }
        if !matches!(
            self.result.meta.source_content,
            Some(SourceContentIdentity::Available { .. })
        ) {
            return Err("experiment artifact has no authoritative compiled-source identity".into());
        }
        self.result
            .validate_complete(limits)
            .map_err(|error| error.to_string())
    }
}

/// Byte-bounded serialization does not first materialize an unbounded JSON Vec.
pub fn encode_run_artifact(
    artifact: &RunArtifact,
    limits: RunResultLimits,
) -> Result<Vec<u8>, String> {
    artifact.validate(limits)?;
    struct BoundedWriter {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for BoundedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let next = self
                .bytes
                .len()
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("artifact byte count overflow"))?;
            if next > self.limit {
                return Err(std::io::Error::other(
                    "experiment artifact exceeds configured byte budget",
                ));
            }
            self.bytes
                .try_reserve(bytes.len())
                .map_err(std::io::Error::other)?;
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit: limits.max_artifact_bytes,
    };
    serde_json::to_writer(&mut writer, artifact).map_err(|error| error.to_string())?;
    Ok(writer.bytes)
}

/// Borrow the large subtrees until their allocation budgets have been admitted.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawArtifact<'a> {
    version: u32,
    experiment_id: ExperimentId,
    #[serde(borrow)]
    definition: &'a serde_json::value::RawValue,
    name: String,
    color_hint: u8,
    created_at: web_time::SystemTime,
    #[serde(borrow)]
    result: &'a serde_json::value::RawValue,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResult<'a> {
    #[serde(borrow)]
    times: &'a serde_json::value::RawValue,
    #[serde(borrow)]
    series: &'a serde_json::value::RawValue,
    meta: crate::RunMeta,
}

struct ScalarBudget {
    remaining: usize,
}
impl ScalarBudget {
    fn consume<E: Error>(&mut self) -> Result<(), E> {
        self.remaining = self.remaining.checked_sub(1).ok_or_else(|| {
            E::custom("experiment artifact exceeds configured decoded scalar budget")
        })?;
        Ok(())
    }
}
struct ValuesSeed<'a>(&'a mut ScalarBudget);
impl<'de> DeserializeSeed<'de> for ValuesSeed<'_> {
    type Value = Vec<f64>;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        struct ValuesVisitor<'a>(&'a mut ScalarBudget);
        impl<'de> Visitor<'de> for ValuesVisitor<'_> {
            type Value = Vec<f64>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a bounded numeric trajectory vector")
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<f64>()? {
                    self.0.consume::<A::Error>()?;
                    values.try_reserve(1).map_err(A::Error::custom)?;
                    values.push(value);
                }
                Ok(values)
            }
        }
        deserializer.deserialize_seq(ValuesVisitor(self.0))
    }
}
struct SeriesSeed<'a>(&'a mut ScalarBudget, usize);
impl<'de> DeserializeSeed<'de> for SeriesSeed<'_> {
    type Value = BTreeMap<String, Vec<f64>>;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        struct SeriesVisitor<'a>(&'a mut ScalarBudget, usize);
        impl<'de> Visitor<'de> for SeriesVisitor<'_> {
            type Value = BTreeMap<String, Vec<f64>>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("unique bounded trajectory columns")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut series = BTreeMap::new();
                let max_columns = self.0.remaining;
                while let Some(name) = map.next_key::<String>()? {
                    if series.contains_key(&name) {
                        return Err(A::Error::custom("duplicate experiment trajectory column"));
                    }
                    // A malformed empty column must not create an unbounded map.
                    if series.len() >= max_columns {
                        return Err(A::Error::custom(
                            "experiment artifact column count exceeds scalar budget",
                        ));
                    }
                    let values = map.next_value_seed(ValuesSeed(self.0))?;
                    if values.len() != self.1 {
                        return Err(A::Error::custom(
                            "artifact series length does not match admitted times",
                        ));
                    }
                    series.insert(name, values);
                }
                Ok(series)
            }
        }
        deserializer.deserialize_map(SeriesVisitor(self.0, self.1))
    }
}

/// Count definition numbers without constructing its parameter-array vectors.
/// Strings and maps remain bounded by the admitted input byte budget.
struct DefinitionPreflight<'a>(&'a mut ScalarBudget);
impl<'de> DeserializeSeed<'de> for DefinitionPreflight<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for DefinitionPreflight<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a byte and scalar bounded execution definition")
    }
    fn visit_bool<E: Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: Error>(self, _: i64) -> Result<(), E> {
        self.0.consume()
    }
    fn visit_u64<E: Error>(self, _: u64) -> Result<(), E> {
        self.0.consume()
    }
    fn visit_f64<E: Error>(self, _: f64) -> Result<(), E> {
        self.0.consume()
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq
            .next_element_seed(DefinitionPreflight(self.0))?
            .is_some()
        {}
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while map.next_key::<serde::de::IgnoredAny>()?.is_some() {
            map.next_value_seed(DefinitionPreflight(self.0))?;
        }
        Ok(())
    }
}

pub fn decode_run_artifact(bytes: &[u8], limits: RunResultLimits) -> Result<RunArtifact, String> {
    limits.validate().map_err(|error| error.to_string())?;
    if bytes.len() > limits.max_artifact_bytes {
        return Err("experiment artifact exceeds configured byte budget".into());
    }
    let raw: RawArtifact<'_> = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if raw.version != RUN_ARTIFACT_VERSION {
        return Err(format!(
            "unsupported experiment artifact version {}",
            raw.version
        ));
    }
    #[derive(Deserialize)]
    struct RawDefinition<'a> {
        #[serde(borrow)]
        overrides: &'a serde_json::value::RawValue,
        #[serde(borrow)]
        inputs: &'a serde_json::value::RawValue,
    }
    let parameters: RawDefinition<'_> =
        serde_json::from_str(raw.definition.get()).map_err(|error| error.to_string())?;
    let mut parameter_budget = ScalarBudget {
        remaining: limits.max_values,
    };
    for values in [parameters.overrides, parameters.inputs] {
        DefinitionPreflight(&mut parameter_budget)
            .deserialize(&mut serde_json::Deserializer::from_str(values.get()))
            .map_err(|error| error.to_string())?;
    }
    let definition =
        serde_json::from_str(raw.definition.get()).map_err(|error| error.to_string())?;
    let result: RawResult<'_> =
        serde_json::from_str(raw.result.get()).map_err(|error| error.to_string())?;
    let mut budget = ScalarBudget {
        remaining: limits.max_values,
    };
    let times = ValuesSeed(&mut budget)
        .deserialize(&mut serde_json::Deserializer::from_str(result.times.get()))
        .map_err(|error| error.to_string())?;
    if times.is_empty() {
        return Err("complete experiment artifact has no times".into());
    }
    let series = SeriesSeed(&mut budget, times.len())
        .deserialize(&mut serde_json::Deserializer::from_str(result.series.get()))
        .map_err(|error| error.to_string())?;
    let artifact = RunArtifact {
        version: raw.version,
        experiment_id: raw.experiment_id,
        definition: Arc::new(definition),
        name: raw.name,
        color_hint: raw.color_hint,
        created_at: raw.created_at,
        result: Arc::new(RunResult {
            times,
            series,
            meta: result.meta,
        }),
    };
    artifact.validate(limits)?;
    Ok(artifact)
}

/// Stamp only the actual compiled closure returned by the compiler owner.
pub fn stamp_source_identity(update: &mut crate::RunUpdate, identity: &SourceContentIdentity) {
    match update {
        crate::RunUpdate::Completed(result) => result.meta.source_content = Some(identity.clone()),
        crate::RunUpdate::Progress {
            delta: Some(result),
            ..
        }
        | crate::RunUpdate::Failed {
            partial: Some(result),
            ..
        } => result.meta.source_content = Some(identity.clone()),
        _ => {}
    }
}

/// CPU/IO operation performed off the UI thread. Exact runtime owner remains
/// in the admitting application task and never crosses this persistence wire.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactRequest {
    pub token: u64,
    pub operation: ArtifactOperation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ArtifactOperation {
    Read {
        path: std::path::PathBuf,
        limits: RunResultLimits,
    },
    Write {
        path: std::path::PathBuf,
        artifact: RunArtifact,
        limits: RunResultLimits,
    },
    List {
        directory: std::path::PathBuf,
        cap: usize,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ArtifactOutcome {
    Read(RunArtifact),
    Written,
    Listed {
        paths: Vec<std::path::PathBuf>,
        truncated: bool,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactResponse {
    pub token: u64,
    pub result: Result<ArtifactOutcome, String>,
}
#[cfg(feature = "bevy")]
#[derive(bevy::prelude::Resource)]
pub struct ArtifactWorkerTransport {
    pub dispatch: fn(
        ArtifactRequest,
        lunco_workspace::DocumentRuntimeOwner,
        crossbeam_channel::Sender<ArtifactResponse>,
    ) -> Result<(), String>,
    pub retire: fn(&lunco_workspace::DocumentRuntimeOwner),
    pub discard: fn(ExperimentId),
    pub clear: fn(),
}

/// Small typed browser codec header; trajectory vectors stay in the already
/// received transferable completion buffer and are never serialized by UI.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunArtifactHeader {
    pub experiment_id: ExperimentId,
    pub definition: Arc<ExperimentDefinition>,
    pub name: String,
    pub color_hint: u8,
    pub created_at: web_time::SystemTime,
    pub source_content: SourceContentIdentity,
}
impl RunArtifactHeader {
    pub fn from_artifact(artifact: &RunArtifact) -> Result<Self, String> {
        let source_content = artifact
            .result
            .meta
            .source_content
            .clone()
            .ok_or("artifact source identity is missing")?;
        if !matches!(source_content, SourceContentIdentity::Available { .. }) {
            return Err("artifact source identity is unavailable".into());
        }
        Ok(Self {
            experiment_id: artifact.experiment_id,
            definition: artifact.definition.clone(),
            name: artifact.name.clone(),
            color_hint: artifact.color_hint,
            created_at: artifact.created_at,
            source_content,
        })
    }
    pub fn with_result(self, result: RunResult) -> Result<RunArtifact, String> {
        if result.meta.source_content.as_ref() != Some(&self.source_content) {
            return Err("transferred completion and artifact header source CID disagree".into());
        }
        Ok(RunArtifact {
            version: RUN_ARTIFACT_VERSION,
            experiment_id: self.experiment_id,
            definition: self.definition,
            name: self.name,
            color_hint: self.color_hint,
            created_at: self.created_at,
            result: Arc::new(result),
        })
    }
}

#[cfg(feature = "bevy")]
impl Drop for ArtifactWorkerTransport {
    fn drop(&mut self) {
        (self.clear)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn artifact() -> RunArtifact {
        RunArtifact {
            version: RUN_ARTIFACT_VERSION,
            experiment_id: ExperimentId::new(),
            definition: Arc::new(ExperimentDefinition {
                model_ref: ModelRef("Probe".into()),
                overrides: BTreeMap::new(),
                inputs: BTreeMap::new(),
                bounds: RunBounds::default(),
            }),
            name: "Run 1".into(),
            color_hint: 0,
            created_at: web_time::UNIX_EPOCH,
            result: Arc::new(RunResult {
                times: vec![0.0, 1.0, 1.0],
                series: BTreeMap::from([("x".into(), vec![1.0, 2.0, 3.0])]),
                meta: crate::RunMeta {
                    sample_count: 3,
                    source_content: Some(SourceContentIdentity::Available {
                        cid: lunco_hash::content::cid(b"compiled closure"),
                    }),
                    ..Default::default()
                },
            }),
        }
    }
    #[test]
    fn artifact_codec_admits_exact_budgets_and_rejects_before_vector_growth() {
        let artifact = artifact();
        let limits = RunResultLimits {
            max_values: 6,
            max_artifact_bytes: 4096,
        };
        let bytes = encode_run_artifact(&artifact, limits).expect("valid envelope");
        let exact = RunResultLimits {
            max_artifact_bytes: bytes.len(),
            ..limits
        };
        let restored = decode_run_artifact(&bytes, exact).expect("exact budget");
        assert_eq!(restored.experiment_id, artifact.experiment_id);
        assert_eq!(restored.result.times, vec![0.0, 1.0, 1.0]);
        assert_eq!(
            restored.result.meta.source_content,
            artifact.result.meta.source_content
        );
        assert!(
            decode_run_artifact(
                &bytes,
                RunResultLimits {
                    max_values: 5,
                    ..exact
                }
            )
            .unwrap_err()
            .contains("scalar budget")
        );
        assert!(
            encode_run_artifact(
                &artifact,
                RunResultLimits {
                    max_artifact_bytes: bytes.len() - 1,
                    ..limits
                }
            )
            .unwrap_err()
            .contains("byte budget")
        );
        assert!(
            decode_run_artifact(
                &bytes,
                RunResultLimits {
                    max_artifact_bytes: bytes.len() - 1,
                    ..limits
                }
            )
            .unwrap_err()
            .contains("byte budget")
        );
        let malformed = String::from_utf8(bytes.clone())
            .expect("JSON")
            .replace("\"x\":[1.0,2.0,3.0]", "\"x\":[]");
        assert!(
            decode_run_artifact(malformed.as_bytes(), limits)
                .unwrap_err()
                .contains("series length")
        );
        let mut oversized = artifact.clone();
        Arc::make_mut(&mut oversized.definition)
            .overrides
            .insert(ParamPath("p".into()), ParamValue::RealArray(vec![0.0; 7]));
        let bytes = serde_json::to_vec(&oversized).expect("negative authored bytes");
        assert!(
            decode_run_artifact(&bytes, limits)
                .unwrap_err()
                .contains("scalar budget")
        );
        let mut unavailable = artifact;
        Arc::make_mut(&mut unavailable.result).meta.source_content =
            Some(SourceContentIdentity::Unavailable {
                reason: "parsed definitions only".into(),
            });
        assert!(
            encode_run_artifact(&unavailable, limits)
                .unwrap_err()
                .contains("source identity")
        );
    }
}
