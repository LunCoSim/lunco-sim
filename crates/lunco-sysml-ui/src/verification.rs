use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use lunco_doc_bevy::DocumentRegistry;
use lunco_scene_runner::{SceneTestVerdict, parse_scene_test_report};
use lunco_sysml::SysmlDocument;
use lunco_workspace::{TwinClosed, TwinId, WorkspaceResource};

use crate::view_model::{RuntimeRequirementEvidence, SysmlRequirementsViewModel};

#[derive(Event, Clone, Debug)]
pub(crate) struct RunSysmlVerification {
    pub twin_id: TwinId,
    pub source_revision: u64,
    pub name: String,
}

#[derive(Event, Clone, Debug)]
pub(crate) struct RunSysmlVerificationSuite {
    pub twin_id: TwinId,
    pub source_revision: u64,
    pub names: Vec<String>,
}

#[derive(Event, Clone, Debug)]
pub(crate) struct CancelSysmlVerification {
    pub twin_id: TwinId,
    pub name: String,
}

#[derive(Event, Clone, Debug)]
pub(crate) struct CancelSysmlVerificationSuite {
    pub twin_id: TwinId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum VerificationRunOutcome {
    Passed,
    Failed,
    Cancelled,
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
    pub diagnostics: Vec<String>,
    pub output: String,
    pub elapsed: Duration,
    pub evidence: Vec<RuntimeRequirementEvidence>,
}

#[derive(Resource, Default)]
pub(crate) struct SysmlVerificationRuns {
    active: Option<ActiveRun>,
    suite: Option<VerificationSuiteRun>,
    completed: HashMap<u64, HashMap<String, VerificationRunResult>>,
    last_suite: HashMap<u64, VerificationSuiteReport>,
}

struct VerificationSuiteRun {
    twin_id: TwinId,
    source_revision: u64,
    names: Vec<String>,
    pending: VecDeque<String>,
    total: usize,
    started: usize,
    stopping: bool,
}

#[derive(Clone)]
pub(crate) struct VerificationSuiteReport {
    pub source_revision: u64,
    pub stopped: bool,
    pub cases: Vec<VerificationSuiteCase>,
}

#[derive(Clone)]
pub(crate) struct VerificationSuiteCase {
    pub name: String,
    pub outcome: VerificationRunOutcome,
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
    stdout_complete: bool,
    stderr_complete: bool,
    output_log: String,
    last_log_stream: Option<ProcessStream>,
    started_at: Instant,
    cancel_requested: bool,
    cancelled: bool,
}

impl Drop for ActiveRun {
    fn drop(&mut self) {
        if self.exit_status.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProcessStream {
    Stdout,
    Stderr,
}

struct ProcessOutput {
    stream: ProcessStream,
    chunk: Option<String>,
    complete: Option<Result<String, String>>,
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

    pub(crate) fn is_queued(&self, twin_id: TwinId, name: &str) -> bool {
        self.suite.as_ref().is_some_and(|suite| {
            suite.twin_id == twin_id && suite.pending.iter().any(|queued| queued == name)
        })
    }

    pub(crate) fn has_active_run(&self) -> bool {
        self.active.is_some() || self.suite.is_some()
    }

    pub(crate) fn suite_progress(&self, twin_id: TwinId) -> Option<(usize, usize, usize, bool)> {
        let suite = self
            .suite
            .as_ref()
            .filter(|suite| suite.twin_id == twin_id)?;
        Some((
            suite.started,
            suite.total,
            suite.pending.len(),
            suite.stopping,
        ))
    }

    pub(crate) fn active_case(&self) -> Option<(TwinId, &str)> {
        self.active
            .as_ref()
            .map(|active| (active.twin_id, active.name.as_str()))
    }

    pub(crate) fn active_output(&self, twin_id: TwinId, name: &str) -> Option<(&str, Duration)> {
        let active = self.active.as_ref()?;
        (active.twin_id == twin_id && active.name == name)
            .then(|| (active.output_log.as_str(), active.started_at.elapsed()))
    }

    pub(crate) fn last_suite(&self, twin_id: TwinId) -> Option<&VerificationSuiteReport> {
        self.last_suite.get(&twin_id.raw())
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
    if runs.active.is_some() || runs.suite.is_some() {
        return;
    }

    start_verification(
        request,
        workspace.as_deref(),
        view_model.as_deref(),
        documents.as_deref(),
        &mut runs,
    );
}

pub(crate) fn start_sysml_verification_suite(
    trigger: On<RunSysmlVerificationSuite>,
    mut runs: ResMut<SysmlVerificationRuns>,
) {
    let request = trigger.event();
    if runs.active.is_some() || runs.suite.is_some() {
        return;
    }
    let mut names = request.names.clone();
    names.retain(|name| !name.trim().is_empty());
    names.sort();
    names.dedup();
    if names.is_empty() {
        return;
    }
    let total = names.len();
    runs.suite = Some(VerificationSuiteRun {
        twin_id: request.twin_id,
        source_revision: request.source_revision,
        names: names.clone(),
        pending: names.into(),
        total,
        started: 0,
        stopping: false,
    });
}

fn start_verification(
    request: &RunSysmlVerification,
    workspace: Option<&WorkspaceResource>,
    view_model: Option<&SysmlRequirementsViewModel>,
    documents: Option<&DocumentRegistry<SysmlDocument>>,
    runs: &mut SysmlVerificationRuns,
) {
    if runs.active.is_some() {
        return;
    }

    let Some(workspace) = workspace else {
        return store_setup_error(runs, request, "workspace is unavailable");
    };
    if workspace.active_twin != Some(request.twin_id) {
        return store_setup_error(runs, request, "the requested Twin is no longer active");
    }
    let Some(view_model) = view_model else {
        return store_setup_error(runs, request, "SysML analysis is unavailable");
    };
    if view_model.twin_id != Some(request.twin_id)
        || view_model.source_revision != Some(request.source_revision)
    {
        return store_setup_error(
            runs,
            request,
            "SysML source changed; refresh before running",
        );
    }
    let Some(twin) = workspace.twin(request.twin_id) else {
        return store_setup_error(runs, request, "the requested Twin is no longer open");
    };
    if view_model
        .source_files
        .iter()
        .filter_map(|source| source.document_view(documents.as_deref()))
        .any(|document| document.dirty)
    {
        return store_setup_error(
            runs,
            request,
            "save or discard open SysML edits before running verification",
        );
    }

    let mut registry_errors = twin.verification_registry_errors();
    registry_errors.extend(twin.component_registry_errors());
    if !registry_errors.is_empty() {
        return store_setup_error(runs, request, &registry_errors.join("; "));
    }
    let Some(case) = twin.verification_case(&request.name) else {
        return store_setup_error(
            runs,
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
                runs,
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
                runs,
                request,
                &format!("could not start the scene-test runner: {error}"),
            );
        }
    };

    let (sender, output) = mpsc::sync_channel(128);
    let stdout = child.stdout.take().expect("piped stdout is available");
    let stderr = child.stderr.take().expect("piped stderr is available");
    if let Err(error) = spawn_output_reader(stdout, ProcessStream::Stdout, sender.clone()) {
        let _ = child.kill();
        let _ = child.wait();
        return store_setup_error(
            runs,
            request,
            &format!("could not read scene-test output: {error}"),
        );
    }
    if let Err(error) = spawn_output_reader(stderr, ProcessStream::Stderr, sender) {
        let _ = child.kill();
        let _ = child.wait();
        return store_setup_error(
            runs,
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
        stdout_complete: false,
        stderr_complete: false,
        output_log: String::new(),
        last_log_stream: None,
        started_at: Instant::now(),
        cancel_requested: false,
        cancelled: false,
    });
}

fn spawn_output_reader<R: Read + Send + 'static>(
    mut reader: R,
    stream: ProcessStream,
    sender: mpsc::SyncSender<ProcessOutput>,
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
                        let _ = sender.send(ProcessOutput {
                            stream,
                            chunk: Some(String::from_utf8_lossy(&chunk[..read]).into_owned()),
                            complete: None,
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => break Err(error),
                }
            };
            let content = read_result
                .map(|()| String::from_utf8_lossy(&tail).into_owned())
                .map_err(|error| error.to_string());
            let _ = sender.send(ProcessOutput {
                stream,
                chunk: None,
                complete: Some(content),
            });
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
                diagnostics: vec![message.to_owned()],
                output: String::new(),
                elapsed: Duration::ZERO,
                evidence: Vec::new(),
            },
        );
}

