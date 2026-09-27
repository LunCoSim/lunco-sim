use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Mutex, mpsc};

use bevy::prelude::*;
use lunco_doc_bevy::DocumentRegistry;
use lunco_sysml::SysmlDocument;
use lunco_workspace::{TwinClosed, TwinId, WorkspaceResource};

use crate::view_model::SysmlRequirementsViewModel;

#[derive(Event, Clone, Debug)]
pub(crate) struct RunSysmlVerification {
    pub twin_id: TwinId,
    pub source_revision: u64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum VerificationRunOutcome {
    Passed,
    Failed,
    NoVerdict,
    Error(String),
}

#[derive(Clone, Debug)]
pub(crate) struct VerificationRunResult {
    pub twin_id: TwinId,
    pub source_revision: u64,
    pub name: String,
    pub outcome: VerificationRunOutcome,
    pub summary: String,
}

#[derive(Resource, Default)]
pub(crate) struct SysmlVerificationRuns {
    active: Option<ActiveRun>,
    completed: HashMap<u64, HashMap<String, VerificationRunResult>>,
}

struct ActiveRun {
    twin_id: TwinId,
    source_revision: u64,
    name: String,
    child: Child,
    output: Mutex<Receiver<ProcessOutput>>,
    stdout: Option<Result<String, String>>,
    stderr: Option<Result<String, String>>,
    exit_status: Option<ExitStatus>,
}

impl Drop for ActiveRun {
    fn drop(&mut self) {
        if self.exit_status.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

enum ProcessStream {
    Stdout,
    Stderr,
}

struct ProcessOutput {
    stream: ProcessStream,
    content: Result<String, String>,
}

impl SysmlVerificationRuns {
    pub(crate) fn result(&self, twin_id: TwinId, name: &str) -> Option<&VerificationRunResult> {
        self.completed.get(&twin_id.raw())?.get(name)
    }

    pub(crate) fn is_running(&self, twin_id: TwinId, name: &str) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.twin_id == twin_id && active.name == name)
    }

    pub(crate) fn has_active_run(&self) -> bool {
        self.active.is_some()
    }

    pub(crate) fn active_case(&self) -> Option<(TwinId, &str)> {
        self.active
            .as_ref()
            .map(|active| (active.twin_id, active.name.as_str()))
    }
}

pub(crate) fn start_sysml_verification(
    trigger: On<RunSysmlVerification>,
    workspace: Option<Res<WorkspaceResource>>,
    view_model: Option<Res<SysmlRequirementsViewModel>>,
    documents: Option<Res<DocumentRegistry<SysmlDocument>>>,
    mut runs: ResMut<SysmlVerificationRuns>,
) {
    let request = trigger.event();
    if runs.active.is_some() {
        return;
    }

    let Some(workspace) = workspace else {
        return store_setup_error(&mut runs, request, "workspace is unavailable");
    };
    if workspace.active_twin != Some(request.twin_id) {
        return store_setup_error(&mut runs, request, "the requested Twin is no longer active");
    }
    let Some(view_model) = view_model else {
        return store_setup_error(&mut runs, request, "SysML analysis is unavailable");
    };
    if view_model.twin_id != Some(request.twin_id)
        || view_model.source_revision != Some(request.source_revision)
    {
        return store_setup_error(
            &mut runs,
            request,
            "SysML source changed; refresh before running",
        );
    }
    let Some(twin) = workspace.twin(request.twin_id) else {
        return store_setup_error(&mut runs, request, "the requested Twin is no longer open");
    };
    if view_model
        .source_files
        .iter()
        .filter_map(|source| source.document_view(documents.as_deref()))
        .any(|document| document.dirty)
    {
        return store_setup_error(
            &mut runs,
            request,
            "save or discard open SysML edits before running verification",
        );
    }

    let mut registry_errors = twin.verification_registry_errors();
    registry_errors.extend(twin.component_registry_errors());
    if !registry_errors.is_empty() {
        return store_setup_error(&mut runs, request, &registry_errors.join("; "));
    }
    let Some(case) = twin.verification_case(&request.name) else {
        return store_setup_error(
            &mut runs,
            request,
            "the Twin has no registered test for this verification case",
        );
    };

    let root = twin.root.clone();
    let scene = root.join(&case.scene);
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            return store_setup_error(
                &mut runs,
                request,
                &format!("cannot locate the LunCoSim executable: {error}"),
            );
        }
    };
    let mut child = match Command::new(executable)
        .arg("test")
        .arg("--scene")
        .arg(&scene)
        .arg("--verification")
        .arg(&case.name)
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return store_setup_error(
                &mut runs,
                request,
                &format!("could not start the scene-test runner: {error}"),
            );
        }
    };

    let (sender, output) = mpsc::channel();
    let stdout = child.stdout.take().expect("piped stdout is available");
    let stderr = child.stderr.take().expect("piped stderr is available");
    if let Err(error) = spawn_output_reader(stdout, ProcessStream::Stdout, sender.clone()) {
        let _ = child.kill();
        let _ = child.wait();
        return store_setup_error(
            &mut runs,
            request,
            &format!("could not read scene-test output: {error}"),
        );
    }
    if let Err(error) = spawn_output_reader(stderr, ProcessStream::Stderr, sender) {
        let _ = child.kill();
        let _ = child.wait();
        return store_setup_error(
            &mut runs,
            request,
            &format!("could not read scene-test errors: {error}"),
        );
    }

    runs.active = Some(ActiveRun {
        twin_id: request.twin_id,
        source_revision: request.source_revision,
        name: request.name.clone(),
        child,
        output: Mutex::new(output),
        stdout: None,
        stderr: None,
        exit_status: None,
    });
}

