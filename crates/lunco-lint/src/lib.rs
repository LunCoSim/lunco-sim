//! Universal lint substrate — FACTS in Rust, RULES in authored policy.
//!
//! # Why a linter at all
//!
//! Some authoring mistakes have no symptom. The one that motivated this crate:
//! every rover mounted four motors whose component asset applied
//! `PhysicsRigidBodyAPI` and which no joint attached, so each motor became a free
//! body and fell out of the vehicle on the first physics step. The rovers still
//! drove, still steered, still made top speed. Nothing logged, nothing failed —
//! the only evidence was hardware lying on the regolith in a screenshot.
//!
//! A linter is the answer to that class: a check over what was AUTHORED, run at
//! load, that says the thing the simulation itself will never say.
//!
//! # The split, and why it is this way round
//!
//! * **Rust supplies FACTS.** Only the domain crate can read its own subject: the
//!   composed USD stage, the parsed rhai document, the Modelica model's declared
//!   ports. Extracting those is code, and it is tested as code.
//! * **Policy supplies RULES.** Every rule is a line in an authored
//!   `assets/scripting/policy/lint_<domain>.rhai`, reached through the hook
//!   registry. Adding a rule, tightening a threshold or silencing a false
//!   positive is an edit to a script and a re-register — no rebuild, and it can
//!   be done against a RUNNING sim, which is the point: a rule you cannot try
//!   immediately is a rule nobody writes.
//!
//! With no policy registered a domain simply produces no findings, so an app that
//! ships without scripting behaves exactly as before.
//!
//! # One axis: the domain
//!
//! Linters are **separate per domain** — `usd`, `rhai`, `modelica`, `sysml`, and whatever
//! comes next — because their subjects, their vocabulary and the people who tune
//! them are different, and one giant rule file would be read by no one. The
//! substrate is universal; the rules are not shared. A domain is just a name:
//!
//! ```text
//!   domain "usd"      → hook `lint.usd`      → assets/scripting/policy/lint_usd.rhai
//!   domain "rhai"     → hook `lint.rhai`     → assets/scripting/policy/lint_rhai.rhai
//!   domain "modelica" → hook `lint.modelica` → assets/scripting/policy/lint_modelica.rhai
//!   domain "sysml"    → hook `lint.sysml`    → assets/scripting/policy/lint_sysml.rhai
//! ```
//!
//! # The contract with a policy
//!
//! `lint_<domain>(facts) -> [ #{ rule, severity, subject, message }, … ]`
//!
//! `facts` is whatever the domain gathered ([`HookValue`] maps/arrays — typed, not
//! JSON). `severity` is `"error"`, `"warn"` or `"info"`; anything else is read as
//! `"warn"` rather than dropped, because a typo in a rule must not silently delete
//! the finding it was written to raise. A policy that returns a non-array or
//! faults yields an explicit error finding without stopping a scene from loading.

use bevy::prelude::*;
use lunco_doc::{Diagnostic, DiagnosticSeverity};
use lunco_hooks::HookValue as H;

/// The hook id a domain's rules are registered under: `lint.<domain>`.
///
/// A convention rather than a constant per domain, so a new domain needs no
/// change here — the crate that owns the subject picks the name.
pub fn hook_id(domain: &str) -> String {
    format!("lint.{domain}")
}

lunco_hooks::declare_hook! {
    id: "lint.usd",
    owner: "lunco-lint",
    description: "Evaluate authored USD facts with the active USD lint rules.",
    signature: [facts: Map],
    output: ArrayOfMap,
    deterministic: true,
    required: false,
    installable: true,
}

lunco_hooks::declare_hook! {
    id: "lint.rhai",
    owner: "lunco-lint",
    description: "Evaluate authored Rhai facts with the active Rhai lint rules.",
    signature: [facts: Map],
    output: ArrayOfMap,
    deterministic: true,
    required: false,
    installable: true,
}

lunco_hooks::declare_hook! {
    id: "lint.modelica",
    owner: "lunco-lint",
    description: "Evaluate authored Modelica facts with the active Modelica lint rules.",
    signature: [facts: Map],
    output: ArrayOfMap,
    deterministic: true,
    required: false,
    installable: true,
}

lunco_hooks::declare_hook! {
    id: "lint.sysml",
    owner: "lunco-lint",
    description: "Evaluate authored SysML facts with the active SysML lint rules.",
    signature: [facts: Map],
    output: ArrayOfMap,
    deterministic: true,
    required: false,
    installable: true,
}

lunco_hooks::declare_hook! {
    id: "lint.twin",
    owner: "lunco-lint",
    description: "Evaluate authored Twin facts with the active Twin lint rules.",
    signature: [facts: Map],
    output: ArrayOfMap,
    deterministic: true,
    required: false,
    installable: true,
}

