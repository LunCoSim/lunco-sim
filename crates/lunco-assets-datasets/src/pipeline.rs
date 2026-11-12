//! Ordered, manifest-owned processing recipes and their portable artifact receipts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{AssetEntry, ProcessConfig};

/// A recipe publishes one directory; individual steps own only scratch outputs.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineConfig {
    /// Additional raw downloads, keyed by the names used by steps.
    #[serde(default)]
    pub sources: BTreeMap<String, AssetEntry>,
    /// Authored code assets, relative to the owning Twin/engine root.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// Optional pure Rhai plan customization; uses the common hook interpreter.
    pub planner: Option<PipelineCallable>,
    /// Steps execute in declaration order, reading only earlier results or inputs.
    pub steps: Vec<PipelineStep>,
    /// The exact files that constitute the delivered product.
    pub artifacts: Vec<PipelineArtifact>,
}

/// A callable code asset in `files`, with an explicit function name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineCallable {
    /// Declared file name, whose bytes participate in the bake identity.
    pub source: String,
    /// Function to call with immutable recipe facts.
    pub entry: String,
}

/// One operation. Scalar fields not listed here override inherited process settings.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PipelineStep {
    /// Unique stable step name.
    pub id: String,
    /// `source`, a named raw/code input, or an earlier step id.
    pub input: String,
    /// Registered native operation or `external`.
    pub kind: String,
    /// Portable scratch-relative output filename or directory.
    pub output: String,
    /// Operation settings overriding the recipe's common process settings.
    #[serde(flatten)]
    pub parameters: BTreeMap<String, toml::Value>,
}

/// A validated file copied from a step output into the published directory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineArtifact {
    /// Producing step id.
    pub from: String,
    /// Optional file inside a directory-valued step output.
    pub source: Option<String>,
    /// Consumer-facing path relative to the final recipe directory.
    pub path: String,
    /// `dem`, `image`, or `file`.
    pub kind: String,
    /// Required image dimensions, when declared.
    pub dimensions: Option<[u32; 2]>,
}

/// Integrity record for one published file.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ArtifactDigest {
    /// Consumer-facing relative file path.
    pub path: String,
    /// SHA-256 of the published bytes.
    pub sha256: String,
}

/// Portable receipt shipped with an artifact bundle.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PipelineReceipt {
    /// Hash of the authored recipe, independent of machine-specific paths.
    pub declaration: String,
    /// Hashes of every admitted input and code asset.
    pub inputs: BTreeMap<String, String>,
    /// Resolved operation settings after inherited defaults and customization.
    pub steps: Vec<ExecutedStep>,
    /// Complete output integrity inventory.
    pub artifacts: Vec<ArtifactDigest>,
}

/// An executed step's portable recipe provenance.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExecutedStep {
    /// Stable step identity.
    pub id: String,
    /// Input identity inside this recipe.
    pub input: String,
    /// Full native configuration that was actually executed.
    pub config: ProcessConfig,
}

/// The receipt is owned by the same transaction as its artifact files.
pub const RECEIPT: &str = ".pipeline.toml";

fn invalid(detail: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, detail.into())
}

fn name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

fn relative(value: &str) -> bool {
    lunco_assets_path::is_portable_relative_path(value)
        && !value
            .split('/')
            .any(|part| matches!(part, RECEIPT | ".bakekey" | ".work" | ".logs"))
}

fn code_name(value: &str) -> bool {
    value.split("::").all(name)
}