fn spawn_output_reader<R: Read + Send + 'static>(
    mut reader: R,
    stream: ProcessStream,
    sender: mpsc::Sender<ProcessOutput>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("sysml-verification-output".to_owned())
        .spawn(move || {
            const MAX_CAPTURED_OUTPUT_BYTES: usize = 128 * 1024;
            let mut tail = Vec::with_capacity(MAX_CAPTURED_OUTPUT_BYTES);
            let mut chunk = [0; 8192];
            let read_result = loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break Ok(()),
                    Ok(read) => {
                        tail.extend_from_slice(&chunk[..read]);
                        if tail.len() > MAX_CAPTURED_OUTPUT_BYTES {
                            let excess = tail.len() - MAX_CAPTURED_OUTPUT_BYTES;
                            tail.drain(..excess);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => break Err(error),
                }
            };
            let content = read_result
                .map(|()| String::from_utf8_lossy(&tail).into_owned())
                .map_err(|error| error.to_string());
            let _ = sender.send(ProcessOutput { stream, content });
        })
        .map(|_| ())
}

fn store_setup_error(
    runs: &mut SysmlVerificationRuns,
    request: &RunSysmlVerification,
    message: &str,
) {
    runs.completed
        .entry(request.twin_id.raw())
        .or_default()
        .insert(
            request.name.clone(),
            VerificationRunResult {
                twin_id: request.twin_id,
                source_revision: request.source_revision,
                name: request.name.clone(),
                outcome: VerificationRunOutcome::Error(message.to_owned()),
                summary: message.to_owned(),
            },
        );
}

