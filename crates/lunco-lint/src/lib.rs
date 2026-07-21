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
//! Linters are **separate per domain** — `usd`, `rhai`, `modelica`, and whatever
//! comes next — because their subjects, their vocabulary and the people who tune
//! them are different, and one giant rule file would be read by no one. The
//! substrate is universal; the rules are not shared. A domain is just a name:
//!
//! ```text
//!   domain "usd"      → hook `lint.usd`      → assets/scripting/policy/lint_usd.rhai
//!   domain "rhai"     → hook `lint.rhai`     → assets/scripting/policy/lint_rhai.rhai
//!   domain "modelica" → hook `lint.modelica` → assets/scripting/policy/lint_modelica.rhai
//! ```
//!
//! # The contract with a policy
//!
//! `lint_<domain>(facts) -> [ #{ rule, severity, subject, message }, … ]`
//!
//! `facts` is whatever the domain gathered ([`HookValue`] maps/arrays — typed, not
//! JSON). `severity` is `"error"`, `"warn"` or `"info"`; anything else is read as
//! `"warn"` rather than dropped, because a typo in a rule must not silently delete
//! the finding it was written to raise. A policy that returns a non-array, or
//! faults, yields no findings and logs why — a broken linter must never be able to
//! stop a scene from loading.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use bevy::prelude::*;
use lunco_hooks::HookValue as H;

/// The hook id a domain's rules are registered under: `lint.<domain>`.
///
/// A convention rather than a constant per domain, so a new domain needs no
/// change here — the crate that owns the subject picks the name.
pub fn hook_id(domain: &str) -> String {
    format!("lint.{domain}")
}

/// How much a finding matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Reflect)]
pub enum LintSeverity {
    /// What was authored does not do what it says. Someone must fix it.
    Error,
    /// Probably wrong, or wrong in a case the author may have meant.
    Warn,
    /// Worth knowing, never a defect.
    Info,
}

impl LintSeverity {
    /// Parse the policy's spelling. Unknown values read as [`Warn`](Self::Warn):
    /// a mistyped severity must surface the finding, not swallow it.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" | "err" => LintSeverity::Error,
            "info" => LintSeverity::Info,
            _ => LintSeverity::Warn,
        }
    }
    /// The policy-facing spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            LintSeverity::Error => "error",
            LintSeverity::Warn => "warn",
            LintSeverity::Info => "info",
        }
    }
}

/// One authoring problem, in any domain.
#[derive(Debug, Clone, Reflect)]
pub struct LintFinding {
    /// Which linter produced it (`usd`, `rhai`, `modelica`, …).
    pub domain: String,
    /// Stable rule id, e.g. `nested-body-no-joint` — greppable, and what a
    /// suppression list would key on.
    pub rule: String,
    /// How much it matters.
    pub severity: LintSeverity,
    /// What it is about: a prim path, a script document, a model name.
    pub subject: String,
    /// What is wrong and what to do about it, in that order.
    pub message: String,
}

impl LintFinding {
    /// The one-line form used for logs, toasts and test assertions.
    pub fn line(&self) -> String {
        format!("[{}/{}] {} — {}", self.domain, self.rule, self.subject, self.message)
    }
}

/// Every finding since the last scene load, from every domain.
///
/// A resource rather than an event stream because the interesting question is
/// "what is wrong with what is loaded right now" — a UI panel, a toast and a test
/// all want the current set, not the history.
#[derive(Resource, Default, Debug)]
pub struct LintReport {
    /// All findings, in the order they were produced.
    pub findings: Vec<LintFinding>,
    /// Findings not yet shown to the user. A UI bridge drains this to raise one
    /// toast per batch instead of one per finding.
    pub unreported: usize,
}

impl LintReport {
    /// Count of findings at [`LintSeverity::Error`].
    pub fn errors(&self) -> usize {
        self.findings.iter().filter(|f| f.severity == LintSeverity::Error).count()
    }
    /// Count of findings at [`LintSeverity::Warn`].
    pub fn warnings(&self) -> usize {
        self.findings.iter().filter(|f| f.severity == LintSeverity::Warn).count()
    }
    /// Drop everything a domain previously reported — what a domain calls before
    /// re-linting the same subject, so a fixed problem disappears instead of
    /// accumulating a duplicate.
    pub fn clear_domain(&mut self, domain: &str) {
        self.findings.retain(|f| f.domain != domain);
    }
    /// Log a batch and file it. Errors log at `error!`, warnings at `warn!`,
    /// info at `info!` — the console is the first place anyone looks.
    pub fn extend_logged(&mut self, findings: Vec<LintFinding>) {
        for f in &findings {
            match f.severity {
                LintSeverity::Error => error!("[lint] {}", f.line()),
                LintSeverity::Warn => warn!("[lint] {}", f.line()),
                LintSeverity::Info => info!("[lint] {}", f.line()),
            }
        }
        self.unreported += findings.len();
        self.findings.extend(findings);
    }
}