impl PipelineConfig {
    /// Validate topology and ownership before downloading or running any code.
    pub fn validate(&self) -> std::io::Result<()> {
        if self.steps.is_empty() || self.artifacts.is_empty() {
            return Err(invalid("pipeline requires steps and declared artifacts"));
        }
        let mut available = BTreeSet::from(["source".to_owned()]);
        let mut destinations = BTreeSet::new();
        for (id, source) in &self.sources {
            if !name(id) || !available.insert(id.clone()) || source.process.is_some() {
                return Err(invalid(format!(
                    "invalid or duplicate raw pipeline input '{id}'"
                )));
            }
            if let Some(dest) = &source.dest {
                if !relative(dest) || !destinations.insert(dest.clone()) {
                    return Err(invalid(format!(
                        "invalid or duplicate pipeline source destination '{dest}'"
                    )));
                }
            }
        }
        for (id, path) in &self.files {
            let valid_path = lunco_assets_core::parse_lunco_uri(path)
                .map(relative)
                .unwrap_or_else(|| relative(path));
            if !code_name(id) || !available.insert(id.clone()) || !valid_path {
                return Err(invalid(format!("invalid pipeline code input '{id}'")));
            }
        }
        if let Some(planner) = &self.planner {
            if !self.files.contains_key(&planner.source) || !name(&planner.entry) {
                return Err(invalid(
                    "pipeline planner requires a declared code input and entry",
                ));
            }
        }
        let mut steps = BTreeSet::new();
        let mut outputs: Vec<PathBuf> = Vec::new();
        for step in &self.steps {
            if !name(&step.id)
                || !available.contains(&step.input)
                || !available.insert(step.id.clone())
                || !relative(&step.output)
                || step.kind == "pipeline"
                || step.parameters.contains_key("pipeline")
                || step.parameters.contains_key("output_root")
                || step.output == "inputs"
                || step.output.starts_with("inputs/")
            {
                return Err(invalid(format!(
                    "invalid pipeline step '{}' (input must precede its consumer)",
                    step.id
                )));
            }
            let output = PathBuf::from(&step.output);
            if outputs
                .iter()
                .any(|other| output.starts_with(other) || other.starts_with(&output))
            {
                return Err(invalid(format!(
                    "pipeline scratch output '{}' overlaps another step",
                    step.output
                )));
            }
            outputs.push(output);
            steps.insert(step.id.clone());
        }
        let mut artifacts: Vec<PathBuf> = Vec::new();
        for artifact in &self.artifacts {
            let path = PathBuf::from(&artifact.path);
            if !steps.contains(&artifact.from)
                || !relative(&artifact.path)
                || artifact
                    .source
                    .as_deref()
                    .is_some_and(|source| !relative(source))
                || !matches!(artifact.kind.as_str(), "dem" | "image" | "file")
                || artifact.dimensions.is_some_and(|[w, h]| w == 0 || h == 0)
                || artifacts
                    .iter()
                    .any(|other| path.starts_with(other) || other.starts_with(&path))
            {
                return Err(invalid(format!(
                    "invalid or overlapping pipeline artifact '{}'",
                    artifact.path
                )));
            }
            artifacts.push(path);
        }
        Ok(())
    }

    /// Lower one step to the authoritative native processor configuration.
    pub fn step_config(
        &self,
        common: &ProcessConfig,
        step: &PipelineStep,
    ) -> std::io::Result<ProcessConfig> {
        let mut table = toml::Table::try_from(common).map_err(|e| invalid(e.to_string()))?;
        table.remove("pipeline");
        table.insert("kind".into(), step.kind.clone().into());
        table.insert("output".into(), step.output.clone().into());
        table.insert("output_root".into(), "cache".into());
        table.extend(step.parameters.clone());
        table
            .try_into()
            .map_err(|e| invalid(format!("step '{}': {e}", step.id)))
    }
}

/// Hash bytes without exposing machine-local source paths in artifact identity.
pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The authored declaration identity, including the processor version.
pub fn declaration_key(config: &ProcessConfig) -> std::io::Result<String> {
    let mut bytes = toml::to_string(config)
        .map_err(|e| invalid(e.to_string()))?
        .into_bytes();
    bytes.extend(crate::PROCESS_PIPELINE_VERSION.to_le_bytes());
    Ok(digest(&bytes))
}