pub(crate) fn poll_sysml_verification_run(
    mut runs: ResMut<SysmlVerificationRuns>,
    workspace: Option<Res<WorkspaceResource>>,
) {
    let Some(active) = runs.active.as_mut() else {
        return;
    };
    let output = match active.output.get_mut() {
        Ok(output) => output,
        Err(_) => {
            let result = VerificationRunResult {
                twin_id: active.twin_id,
                source_revision: active.source_revision,
                name: active.name.clone(),
                outcome: VerificationRunOutcome::Error(
                    "scene-test output channel is unavailable".to_owned(),
                ),
                summary: "The scene-test output channel could not be read.".to_owned(),
            };
            runs.active = None;
            store_result_if_twin_open(&mut runs, workspace.as_deref(), result);
            return;
        }
    };
    loop {
        match output.try_recv() {
            Ok(ProcessOutput {
                stream: ProcessStream::Stdout,
                content,
            }) => active.stdout = Some(content),
            Ok(ProcessOutput {
                stream: ProcessStream::Stderr,
                content,
            }) => active.stderr = Some(content),
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                if active.stdout.is_none() {
                    active.stdout = Some(Err("stdout reader disconnected".to_owned()));
                }
                if active.stderr.is_none() {
                    active.stderr = Some(Err("stderr reader disconnected".to_owned()));
                }
                break;
            }
        }
    }
    if active.exit_status.is_none() {
        match active.child.try_wait() {
            Ok(status) => active.exit_status = status,
            Err(error) => {
                let result = VerificationRunResult {
                    twin_id: active.twin_id,
                    source_revision: active.source_revision,
                    name: active.name.clone(),
                    outcome: VerificationRunOutcome::Error(format!(
                        "could not read scene-test process status: {error}"
                    )),
                    summary: "The scene-test process could not be monitored.".to_owned(),
                };
                runs.active = None;
                store_result_if_twin_open(&mut runs, workspace.as_deref(), result);
                return;
            }
        }
    }
    if active.exit_status.is_none() || active.stdout.is_none() || active.stderr.is_none() {
        return;
    }

    let active = runs.active.take().expect("active run was just checked");
    let result = build_run_result(active);
    store_result_if_twin_open(&mut runs, workspace.as_deref(), result);
}

fn store_result_if_twin_open(
    runs: &mut SysmlVerificationRuns,
    workspace: Option<&WorkspaceResource>,
    result: VerificationRunResult,
) {
    if workspace.is_some_and(|workspace| workspace.twin(result.twin_id).is_some()) {
        runs.completed
            .entry(result.twin_id.raw())
            .or_default()
            .insert(result.name.clone(), result);
    }
}

fn build_run_result(mut active: ActiveRun) -> VerificationRunResult {
    let stdout = active
        .stdout
        .take()
        .unwrap_or_else(|| Err("stdout was not captured".to_owned()));
    let stderr = active
        .stderr
        .take()
        .unwrap_or_else(|| Err("stderr was not captured".to_owned()));
    let status = active
        .exit_status
        .as_ref()
        .expect("run result waits for process exit");
    let stdout = stdout.unwrap_or_else(|error| format!("stdout capture failed: {error}"));
    let stderr = stderr.unwrap_or_else(|error| format!("stderr capture failed: {error}"));
    let summary = test_summary(&stdout, &stderr, status);
    let outcome = match status.code() {
        Some(0) => VerificationRunOutcome::Passed,
        Some(1) => VerificationRunOutcome::Failed,
        Some(_) | None => VerificationRunOutcome::NoVerdict,
    };
    VerificationRunResult {
        twin_id: active.twin_id,
        source_revision: active.source_revision,
        name: active.name.clone(),
        outcome,
        summary,
    }
}

fn test_summary(stdout: &str, stderr: &str, status: &ExitStatus) -> String {
    let summary = stdout
        .lines()
        .chain(stderr.lines())
        .rev()
        .find(|line| line.trim_start().starts_with("luncosim test "));
    if let Some(summary) = summary {
        return summary.trim().to_owned();
    }
    let detail = stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .or_else(|| stdout.lines().rev().find(|line| !line.trim().is_empty()));
    match detail {
        Some(detail) => format!("Process exit {:?}: {detail}", status.code()),
        None => format!(
            "Process exited with status {:?} without a test summary.",
            status.code()
        ),
    }
}

pub(crate) fn clear_sysml_verification_results(
    trigger: On<TwinClosed>,
    mut runs: ResMut<SysmlVerificationRuns>,
) {
    let twin_id = trigger.event().twin;
    runs.completed.remove(&twin_id.raw());
    if runs
        .active
        .as_ref()
        .is_some_and(|active| active.twin_id == twin_id)
    {
        runs.active = None;
    }
}
