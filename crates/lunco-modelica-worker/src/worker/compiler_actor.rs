//! Single-owner native Rumoca session actor.
//!
//! Source-root installation mutates Rumoca's session, so it shares one FIFO
//! mailbox with every compile request. The simulation worker sends immutable
//! source units and commits returned artifacts on its own ordered lane.

use super::{
    BackendCompileResult, CompileUnit, ModelicaCompiler, PreparedSourceRoot,
    WorkerPreparationResult, compile_shared,
};
use crossbeam_channel::{Receiver, Sender};
use std::collections::HashMap;
use std::thread::JoinHandle;

const MAX_PENDING_COMPILER_OPERATIONS: usize = 4;

pub(super) enum CompilerCompletion {
    Compile {
        id: u64,
        artifact: BackendCompileResult,
    },
    SourceRoot {
        id: u64,
        commit: SourceRootCommit,
    },
}

pub(super) struct SourceRootCommit {
    pub root_id: String,
    pub error: Option<String>,
    pub inserted_file_count: usize,
    pub parsed_file_count: usize,
    pub library_defaults: HashMap<String, f64>,
    pub library_revision: u64,
}

enum Request {
    CompileAsync {
        id: u64,
        model_name: String,
        unit: CompileUnit,
        doc_uri: String,
        library_gen: u64,
    },
    InstallSourceRoot {
        id: u64,
        prepared: PreparedSourceRoot,
    },
}

pub(super) struct CompilerActor {
    tx: Option<Sender<Request>>,
    next_id: u64,
    thread: Option<JoinHandle<()>>,
}

impl CompilerActor {
    pub(super) fn new(results: Sender<WorkerPreparationResult>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let thread = std::thread::Builder::new()
            .name("modelica-rumoca-owner".to_owned())
            .stack_size(16 * 1024 * 1024)
            .spawn(move || compiler_actor_loop(rx, results))
            .expect("Modelica compiler actor thread must be constructible");
        Self {
            tx: Some(tx),
            next_id: 1,
            thread: Some(thread),
        }
    }

    pub(super) fn can_submit(&self, pending: usize) -> bool {
        pending < MAX_PENDING_COMPILER_OPERATIONS
    }

    pub(super) fn submit_compile(
        &mut self,
        model_name: String,
        unit: CompileUnit,
        doc_uri: String,
        library_gen: u64,
    ) -> Result<u64, (String, CompileUnit)> {
        let id = match self.allocate_id() {
            Ok(id) => id,
            Err(error) => return Err((error, unit)),
        };
        match self.sender().send(Request::CompileAsync {
            id,
            model_name,
            unit,
            doc_uri,
            library_gen,
        }) {
            Ok(()) => Ok(id),
            Err(error) => match error.0 {
                Request::CompileAsync { unit, .. } => {
                    Err(("Rumoca compiler actor is unavailable".to_owned(), unit))
                }
                _ => unreachable!("the failed request was a compile"),
            },
        }
    }

    pub(super) fn submit_source_root(
        &mut self,
        prepared: PreparedSourceRoot,
    ) -> Result<u64, (String, PreparedSourceRoot)> {
        let id = match self.allocate_id() {
            Ok(id) => id,
            Err(error) => return Err((error, prepared)),
        };
        match self
            .sender()
            .send(Request::InstallSourceRoot { id, prepared })
        {
            Ok(()) => Ok(id),
            Err(error) => match error.0 {
                Request::InstallSourceRoot { prepared, .. } => {
                    Err(("Rumoca compiler actor is unavailable".to_owned(), prepared))
                }
                _ => unreachable!("the failed request was a source-root install"),
            },
        }
    }

    fn sender(&self) -> &Sender<Request> {
        self.tx
            .as_ref()
            .expect("compiler actor sender is present until drop")
    }

    fn allocate_id(&mut self) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or_else(|| "Rumoca compiler operation identity exhausted".to_owned())?;
        Ok(id)
    }
}