/// Resolve declared inputs through the shared asset owner on the processing worker.
#[cfg(not(target_arch = "wasm32"))]
pub fn input_paths(
    config: &PipelineConfig,
    source: &Path,
    owner: Option<&Path>,
) -> std::io::Result<BTreeMap<String, PathBuf>> {
    config.validate()?;
    let mut paths = BTreeMap::from([("source".to_owned(), source.to_path_buf())]);
    let root = owner
        .map(Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(lunco_assets_core::assets_dir_abs)?;
    let roots = if owner.is_some() {
        lunco_assets_core::asset_read_roots(&root)
    } else {
        lunco_assets_core::engine_asset_read_roots(&root)
    };
    for (id, input) in &config.sources {
        let path = crate::source_read_path(input, &roots)?;
        if !path.is_file() {
            return Err(invalid(format!(
                "pipeline input '{id}' must be an individual file; declare its data files explicitly"
            )));
        }
        paths.insert(id.clone(), path);
    }
    if !config.files.is_empty() {
        for (id, relative) in &config.files {
            let engine_relative = lunco_assets_core::parse_lunco_uri(relative);
            let code_roots = match engine_relative {
                Some(_) => {
                    lunco_assets_core::engine_asset_read_roots(&lunco_assets_core::assets_dir_abs()?)
                }
                None => roots.clone(),
            };
            let path = lunco_assets_core::resolve_native_asset(
                &code_roots,
                Path::new(engine_relative.unwrap_or(relative)),
                Some(lunco_storage::StorageEntryKind::File),
            )?
            .ok_or_else(|| invalid(format!("pipeline code input '{relative}' is missing")))?
            .1;
            paths.insert(id.clone(), path);
        }
    }
    Ok(paths)
}

/// Compare a delivered receipt with the exact currently admitted input bytes.
#[cfg(not(target_arch = "wasm32"))]
pub fn inputs_current(
    output: &Path,
    config: &ProcessConfig,
    source: &Path,
    owner: Option<&Path>,
) -> bool {
    let Some(pipeline) = &config.pipeline else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(output.join(RECEIPT)) else {
        return false;
    };
    let Ok(receipt) = toml::from_str::<PipelineReceipt>(&text) else {
        return false;
    };
    input_paths(pipeline, source, owner)
        .and_then(|paths| input_digests(&paths))
        .is_ok_and(|current| current == receipt.inputs)
}

/// Hash all actual admitted source/code bytes. Reused by bakes and readiness scans.
#[cfg(not(target_arch = "wasm32"))]
pub fn input_digests(
    paths: &BTreeMap<String, PathBuf>,
) -> std::io::Result<BTreeMap<String, String>> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    paths
        .iter()
        .map(|(id, path)| {
            let mut file = std::fs::File::open(path)?;
            let mut hash = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
            Ok((
                id.clone(),
                hash.finalize()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            ))
        })
        .collect()
}

/// Check a portable bundle against its manifest and complete output inventory.
#[cfg(not(target_arch = "wasm32"))]
pub fn bundle_present(path: &Path, config: &ProcessConfig) -> bool {
    let Some(pipeline) = &config.pipeline else {
        return false;
    };
    if pipeline.validate().is_err() {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(path.join(RECEIPT)) else {
        return false;
    };
    let Ok(receipt) = toml::from_str::<PipelineReceipt>(&text) else {
        return false;
    };
    if declaration_key(config).ok().as_ref() != Some(&receipt.declaration)
        || receipt.artifacts.len() != pipeline.artifacts.len()
    {
        return false;
    }
    pipeline.artifacts.iter().all(|artifact| {
        let Some(record) = receipt
            .artifacts
            .iter()
            .find(|record| record.path == artifact.path)
        else {
            return false;
        };
        lunco_assets_core::resolve_native_asset(
            &[path.to_path_buf()],
            Path::new(&artifact.path),
            Some(lunco_storage::StorageEntryKind::File),
        )
        .ok()
        .flatten()
        .is_some_and(|(_, file)| {
            std::fs::read(file)
                .is_ok_and(|bytes| !bytes.is_empty() && digest(&bytes) == record.sha256)
        })
    })
}