pub(crate) fn cancel_sysml_verification(
    trigger: On<CancelSysmlVerification>,
    mut runs: ResMut<SysmlVerificationRuns>,
) {
    let request = trigger.event();
    if let Some(active) = runs.active.as_mut()
        && active.twin_id == request.twin_id
        && active.name == request.name
    {
        active.cancel_requested = true;
    }
}

pub(crate) fn cancel_sysml_verification_suite(
    trigger: On<CancelSysmlVerificationSuite>,
    mut runs: ResMut<SysmlVerificationRuns>,
) {
    let twin_id = trigger.event().twin_id;
    let (source_revision, pending) = {
        let Some(suite) = runs.suite.as_mut().filter(|suite| suite.twin_id == twin_id) else {
            return;
        };
        suite.stopping = true;
        (
            suite.source_revision,
            suite.pending.drain(..).collect::<Vec<_>>(),
        )
    };
    for name in pending {
        store_cancelled_queued_case(&mut runs, twin_id, source_revision, name);
    }
    if let Some(active) = runs
        .active
        .as_mut()
        .filter(|active| active.twin_id == twin_id)
    {
        active.cancel_requested = true;
    } else {
        finish_suite(&mut runs, twin_id);
    }
}

fn store_cancelled_queued_case(
    runs: &mut SysmlVerificationRuns,
    twin_id: TwinId,
    source_revision: u64,
    name: String,
) {
    let message = "Not run because the verification suite was stopped.".to_owned();
    runs.completed.entry(twin_id.raw()).or_default().insert(
        name.clone(),
        VerificationRunResult {
            twin_id,
            source_revision,
            name,
            outcome: VerificationRunOutcome::Cancelled,
            summary: message.clone(),
            diagnostics: vec![message],
            output: String::new(),
            elapsed: Duration::ZERO,
            evidence: Vec::new(),
        },
    );
}

