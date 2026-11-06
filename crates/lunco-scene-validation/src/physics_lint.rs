//! Change-driven diagnostics over the native solver owner; policy remains in Rhai.

use bevy::prelude::*;
use lunco_core::{AppSceneExt, SceneResource};
use lunco_hooks::HookValue;

const DOMAIN: &str = "physics";
const EVENT: &str = "physics.solver-profile";

#[derive(Resource)]
struct PhysicsLintState {
    dirty: bool,
    policy_generation: u64,
    findings: Vec<lunco_doc::Diagnostic>,
}

impl Default for PhysicsLintState {
    fn default() -> Self {
        Self {
            dirty: true,
            policy_generation: 0,
            findings: Vec::new(),
        }
    }
}

impl SceneResource for PhysicsLintState {
    fn clear_scene(&mut self) {
        *self = Self::default();
    }
}

fn changed(
    _event: On<lunco_physics::PhysicsSolverConfigurationChanged>,
    mut state: ResMut<PhysicsLintState>,
) {
    state.dirty = true;
}

fn pending(state: Res<PhysicsLintState>) -> bool {
    state.dirty || state.policy_generation != lunco_hooks::generation()
}

/// Shared consumer for native change events, policy replacement and explicit RunLint.
fn run(world: &mut World) {
    run_with_context(
        world,
        lunco_core::RuntimeExecutionContext {
            route: Some(lunco_core::RuntimeRoute::application(
                lunco_core::RuntimeCycle::Lifecycle,
            )),
            phase: lunco_core::RuntimePhase::Event,
            ..lunco_core::RuntimeExecutionContext::unclassified()
        },
    );
}

pub(crate) fn run_with_context(world: &mut World, context: lunco_core::RuntimeExecutionContext) {
    let generation = lunco_hooks::generation();
    let Some(substeps) = lunco_physics::solver_substeps(world) else {
        let mut report = world.resource_mut::<lunco_lint::LintReport>();
        report.clear_domain(DOMAIN);
        let revision = report.begin_scope(DOMAIN);
        report.fail_scope(
            DOMAIN,
            revision,
            "Native physics solver is not installed in this host",
        );
        let mut state = world.resource_mut::<PhysicsLintState>();
        state.dirty = false;
        state.policy_generation = generation;
        state.findings.clear();
        return;
    };
    let findings = {
        lunco_lint::run_lint_with_context(
            DOMAIN,
            HookValue::map([
                ("substeps", HookValue::Int(i64::from(substeps))),
                (
                    "standard_substeps",
                    HookValue::Int(i64::from(lunco_physics::DEFAULT_SUBSTEP_COUNT)),
                ),
            ]),
            context,
        )
    };
    let changed = world.resource::<PhysicsLintState>().findings != findings;
    let had_findings = !world.resource::<PhysicsLintState>().findings.is_empty();
    if changed {
        let mut commands = world.commands();
        for finding in &findings {
            let severity = match finding.severity {
                lunco_doc::DiagnosticSeverity::Error => lunco_telemetry_core::Severity::Error,
                lunco_doc::DiagnosticSeverity::Warning => lunco_telemetry_core::Severity::Warning,
                _ => lunco_telemetry_core::Severity::Info,
            };
            lunco_telemetry_core::emit_status_telemetry_event(
                &mut commands,
                EVENT,
                severity,
                finding.message.clone(),
            );
        }
        if had_findings && findings.is_empty() {
            lunco_telemetry_core::emit_status_telemetry_event(
                &mut commands,
                EVENT,
                lunco_telemetry_core::Severity::Info,
                "Physics solver lint cleared",
            );
        }
    }
    let mut report = world.resource_mut::<lunco_lint::LintReport>();
    let revision = report.begin_scope(DOMAIN);
    report.clear_domain(DOMAIN);
    if changed {
        report.extend_logged(findings.clone());
    } else {
        report.findings.extend(findings.clone());
    }
    report.complete_scope(DOMAIN, revision);
    let mut state = world.resource_mut::<PhysicsLintState>();
    state.dirty = false;
    state.policy_generation = generation;
    state.findings = findings;
}

pub(crate) fn install(app: &mut App) {
    app.init_scene_resource::<PhysicsLintState>()
        .add_observer(changed)
        .add_systems(
            Update,
            run.after(lunco_physics::PhysicsConfigurationSet::Publish)
                .run_if(pending),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    // A host without native physics cannot expose this seam through a running
    // scene. Check the generic resource boundary without authored asset fixtures.
    #[test]
    fn missing_solver_cannot_report_a_clean_physics_scope() {
        let mut world = World::new();
        world.init_resource::<PhysicsLintState>();
        world.init_resource::<lunco_lint::LintReport>();
        run(&mut world);
        let report = world.resource::<lunco_lint::LintReport>();
        let scope = &report.scopes[DOMAIN];
        assert!(
            matches!(&scope.state, lunco_lint::LintScopeState::Failed(reason)
            if reason.contains("not installed"))
        );
        assert!(!world.resource::<PhysicsLintState>().dirty);
    }
}