impl Drop for CompilerActor {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                bevy::log::error!("[modelica-runtime] Rumoca compiler actor panicked on shutdown");
            }
        }
    }
}

fn compiler_actor_loop(rx: Receiver<Request>, results: Sender<WorkerPreparationResult>) {
    let mut compiler: Option<ModelicaCompiler> = None;
    let mut compiled_artifacts = HashMap::new();
    let mut terminal_error: Option<String> = None;

    while let Ok(request) = rx.recv() {
        match request {
            Request::CompileAsync {
                id,
                model_name,
                unit,
                doc_uri,
                library_gen,
            } => {
                let artifact = compile_artifact(
                    &mut compiler,
                    &mut compiled_artifacts,
                    &mut terminal_error,
                    &model_name,
                    unit,
                    &doc_uri,
                    library_gen,
                );
                if results
                    .send(WorkerPreparationResult::Compiler(
                        CompilerCompletion::Compile { id, artifact },
                    ))
                    .is_err()
                {
                    bevy::log::error!("[modelica-runtime] Rumoca compile result dropped id={id}");
                    return;
                }
            }
            Request::InstallSourceRoot { id, prepared } => {
                let commit = install_source_root(
                    &mut compiler,
                    &mut compiled_artifacts,
                    &mut terminal_error,
                    prepared,
                );
                if results
                    .send(WorkerPreparationResult::Compiler(
                        CompilerCompletion::SourceRoot { id, commit },
                    ))
                    .is_err()
                {
                    bevy::log::error!(
                        "[modelica-runtime] Rumoca source-root result dropped id={id}"
                    );
                    return;
                }
            }
        }
    }
}

fn compile_artifact(
    compiler: &mut Option<ModelicaCompiler>,
    compiled_artifacts: &mut HashMap<u64, Box<rumoca_compile::compile::DaeCompilationResult>>,
    terminal_error: &mut Option<String>,
    model_name: &str,
    mut unit: CompileUnit,
    doc_uri: &str,
    library_gen: u64,
) -> BackendCompileResult {
    let started = web_time::Instant::now();
    if let Some(error) = terminal_error {
        return BackendCompileResult {
            unit,
            outcome: Err(format!("Rumoca compiler actor faulted: {error}")),
            diagnostics: Vec::new(),
            library_revision: 0,
        };
    }
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
        unit.merge_library_defaults(compiler.library_input_defaults());
        let outcome = compile_shared(
            compiled_artifacts,
            compiler,
            model_name,
            &unit,
            doc_uri,
            library_gen,
        );
        let diagnostics = if outcome.is_err() {
            compiler.compile_diagnostics(model_name, doc_uri)
        } else {
            Vec::new()
        };
        (outcome, diagnostics, compiler.library_revision())
    }));
    let (outcome, diagnostics, library_revision) = match outcome {
        Ok(result) => result,
        Err(payload) => {
            let message = panic_message(payload.as_ref());
            let fault = format!("compile `{model_name}` panicked: {message}");
            *terminal_error = Some(fault.clone());
            *compiler = None;
            compiled_artifacts.clear();
            (Err(fault), Vec::new(), 0)
        }
    };
    let elapsed = started.elapsed();
    if elapsed > std::time::Duration::from_secs(2) {
        bevy::log::warn!(
            "[modelica-runtime] Rumoca compile `{model_name}` took {elapsed:?} on the compiler actor"
        );
    } else {
        bevy::log::debug!(
            "[modelica-runtime] Rumoca compile `{model_name}` completed in {elapsed:?}"
        );
    }
    BackendCompileResult {
        unit,
        outcome,
        diagnostics,
        library_revision,
    }
}