pub(crate) fn poll_sysml_verification_run(
    mut runs: ResMut<SysmlVerificationRuns>,
    workspace: Option<Res<WorkspaceResource>>,
    view_model: Option<Res<SysmlRequirementsViewModel>>,
    documents: Option<Res<DocumentRegistry<SysmlDocument>>>,
) {
    let workspace = workspace.as_deref();
    if poll_active_verification(&mut runs, workspace) {
        return;
    }
    start_next_suite_case(
        &mut runs,
        workspace,
        view_model.as_deref(),
        documents.as_deref(),
    );
}

/// Returns true while a child process remains active.
fn poll_active_verification(
    runs: &mut SysmlVerificationRuns,
    workspace: Option<&WorkspaceResource>,
) -> bool {
    let Some(active) = runs.active.as_mut() else {
        return false;
    };
    if active.cancel_requested {
        active.cancel_requested = false;
        if active.exit_status.is_none() {
            active.cancelled = true;
            let _ = active.child.kill();
        }
    }
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
                diagnostics: vec!["The scene-test output channel could not be read.".to_owned()],
                output: active.output_log.clone(),
                elapsed: active.started_at.elapsed(),
                evidence: Vec::new(),
            };
            runs.active = None;
            store_result_if_twin_open(runs, workspace, result);
            return false;
        }
    };
    loop {
        match output.try_recv() {
            Ok(message) => {
                if let Some(chunk) = message.chunk {
                    append_output_log(
                        &mut active.output_log,
                        &mut active.last_log_stream,
                        message.stream,
                        &chunk,
                    );
                }
                if let Some(content) = message.complete {
                    match message.stream {
                        ProcessStream::Stdout => {
                            active.stdout = Some(content);
                            active.stdout_complete = true;
                        }
                        ProcessStream::Stderr => {
                            active.stderr = Some(content);
                            active.stderr_complete = true;
                        }
                    }
                }
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                if active.stdout.is_none() {
                    active.stdout = Some(Err("stdout reader disconnected".to_owned()));
                    active.stdout_complete = true;
                }
                if active.stderr.is_none() {
                    active.stderr = Some(Err("stderr reader disconnected".to_owned()));
                    active.stderr_complete = true;
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
                    diagnostics: vec![format!("Could not read scene-test process status: {error}")],
                    output: active.output_log.clone(),
                    elapsed: active.started_at.elapsed(),
                    evidence: Vec::new(),
                };
                runs.active = None;
                store_result_if_twin_open(runs, workspace, result);
                return false;
            }
        }
    }
    if active.exit_status.is_none() || !active.stdout_complete || !active.stderr_complete {
        return true;
    }

    let active = runs.active.take().expect("active run was just checked");
    let result = build_run_result(active);
    store_result_if_twin_open(runs, workspace, result);
    false
}

