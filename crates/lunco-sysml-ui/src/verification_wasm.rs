use bevy::prelude::*;
use lunco_workspace::{TwinClosed, TwinId};

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
pub(crate) struct SysmlVerificationRuns;

impl SysmlVerificationRuns {
    pub(crate) fn result(&self, _twin_id: TwinId, _name: &str) -> Option<&VerificationRunResult> {
        None
    }

    pub(crate) fn is_running(&self, _twin_id: TwinId, _name: &str) -> bool {
        false
    }

    pub(crate) fn has_active_run(&self) -> bool {
        false
    }

    pub(crate) fn active_case(&self) -> Option<(TwinId, &str)> {
        None
    }
}

pub(crate) fn start_sysml_verification(
    _trigger: On<RunSysmlVerification>,
    _runs: ResMut<SysmlVerificationRuns>,
) {
}

pub(crate) fn poll_sysml_verification_run(_runs: ResMut<SysmlVerificationRuns>) {}

pub(crate) fn clear_sysml_verification_results(
    _trigger: On<TwinClosed>,
    _runs: ResMut<SysmlVerificationRuns>,
) {
}