fn install_source_root(
    compiler: &mut Option<ModelicaCompiler>,
    compiled_artifacts: &mut HashMap<u64, Box<rumoca_compile::compile::DaeCompilationResult>>,
    terminal_error: &mut Option<String>,
    prepared: PreparedSourceRoot,
) -> SourceRootCommit {
    let root_id = prepared.source_set_id().to_owned();
    let started = web_time::Instant::now();
    if let Some(error) = terminal_error {
        return SourceRootCommit {
            root_id,
            error: Some(format!("Rumoca compiler actor faulted: {error}")),
            inserted_file_count: 0,
            parsed_file_count: 0,
            library_defaults: HashMap::new(),
            library_revision: 0,
        };
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compiler
            .get_or_insert_with(ModelicaCompiler::new)
            .install_source_root(prepared)
    }));
    let commit = match result {
        Ok(report) => {
            let error = (!report.diagnostics.is_empty()).then(|| report.diagnostics.join("; "));
            let compiler = compiler
                .as_ref()
                .expect("source-root installation initializes the compiler");
            if report.inserted_file_count > 0 {
                compiled_artifacts.clear();
            }
            SourceRootCommit {
                root_id: root_id.clone(),
                error,
                inserted_file_count: report.inserted_file_count,
                parsed_file_count: report.parsed_file_count,
                library_defaults: compiler.library_input_defaults().clone(),
                library_revision: compiler.library_revision(),
            }
        }
        Err(payload) => {
            let message = panic_message(payload.as_ref());
            let fault = format!("source-root `{root_id}` installation panicked: {message}");
            *terminal_error = Some(fault.clone());
            *compiler = None;
            compiled_artifacts.clear();
            SourceRootCommit {
                root_id: root_id.clone(),
                error: Some(fault),
                inserted_file_count: 0,
                parsed_file_count: 0,
                library_defaults: HashMap::new(),
                library_revision: 0,
            }
        }
    };
    let elapsed = started.elapsed();
    if elapsed > std::time::Duration::from_secs(2) {
        bevy::log::warn!(
            "[modelica-runtime] Rumoca source-root `{root_id}` install took {elapsed:?}"
        );
    } else {
        bevy::log::debug!(
            "[modelica-runtime] Rumoca source-root `{root_id}` installed in {elapsed:?}"
        );
    }
    commit
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic payload")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn source_root_and_compile_share_one_fifo_owner() {
        let (results_tx, results_rx) = crossbeam_channel::unbounded();
        let mut actor = CompilerActor::new(results_tx);
        let root_id = actor
            .submit_source_root(PreparedSourceRoot::prepare(
                "actor-test-root",
                "inline actor test",
                vec![(
                    "ActorLibrary/Rate.mo".into(),
                    "within ActorLibrary;\nmodel Rate\n  output Real y;\nequation\n  y = 1;\nend Rate;\n".into(),
                )],
                Vec::new(),
            ))
            .unwrap_or_else(|_| panic!("source-root operation should be admitted"));
        let compile_id = actor
            .submit_compile(
                "ActorCompileProbe".into(),
                CompileUnit {
                    source: "model ActorCompileProbe\n  ActorLibrary.Rate rate;\n  Real x(start=0);\nequation\n  der(x) = rate.y;\nend ActorCompileProbe;\n".into(),
                    extras: Vec::new(),
                    input_defaults: HashMap::new(),
                    default_diagnostics: Vec::new(),
                },
                "ActorCompileProbe.mo".into(),
                0,
            )
            .unwrap_or_else(|_| panic!("compile operation should be admitted"));

        match results_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("source-root result should arrive")
        {
            WorkerPreparationResult::Compiler(CompilerCompletion::SourceRoot { id, commit }) => {
                assert_eq!(id, root_id);
                assert_eq!(commit.root_id, "actor-test-root");
                assert_eq!(commit.inserted_file_count, 1);
                assert_eq!(commit.error, None);
            }
            _ => panic!("source-root result must precede the later compile result"),
        }
        match results_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("compile result should arrive")
        {
            WorkerPreparationResult::Compiler(CompilerCompletion::Compile { id, artifact }) => {
                assert_eq!(id, compile_id);
                if let Err(error) = artifact.outcome {
                    panic!("Modelica actor compile failed: {error}");
                }
                assert!(artifact.unit.source.contains("ActorCompileProbe"));
            }
            _ => panic!("compile result must follow source-root installation"),
        }
    }
}