fn start_next_suite_case(
    runs: &mut SysmlVerificationRuns,
    workspace: Option<&WorkspaceResource>,
    view_model: Option<&SysmlRequirementsViewModel>,
    documents: Option<&DocumentRegistry<SysmlDocument>>,
) {
    if runs.active.is_some() {
        return;
    }
    if runs
        .suite
        .as_ref()
        .is_some_and(|suite| suite.pending.is_empty())
    {
        let twin_id = runs.suite.as_ref().expect("suite was just checked").twin_id;
        finish_suite(runs, twin_id);
        return;
    }
    let request = {
        let Some(suite) = runs.suite.as_mut() else {
            return;
        };
        let name = suite
            .pending
            .pop_front()
            .expect("non-empty suite was checked");
        suite.started += 1;
        RunSysmlVerification {
            twin_id: suite.twin_id,
            source_revision: suite.source_revision,
            name,
        }
    };
    start_verification(&request, workspace, view_model, documents, runs);
}

fn finish_suite(runs: &mut SysmlVerificationRuns, twin_id: TwinId) {
    let Some(suite) = runs.suite.take().filter(|suite| suite.twin_id == twin_id) else {
        return;
    };
    let completed = runs.completed.get(&twin_id.raw());
    let cases = suite
        .names
        .into_iter()
        .map(|name| VerificationSuiteCase {
            outcome: completed
                .and_then(|cases| cases.get(&name))
                .map_or(VerificationRunOutcome::NoVerdict, |result| {
                    result.outcome.clone()
                }),
            name,
        })
        .collect();
    runs.last_suite.insert(
        twin_id.raw(),
        VerificationSuiteReport {
            source_revision: suite.source_revision,
            stopped: suite.stopping,
            cases,
        },
    );
}

