//! Persistent reuse of checked FMI components. Only a retained, immutable root crosses to the writer.

use super::{ModelicaArtifactDiskLimits, ModelicaCacheReadError, ModelicaCacheWriteError};
use crossbeam_channel::Sender;
use lunco_assets_core::cache_policy::{ArtifactCacheDecision, artifact_cache_decision};
use lunco_hash::content::{Cid, ContentHasher};
use lunco_storage::{FileStorage, StorageHandle};
use rumoca_eval_solve::fmi_wire::{
    FMI_COMPONENT_SCHEMA_VERSION, deserialize_fmi_component, retained_fmi_component_wire,
};
use rumoca_ir_solve::{RetainedAssertionRoot, fmi::FmiComponent};
use rumoca_sim::{SimOptions, SimulationDiagnosticError, SimulationSession};

struct Publication {
    key: Cid,
    root: RetainedAssertionRoot,
}

enum Command {
    Publish(Publication),
    Stop,
}

pub(in crate::worker) struct Cache {
    limits: ModelicaArtifactDiskLimits,
    writer: Sender<Command>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Cache {
    pub(in crate::worker) fn new(limits: ModelicaArtifactDiskLimits) -> Result<Self, String> {
        limits.validate()?;
        // Bound pending roots. A busy writer applies backpressure only on preparation threads.
        let (writer, jobs) = crossbeam_channel::bounded::<Command>(1);
        let thread = std::thread::Builder::new()
            .name("modelica-component-cache".into())
            .spawn(move || {
                while let Ok(Command::Publish(job)) = jobs.recv() {
                    if let Err(error) = job.publish(limits) {
                        log::warn!("prepared Modelica component publication failed: {error}");
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            limits,
            writer,
            thread: Some(thread),
        })
    }

    pub(in crate::worker) fn request(
        &self,
        artifact: &crate::worker::CompiledModelArtifact,
        doc_uri: &str,
    ) -> Option<Request> {
        let build =
            rumoca_tool_build_info::build_identity().filter(|id| !id.ends_with("-dirty"))?;
        if !artifact.compiled.dae.external_tables().is_empty() {
            return None;
        }
        let mut hash = ContentHasher::default();
        hash.update(b"LunCoPreparedFmiComponent-v1");
        hash.update(&FMI_COMPONENT_SCHEMA_VERSION.to_le_bytes());
        field(&mut hash, build.as_bytes());
        field(&mut hash, std::env::consts::OS.as_bytes());
        field(&mut hash, std::env::consts::ARCH.as_bytes());
        field(&mut hash, &artifact.binding_schema.to_bytes());
        field(&mut hash, &artifact.source_content.to_bytes());
        values(&mut hash, &artifact.structural_overrides);
        Some(Request {
            source: hash.finish(),
            doc_uri: doc_uri.into(),
            writer: self.writer.clone(),
            limits: self.limits,
        })
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        let _ = self.writer.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone)]
pub(in crate::worker) struct Request {
    source: Cid,
    doc_uri: String,
    writer: Sender<Command>,
    limits: ModelicaArtifactDiskLimits,
}

fn field(hash: &mut ContentHasher, bytes: &[u8]) {
    hash.update(&(bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

fn values(hash: &mut ContentHasher, values: &[(String, f64)]) {
    hash.update(&(values.len() as u64).to_le_bytes());
    for (name, value) in values {
        field(hash, name.as_bytes());
        hash.update(&value.to_bits().to_le_bytes());
    }
}

fn component_key(source: &Cid, solver: &str, options: &SimOptions) -> Cid {
    let mut hash = ContentHasher::default();
    field(&mut hash, &source.to_bytes());
    field(&mut hash, solver.as_bytes());
    hash.update(&[options.scalarize as u8]);
    values(&mut hash, &options.param_overrides);
    values(&mut hash, &options.initial_inputs);
    values(&mut hash, &options.start_overrides);
    hash.finish()
}

fn path(key: &Cid) -> std::path::PathBuf {
    lunco_assets_core::modelica_dir()
        .join("prepared-fmi-v1")
        .join(super::artifact_filename(key))
}

impl Request {
    pub(in crate::worker) fn construct(
        &self,
        dae: &rumoca_ir_dae::Dae,
        options: SimOptions,
        solver: &str,
    ) -> Result<SimulationSession, SimulationDiagnosticError> {
        let key = component_key(&self.source, solver, &options);
        let handle = StorageHandle::File(path(&key));
        let storage = FileStorage::new();
        let started = std::time::Instant::now();
        let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.read(&storage, &handle, &key)
        }))
        .unwrap_or_else(|_| {
            Err(ModelicaCacheReadError::Codec(serde_json::Error::io(
                std::io::Error::other("prepared component decoder panicked"),
            )))
        });
        let cached = match decoded {
            Ok(component) => component,
            Err(error) => {
                log::warn!(
                    "prepared Modelica component rejected for {}: {error}",
                    self.doc_uri
                );
                None
            }
        };
        let decision = artifact_cache_decision(
            "modelica_prepared",
            crate::worker::modelica_cache_scope(&self.doc_uri),
            &self.doc_uri,
            &key.to_string(),
            cached.is_some(),
            false,
        )
        .map_err(SimulationDiagnosticError::Solver)?;
        let component = if let Some(component) =
            cached.filter(|_| decision == ArtifactCacheDecision::Reuse)
        {
            if let Err(error) = storage.mark_cache_file_used(&handle) {
                log::warn!("prepared Modelica component use was not recorded: {error}");
            }
            log::info!(
                "reused prepared Modelica component for {} in {:.3} s",
                self.doc_uri,
                started.elapsed().as_secs_f64()
            );
            component
        } else {
            let prepared = SimulationSession::prepare_component(dae, &options)?;
            let lowered = started.elapsed();
            let root = prepared.replay_root();
            let reusable = root.is_some();
            if let Some(root) = root
                && self
                    .writer
                    .send(Command::Publish(Publication { key, root }))
                    .is_err()
            {
                log::warn!("prepared Modelica component writer disconnected");
            }
            log::info!(
                "prepared Modelica component for {} in {:.3} s, reusable={reusable}, publication queue wait {:.3} s",
                self.doc_uri,
                lowered.as_secs_f64(),
                started.elapsed().saturating_sub(lowered).as_secs_f64()
            );
            prepared.into_component()
        };
        SimulationSession::from_prepared_component(component, options)
    }

    fn read(
        &self,
        storage: &FileStorage,
        handle: &StorageHandle,
        key: &Cid,
    ) -> Result<Option<FmiComponent>, ModelicaCacheReadError> {
        let Some(bytes) = super::read_payload(storage, handle, self.limits)? else {
            return Ok(None);
        };
        let payload = bytes
            .strip_prefix(key.to_bytes().as_slice())
            .ok_or(ModelicaCacheReadError::Identity)?;
        let mut reader = serde_json::Deserializer::from_slice(payload);
        let component =
            deserialize_fmi_component(&mut reader).map_err(ModelicaCacheReadError::Codec)?;
        reader.end().map_err(ModelicaCacheReadError::Codec)?;
        Ok(Some(component))
    }
}

impl Publication {
    fn publish(self, limits: ModelicaArtifactDiskLimits) -> Result<(), ModelicaCacheWriteError> {
        let wire = retained_fmi_component_wire(&self.root)
            .map_err(|error| ModelicaCacheWriteError::ConstructionWire(error.to_string()))?;
        let encoded = super::encode_payload(limits, |writer| {
            writer
                .write_all(&self.key.to_bytes())
                .map_err(ModelicaCacheWriteError::Compression)?;
            serde_json::to_writer(writer, &wire).map_err(ModelicaCacheWriteError::Codec)
        })?;
        super::publish_record(&path(&self.key), &encoded, limits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_identity_preserves_input_role_name_bits_and_solver() {
        let source = ContentHasher::default().finish();
        let mut options = SimOptions::default();
        options.initial_inputs = vec![("v".into(), 0.0)];
        let initial = component_key(&source, "rk45", &options);
        options.initial_inputs[0].1 = -0.0;
        assert_ne!(initial, component_key(&source, "rk45", &options));
        options.param_overrides = std::mem::take(&mut options.initial_inputs);
        let parameter = component_key(&source, "rk45", &options);
        options.start_overrides = std::mem::take(&mut options.param_overrides);
        assert_ne!(parameter, component_key(&source, "rk45", &options));
        assert_ne!(
            component_key(&source, "bdf", &options),
            component_key(&source, "rk45", &options)
        );
    }
}