/// Every finding since the last scene load, from every domain.
///
/// A resource rather than an event stream because the interesting question is
/// "what is wrong with what is loaded right now" — a UI panel, a toast and a test
/// all want the current set, not the history.
#[derive(Resource, Default, Debug)]
pub struct LintReport {
    /// All findings, in the order they were produced.
    pub findings: Vec<Diagnostic>,
    /// Findings not yet shown to the user. A UI bridge drains this to raise one
    /// toast per batch instead of one per finding.
    pub unreported: usize,
    /// Independent lifecycle and revision for each explicit lint scope.
    pub scopes: std::collections::BTreeMap<String, LintScopeReport>,
}

/// Lifecycle and revision of one explicit lint scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintScopeReport {
    /// Monotonic run revision for this scope.
    pub revision: u64,
    /// Current result lifecycle.
    pub state: LintScopeState,
}

/// Completion state for one explicit lint scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LintScopeState {
    Pending,
    Ready,
    Failed(String),
}

impl LintScopeState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Failed(_) => "failed",
        }
    }

    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Failed(message) => Some(message),
            Self::Pending | Self::Ready => None,
        }
    }
}

impl LintReport {
    /// Start a new lint pass for one scope and return its revision.
    pub fn begin_scope(&mut self, scope: &str) -> u64 {
        let revision = self
            .scopes
            .get(scope)
            .map_or(1, |report| report.revision.saturating_add(1));
        self.scopes.insert(
            scope.to_owned(),
            LintScopeReport {
                revision,
                state: LintScopeState::Pending,
            },
        );
        revision
    }

    /// Mark the matching scope pass as complete. A superseded completion is ignored.
    pub fn complete_scope(&mut self, scope: &str, revision: u64) {
        if let Some(report) = self.scopes.get_mut(scope)
            && report.revision == revision
            && report.state == LintScopeState::Pending
        {
            report.state = LintScopeState::Ready;
        }
    }

    /// Mark the matching scope pass as failed with an actionable reason.
    pub fn fail_scope(&mut self, scope: &str, revision: u64, message: impl Into<String>) {
        if let Some(report) = self.scopes.get_mut(scope)
            && report.revision == revision
            && report.state == LintScopeState::Pending
        {
            report.state = LintScopeState::Failed(message.into());
        }
    }

    /// Count of error-severity findings.
    pub fn errors(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == DiagnosticSeverity::Error)
            .count()
    }
    /// Count of warning-severity findings.
    pub fn warnings(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == DiagnosticSeverity::Warning)
            .count()
    }
    /// Drop everything a domain previously reported — what a domain calls before
    /// re-linting the same subject, so a fixed problem disappears instead of
    /// accumulating a duplicate.
    pub fn clear_domain(&mut self, domain: &str) {
        self.findings
            .retain(|finding| finding.domain.as_deref() != Some(domain));
    }
    /// Log a batch and file it. Errors log at `error!`, warnings at `warn!`,
    /// info at `info!` — the console is the first place anyone looks.
    pub fn extend_logged(&mut self, findings: Vec<Diagnostic>) {
        for f in &findings {
            match f.severity {
                DiagnosticSeverity::Error => error!("[lint] {}", f.summary()),
                DiagnosticSeverity::Warning => warn!("[lint] {}", f.summary()),
                DiagnosticSeverity::Info | DiagnosticSeverity::Hint => {
                    info!("[lint] {}", f.summary())
                }
            }
        }
        self.unreported += findings.len();
        self.findings.extend(findings);
    }
}

/// Ask a domain's authored rules what is wrong with `facts`.
///
/// Returns an empty vec when no policy is registered for the domain — the
/// no-scripting case. Policy faults and invalid result shapes become explicit
/// error findings so validation cannot report a clean result when linting did
/// not run. Findings remain diagnostic and do not prevent scene loading.
pub fn run_lint(domain: &str, facts: H) -> Vec<Diagnostic> {
    run_lint_with_context(
        domain,
        facts,
        lunco_hooks::RuntimeExecutionContext::unclassified(),
    )
}