fn append_output_log(
    output: &mut String,
    last_stream: &mut Option<ProcessStream>,
    stream: ProcessStream,
    chunk: &str,
) {
    const MAX_VISIBLE_OUTPUT_BYTES: usize = 64 * 1024;
    if *last_stream != Some(stream) {
        output.push_str(match stream {
            ProcessStream::Stdout => "\n[stdout] ",
            ProcessStream::Stderr => "\n[stderr] ",
        });
        *last_stream = Some(stream);
    }
    output.push_str(chunk);
    if output.len() > MAX_VISIBLE_OUTPUT_BYTES {
        let excess = output.len() - MAX_VISIBLE_OUTPUT_BYTES;
        let boundary = output
            .char_indices()
            .find_map(|(index, _)| (index >= excess).then_some(index))
            .unwrap_or(output.len());
        output.drain(..boundary);
    }
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
    let elapsed = active.started_at.elapsed();
    let output_log = active.output_log.clone();
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
    let stderr_error = stderr.as_ref().err().cloned();
    drop(stderr);
    let (outcome, summary, diagnostics, report) = if active.cancelled {
        (
            VerificationRunOutcome::Cancelled,
            format!("Cancelled after {:.1} s", elapsed.as_secs_f32()),
            Vec::new(),
            None,
        )
    } else {
        let parsed_report = match stdout {
            Ok(stdout) => parse_scene_test_report(&stdout),
            Err(error) => Err(format!("scene-test stdout capture failed: {error}")),
        };
        match parsed_report {
            Err(error) => {
                let summary = format!("The scene-test report could not be read: {error}");
                (
                    VerificationRunOutcome::Error(summary.clone()),
                    summary.clone(),
                    vec![summary],
                    None,
                )
            }
            Ok(Some(report))
                if report.schema_version != 1
                    || report.verification != active.name
                    || status.code() != Some(i32::from(report.process_exit_code)) =>
            {
                let message = format!(
                    "The scene-test report did not match this run (schema {}, verification {}).",
                    report.schema_version, report.verification
                );
                (
                    VerificationRunOutcome::Error(message.clone()),
                    message.clone(),
                    vec![message],
                    Some(report),
                )
            }
            Ok(Some(report)) => {
                let outcome = match report.process_exit_code {
                    0 => VerificationRunOutcome::Passed,
                    1 => VerificationRunOutcome::Failed,
                    2 => VerificationRunOutcome::NoVerdict,
                    code => VerificationRunOutcome::Error(format!(
                        "The scene-test runner returned unsupported status {code}."
                    )),
                };
                let channel = report
                    .verdict_channel
                    .as_deref()
                    .map(|channel| format!(" · {channel}"))
                    .unwrap_or_default();
                let summary = match (report.process_exit_code, report.verdict) {
                    (0, Some(SceneTestVerdict::Passed)) => format!("PASS{channel}"),
                    (1, Some(SceneTestVerdict::Failed)) => format!("FAIL{channel}"),
                    (1, Some(SceneTestVerdict::Passed)) => {
                        format!("RUNNER FAIL · scenario reported PASS{channel}")
                    }
                    (1, None) => "RUNNER FAIL · no scenario verdict".to_owned(),
                    (0, None) => "PASS".to_owned(),
                    (0, Some(SceneTestVerdict::Failed)) => {
                        format!(
                            "RUNNER ERROR · exit status PASS but scenario reported FAIL{channel}"
                        )
                    }
                    (2, Some(verdict)) => format!(
                        "NO VERDICT · scenario reported {}{channel}",
                        match verdict {
                            SceneTestVerdict::Passed => "PASS",
                            SceneTestVerdict::Failed => "FAIL",
                        }
                    ),
                    (2, None) => "NO VERDICT".to_owned(),
                    (code, _) => format!("RUNNER ERROR · unsupported status {code}"),
                };
                let mut diagnostics = Vec::new();
                if let Some(diagnostic) = &report.runner_diagnostic {
                    diagnostics.push(diagnostic.clone());
                }
                if report.details_truncated {
                    diagnostics.push(
                        "Some structured evidence or check details exceeded the report size limit."
                            .to_owned(),
                    );
                }
                (outcome, summary, diagnostics, Some(report))
            }
            Ok(None) => {
                let summary = format!(
                    "Scene-test process exited with status {:?} without a structured verification report.",
                    status.code()
                );
                (
                    VerificationRunOutcome::Error(summary.clone()),
                    summary.clone(),
                    vec![summary],
                    None,
                )
            }
        }
    };
    let mut diagnostics = diagnostics;
    if let Some(error) = stderr_error {
        diagnostics.push(format!("Scene-test stderr capture failed: {error}"));
    }
    let evidence = report
        .as_ref()
        .map(crate::view_model::scene_test_requirement_evidence)
        .unwrap_or_default();
    let structured_diagnostics = report
        .as_ref()
        .map(crate::view_model::scene_test_report_diagnostics)
        .unwrap_or_default();
    for diagnostic in &structured_diagnostics {
        if !diagnostics.contains(diagnostic) {
            diagnostics.push(diagnostic.clone());
        }
    }
    if matches!(outcome, VerificationRunOutcome::Failed)
        && evidence.iter().all(|evidence| evidence.failures == 0)
        && structured_diagnostics.is_empty()
        && diagnostics.is_empty()
    {
        diagnostics.push(
            "The scene test failed without a structured requirement check failure.".to_owned(),
        );
    }
    VerificationRunResult {
        twin_id: active.twin_id,
        source_revision: active.source_revision,
        name: active.name.clone(),
        outcome,
        summary,
        diagnostics,
        output: output_log,
        elapsed,
        evidence,
    }
}

pub(crate) fn clear_sysml_verification_results(
    trigger: On<TwinClosed>,
    mut runs: ResMut<SysmlVerificationRuns>,
) {
    let twin_id = trigger.event().twin;
    runs.completed.remove(&twin_id.raw());
    runs.last_suite.remove(&twin_id.raw());
    if runs
        .suite
        .as_ref()
        .is_some_and(|suite| suite.twin_id == twin_id)
    {
        runs.suite = None;
    }
    if runs
        .active
        .as_ref()
        .is_some_and(|active| active.twin_id == twin_id)
    {
        runs.active = None;
    }
}
