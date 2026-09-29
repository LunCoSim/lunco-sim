use std::time::Duration;

use bevy::prelude::*;
use lunco_workspace::{TwinClosed, TwinId};

use crate::view_model::RuntimeRequirementEvidence;

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
    Inconclusive,
    Unverified,
    Error,
    Cancelled,
    NoVerdict,
    RunError(String),
}

#[derive(Clone, Debug)]
pub(crate) struct VerificationRunResult {
    pub twin_id: TwinId,
    pub source_revision: u64,
    pub observed_source_revision: Option<u64>,
    pub source_revision_matches: bool,
    pub name: String,
    pub outcome: VerificationRunOutcome,
    pub summary: String,
    pub diagnostics: Vec<String>,
    pub output: String,
    pub elapsed: Duration,
    pub evidence: Vec<RuntimeRequirementEvidence>,
}

#[derive(Resource, Default)]
pub(crate) struct SysmlVerificationRuns;

impl SysmlVerificationRuns {
    pub(crate) fn result(&self, _twin_id: TwinId, _name: &str) -> Option<&VerificationRunResult> {
        None
    }

    pub(crate) fn is_running(&self, _twin_id: TwinId, _name: &str) -> bool {
        false
    }

    pub(crate) fn is_queued(&self, _twin_id: TwinId, _name: &str) -> bool {
        false
    }

    pub(crate) fn has_active_run(&self) -> bool {
        false
    }

    pub(crate) fn suite_progress(&self, _twin_id: TwinId) -> Option<(usize, usize, usize, bool)> {
        None
    }

    pub(crate) fn active_case(&self) -> Option<(TwinId, &str)> {
        None
    }

    pub(crate) fn active_output(&self, _twin_id: TwinId, _name: &str) -> Option<(&str, Duration)> {
        None
    }
}

pub(crate) fn start_sysml_verification(
    _trigger: On<RunSysmlVerification>,
    _runs: ResMut<SysmlVerificationRuns>,
) {
}

pub(crate) fn start_sysml_verification_suite(
    _trigger: On<RunSysmlVerificationSuite>,
    _runs: ResMut<SysmlVerificationRuns>,
) {
}

pub(crate) fn poll_sysml_verification_run(_runs: ResMut<SysmlVerificationRuns>) {}

pub(crate) fn cancel_sysml_verification(
    _trigger: On<CancelSysmlVerification>,
    _runs: ResMut<SysmlVerificationRuns>,
) {
}

pub(crate) fn cancel_sysml_verification_suite(
    _trigger: On<CancelSysmlVerificationSuite>,
    _runs: ResMut<SysmlVerificationRuns>,
) {
}

pub(crate) fn clear_sysml_verification_results(
    _trigger: On<TwinClosed>,
    _runs: ResMut<SysmlVerificationRuns>,
) {
}
