//! In-memory and persistent prepared-solve cache for the Modelica worker.
//!
//! Library admission clears worker-local solve models. Persistent entries stay
//! usable because their keys include the deterministic admitted-library
//! revision, solver identity, structural source identity, and parameter values.

#[cfg(not(target_arch = "wasm32"))]
use super::PREPARED_SOLVE_CACHE_VERSION;
use super::solver;
#[cfg(not(target_arch = "wasm32"))]
use lunco_assets_core::modelica_dir;
#[cfg(not(target_arch = "wasm32"))]
use lunco_storage::{read_file_sync, write_file_sync};
#[cfg(not(target_arch = "wasm32"))]
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A prepared solve model is reusable for the exact structural source key,
/// admitted library revision, solver, and parameter override vector that
/// produced it. This is the same identity used by the persistent cache, so
/// equivalent generated networks share pure solve IR within one session too.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct PreparedSolveKey {
    pub(super) source_key: u64,
    pub(super) library_revision: u64,
    pub(super) solver_id: String,
    pub(super) parameter_overrides: Vec<(String, u64)>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Deserialize, Serialize)]
struct PreparedSolveDiskRecord {
    version: u32,
    source_key: u64,
    library_revision: u64,
    solver_id: String,
    parameter_overrides: Vec<(String, u64)>,
    model: rumoca_ir_solve::SolveModel,
}

#[derive(Default)]
pub(super) struct PreparedSolveCache {
    pub(super) models: HashMap<PreparedSolveKey, rumoca_ir_solve::SolveModel>,
}

impl PreparedSolveCache {
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn new() -> Self {
        Self {
            models: HashMap::default(),
        }
    }

    pub(super) fn key(
        source_key: u64,
        library_revision: u64,
        spec: &solver::SolverSpec,
        parameter_overrides: &[(String, f64)],
    ) -> PreparedSolveKey {
        PreparedSolveKey {
            source_key,
            library_revision,
            solver_id: spec.id.to_string(),
            parameter_overrides: parameter_overrides
                .iter()
                .map(|(name, value)| (name.clone(), value.to_bits()))
                .collect(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.models.clear();
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn disk_path(
        source_key: u64,
        library_revision: u64,
        solver_id: &str,
        parameter_overrides: &[(String, u64)],
    ) -> std::path::PathBuf {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        PREPARED_SOLVE_CACHE_VERSION.hash(&mut hasher);
        source_key.hash(&mut hasher);
        library_revision.hash(&mut hasher);
        solver_id.hash(&mut hasher);
        parameter_overrides.hash(&mut hasher);
        let key = hasher.finish();
        modelica_dir()
            .join("prepared-solve-v5")
            .join(format!("{source_key:016x}-{key:016x}.bin.zst"))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn load_disk(
        source_key: u64,
        library_revision: u64,
        solver_id: &str,
        parameter_overrides: &[(String, u64)],
    ) -> Option<rumoca_ir_solve::SolveModel> {
        let path = Self::disk_path(source_key, library_revision, solver_id, parameter_overrides);
        let compressed = read_file_sync(&path).ok()?;
        let bytes = zstd::stream::decode_all(compressed.as_slice()).ok()?;
        let (record, _): (PreparedSolveDiskRecord, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).ok()?;
        if record.version != PREPARED_SOLVE_CACHE_VERSION
            || record.source_key != source_key
            || record.library_revision != library_revision
            || record.solver_id != solver_id
            || record.parameter_overrides != parameter_overrides
        {
            return None;
        }
        Some(record.model)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn save_disk(
        source_key: u64,
        library_revision: u64,
        solver_id: &str,
        parameter_overrides: &[(String, u64)],
        model: &rumoca_ir_solve::SolveModel,
    ) {
        let path = Self::disk_path(source_key, library_revision, solver_id, parameter_overrides);
        let record = PreparedSolveDiskRecord {
            version: PREPARED_SOLVE_CACHE_VERSION,
            source_key,
            library_revision,
            solver_id: solver_id.to_owned(),
            parameter_overrides: parameter_overrides.to_vec(),
            model: model.clone(),
        };
        let Ok(bytes) = bincode::serde::encode_to_vec(record, bincode::config::standard()) else {
            return;
        };
        let Ok(compressed) = zstd::stream::encode_all(bytes.as_slice(), 3) else {
            return;
        };
        // Storage performs the native atomic replacement and owns the
        // platform-specific persistence path.
        let _ = write_file_sync(&path, &compressed);
    }
}