/// Ask a domain's authored rules what is wrong with `facts`.
///
/// Returns an empty vec when no policy is registered for the domain — the
/// no-scripting case — and when the policy faults or answers with something that
/// is not an array of finding maps. A linter is diagnostics: it may not break the
/// thing it is diagnosing.
pub fn run_lint(domain: &str, facts: H) -> Vec<LintFinding> {
    let hook = hook_id(domain);
    let Some(outcome) = lunco_hooks::invoke(&hook, &[facts]) else {
        // No rules authored for this domain. Not a problem, and not worth a log
        // line on every scene load.
        return Vec::new();
    };
    let result = match outcome {
        Ok(v) => v,
        Err(e) => {
            // A rule that throws is a broken RULE, not a broken scene. Say so
            // once, loudly enough to be fixed, and load anyway.
            error!("[lint] policy '{hook}' faulted: {e:?} — no findings from this domain");
            return Vec::new();
        }
    };
    let H::Array(items) = result else {
        warn!(
            "[lint] policy '{hook}' returned {result:?}, expected an array of \
             #{{rule, severity, subject, message}} — no findings recorded"
        );
        return Vec::new();
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
        out.push(LintFinding {
            domain: domain.to_string(),
            rule,
            severity: LintSeverity::parse(&text("severity")),
            subject: text("subject"),
            message,
        });
    }
    out
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
    use lunco_hooks::{register, RegisteredHook, ScriptHook};
    use std::sync::Arc;

    /// A stand-in for a rhai policy: whatever the test wants to "author".
    struct Canned(Vec<H>);
    impl ScriptHook for Canned {
        fn invoke(&self, _args: &[H]) -> lunco_hooks::HookResult {
            Ok(H::Array(self.0.clone()))
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
    fn no_policy_means_no_findings() {
        assert!(run_lint("domain_with_no_policy", H::Unit).is_empty());
    }

    #[test]
    fn policy_findings_are_parsed_with_severity() {
        register_canned(
            "test_parse",
            vec![finding_map("nested-body-no-joint", "error"), finding_map("slow", "info")],
        );
        let f = run_lint("test_parse", H::Unit);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].severity, LintSeverity::Error);
        assert_eq!(f[0].rule, "nested-body-no-joint");
        assert_eq!(f[0].domain, "test_parse");
        assert_eq!(f[1].severity, LintSeverity::Info);
        lunco_hooks::unregister(&hook_id("test_parse"));
    }

    /// A mistyped severity must still raise the finding — silence is the one
    /// failure mode a linter cannot afford.
    #[test]
    fn unknown_severity_becomes_warn() {
        register_canned("test_sev", vec![finding_map("r", "CRITICAL!!")]);
        let f = run_lint("test_sev", H::Unit);
        assert_eq!(f[0].severity, LintSeverity::Warn);
        lunco_hooks::unregister(&hook_id("test_sev"));
    }

    /// A finding with no rule or no message names nothing and is dropped rather
    /// than logged as a mystery.
    #[test]
    fn incomplete_findings_are_skipped() {
        register_canned("test_incomplete", vec![H::map([("severity", H::str("error"))])]);
        assert!(run_lint("test_incomplete", H::Unit).is_empty());
        lunco_hooks::unregister(&hook_id("test_incomplete"));
    }

    #[test]
    fn clear_domain_only_drops_that_domain() {
        let mut r = LintReport::default();
        r.extend_logged(vec![
            LintFinding {
                domain: "usd".into(),
                rule: "a".into(),
                severity: LintSeverity::Error,
                subject: "/x".into(),
                message: "m".into(),
            },
            LintFinding {
                domain: "rhai".into(),
                rule: "b".into(),
                severity: LintSeverity::Warn,
                subject: "s.rhai".into(),
                message: "m".into(),
            },
        ]);
        assert_eq!(r.errors(), 1);
        r.clear_domain("usd");
        assert_eq!(r.findings.len(), 1);
        assert_eq!(r.findings[0].domain, "rhai");
    }
}