/// Evaluate policy with the runtime context captured by its admission owner.
/// Asynchronous preparation never changes the caller's scope, clock, or sequence.
pub fn run_lint_with_context(
    domain: &str,
    facts: H,
    context: lunco_hooks::RuntimeExecutionContext,
) -> Vec<Diagnostic> {
    let hook = hook_id(domain);
    let Some(outcome) = lunco_hooks::invoke_with_context(&hook, &[facts], context) else {
        // No rules authored for this domain. Not a problem, and not worth a log
        // line on every scene load.
        return Vec::new();
    };
    let result = match outcome {
        Ok(v) => v,
        Err(e) => {
            let message = format!("authored lint policy failed: {e}");
            error!("[lint] policy '{hook}' faulted: {e:?}");
            return vec![policy_failure(
                domain,
                "policy-execution-failed",
                &hook,
                message,
            )];
        }
    };
    let H::Array(items) = result else {
        let message =
            format!("authored lint policy returned {result:?}; expected an array of finding maps");
        error!("[lint] policy '{hook}' returned a non-array result: {result:?}");
        return vec![policy_failure(
            domain,
            "policy-invalid-result",
            &hook,
            message,
        )];
    };
    let mut out = Vec::new();
    for item in items {
        let H::Map(entries) = &item else {
            warn!("[lint] policy '{hook}' produced a non-map finding {item:?} — skipped");
            continue;
        };
        let get = |k: &str| -> Option<&H> { entries.iter().find(|(n, _)| n == k).map(|(_, v)| v) };
        let text = |k: &str| -> String {
            match get(k) {
                Some(H::Str(s)) => s.clone(),
                Some(other) => format!("{other:?}"),
                None => String::new(),
            }
        };
        let rule = text("rule");
        let message = text("message");
        if rule.is_empty() || message.is_empty() {
            // A finding nobody can act on. Naming the offending item is the
            // whole product of a linter, so an unnamed one is a policy bug and
            // says so rather than appearing as a mystery line in the console.
            warn!("[lint] policy '{hook}' produced a finding with no rule/message: {item:?}");
            continue;
        }
        let severity = match text("severity").trim().to_ascii_lowercase().as_str() {
            "error" | "err" => DiagnosticSeverity::Error,
            "info" => DiagnosticSeverity::Info,
            _ => DiagnosticSeverity::Warning,
        };
        out.push(
            Diagnostic::new(severity, message, None, None)
                .with_domain(domain)
                .with_source(hook_id(domain))
                .with_code(rule)
                .with_subject(text("subject")),
        );
    }
    out
}

fn policy_failure(domain: &str, rule: &str, hook: &str, message: String) -> Diagnostic {
    Diagnostic::error(message, None, None)
        .with_domain(domain)
        .with_source(hook)
        .with_code(rule)
        .with_subject(hook)
}

/// Bevy wiring: the report resource, cleared when a scene is torn down.
///
/// Deliberately NOT a scene-lifecycle dependency — this crate stays substrate, so
/// the app (or the domain plugin) clears it. [`clear_on_scene_teardown`] is the
/// system to add wherever that lifecycle lives.
pub struct LunCoLintPlugin;

impl Plugin for LunCoLintPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LintReport>();
    }
}

/// Reset the report — findings name subjects of the scene being replaced.
pub fn clear_report(mut report: ResMut<LintReport>) {
    *report = LintReport::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_hooks::{RegisteredHook, ScriptHook, register};
    use std::sync::Arc;

    #[test]
    fn prepared_policy_preserves_context_and_rejects_invalid_clock() {
        struct ContextProbe {
            expected: lunco_hooks::RuntimeExecutionContext,
            calls: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl ScriptHook for ContextProbe {
            fn invoke(
                &self,
                invocation: &lunco_hooks::HookInvocation<'_>,
            ) -> lunco_hooks::HookResult {
                assert_eq!(invocation.context, self.expected);
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(H::Array(Vec::new()))
            }
        }
        let context = lunco_hooks::RuntimeExecutionContext {
            route: Some(lunco_hooks::RuntimeRoute::twin_owned(
                lunco_hooks::RuntimeCycle::Simulation,
                9,
                7,
            )),
            phase: lunco_hooks::RuntimePhase::Behavior,
            clock: lunco_hooks::RuntimeClock::Simulation,
            time_seconds: Some(12.0),
            delta_seconds: Some(0.25),
            sequence: Some(48),
            producer: None,
        };
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        register(RegisteredHook {
            id: hook_id("test_prepared_context"),
            backend: "test".into(),
            deterministic: true,
            hook: Arc::new(ContextProbe {
                expected: context,
                calls: Arc::clone(&calls),
            }),
        });
        assert!(
            run_lint_with_context("test_prepared_context", H::Map(Vec::new()), context).is_empty()
        );
        let invalid = lunco_hooks::RuntimeExecutionContext {
            time_seconds: None,
            ..context
        };
        let findings = run_lint_with_context("test_prepared_context", H::Map(Vec::new()), invalid);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code.as_deref(), Some("policy-execution-failed"));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        lunco_hooks::unregister(&hook_id("test_prepared_context"));
    }

    /// A stand-in for a rhai policy: whatever the test wants to "author".
    struct Canned(Vec<H>);
    impl ScriptHook for Canned {
        fn invoke(&self, _invocation: &lunco_hooks::HookInvocation<'_>) -> lunco_hooks::HookResult {
            Ok(H::Array(self.0.clone()))
        }
    }

    struct Faulting;
    impl ScriptHook for Faulting {
        fn invoke(&self, _invocation: &lunco_hooks::HookInvocation<'_>) -> lunco_hooks::HookResult {
            Err(lunco_hooks::HookError("expected lint policy fault".into()))
        }
    }

    fn finding_map(rule: &str, sev: &str) -> H {
        H::map([
            ("rule", H::str(rule)),
            ("severity", H::str(sev)),
            ("subject", H::str("/Rover/Motor_FL")),
            ("message", H::str("came off")),
        ])
    }

    fn register_canned(domain: &str, items: Vec<H>) {
        let _ = register(RegisteredHook {
            id: hook_id(domain),
            backend: "test".into(),
            deterministic: false,
            hook: Arc::new(Canned(items)),
        });
    }

    #[test]
    fn policy_fault_is_reported_as_an_error_finding() {
        let _ = register(RegisteredHook {
            id: hook_id("test_fault"),
            backend: "test".into(),
            deterministic: false,
            hook: Arc::new(Faulting),
        });

        let findings = run_lint("test_fault", H::Unit);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code.as_deref(), Some("policy-execution-failed"));
        assert_eq!(findings[0].severity, DiagnosticSeverity::Error);
        assert_eq!(findings[0].subject.as_deref(), Some("lint.test_fault"));
        assert!(findings[0].message.contains("expected lint policy fault"));

        lunco_hooks::unregister(&hook_id("test_fault"));
    }

    #[test]
    fn no_policy_means_no_findings() {
        assert!(run_lint("domain_with_no_policy", H::Unit).is_empty());
    }

    #[test]
    fn policy_findings_are_parsed_with_severity() {
        register_canned(
            "test_parse",
            vec![
                finding_map("nested-body-no-joint", "error"),
                finding_map("slow", "info"),
            ],
        );
        let f = run_lint("test_parse", H::Unit);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].severity, DiagnosticSeverity::Error);
        assert_eq!(f[0].code.as_deref(), Some("nested-body-no-joint"));
        assert_eq!(f[0].domain.as_deref(), Some("test_parse"));
        assert_eq!(f[1].severity, DiagnosticSeverity::Info);
        lunco_hooks::unregister(&hook_id("test_parse"));
    }

    /// A mistyped severity must still raise the finding — silence is the one
    /// failure mode a linter cannot afford.
    #[test]
    fn unknown_severity_becomes_warn() {
        register_canned("test_sev", vec![finding_map("r", "CRITICAL!!")]);
        let f = run_lint("test_sev", H::Unit);
        assert_eq!(f[0].severity, DiagnosticSeverity::Warning);
        lunco_hooks::unregister(&hook_id("test_sev"));
    }

    /// A finding with no rule or no message names nothing and is dropped rather
    /// than logged as a mystery.
    #[test]
    fn incomplete_findings_are_skipped() {
        register_canned(
            "test_incomplete",
            vec![H::map([("severity", H::str("error"))])],
        );
        assert!(run_lint("test_incomplete", H::Unit).is_empty());
        lunco_hooks::unregister(&hook_id("test_incomplete"));
    }

    #[test]
    fn clear_domain_only_drops_that_domain() {
        let mut r = LintReport::default();
        r.extend_logged(vec![
            Diagnostic::error("m", None, None)
                .with_domain("usd")
                .with_code("a")
                .with_subject("/x"),
            Diagnostic::warning("m", None, None)
                .with_domain("rhai")
                .with_code("b")
                .with_subject("s.rhai"),
        ]);
        assert_eq!(r.errors(), 1);
        r.clear_domain("usd");
        assert_eq!(r.findings.len(), 1);
        assert_eq!(r.findings[0].domain.as_deref(), Some("rhai"));
    }

    #[test]
    fn scope_lifecycle_is_independent_and_rejects_superseded_results() {
        let mut report = LintReport::default();
        let first_twin = report.begin_scope("twin");
        let loaded = report.begin_scope("loaded_stages");
        report.complete_scope("loaded_stages", loaded);
        assert_eq!(report.scopes["loaded_stages"].state, LintScopeState::Ready);
        assert_eq!(report.scopes["twin"].state, LintScopeState::Pending);

        let current_twin = report.begin_scope("twin");
        report.fail_scope("twin", first_twin, "superseded result");
        assert_eq!(report.scopes["twin"].state, LintScopeState::Pending);
        report.fail_scope("twin", current_twin, "source set unavailable");
        assert_eq!(
            report.scopes["twin"].state,
            LintScopeState::Failed("source set unavailable".to_owned())
        );
        assert_eq!(report.scopes["loaded_stages"].state, LintScopeState::Ready);
    }
}
