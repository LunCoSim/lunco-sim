//! `ValidateAsset` and `ValidateTwin` — read-only pre-flight checks for asset
//! files and Twin-wide resolver namespaces.
//!
//! ## The light-path contract
//!
//! This is the PARSE-ONLY tier: no solver instance, no scene load, no
//! `AssetServer`, no GPU, no ECS state read or written. Every check here is a
//! pure function over file bytes (plus, for `.usda`, the referenced layers the
//! composer opens), so it is safe to call from anywhere — the HTTP API of a
//! running instance, or `luncosim --validate <path>` before any app exists.
//! Asset authors get "will this load?" in milliseconds instead of finding out
//! by spawning it into a live sim.
//!
//! Per extension:
//! - `.mo` — the same `lunco_modelica_ast::parse_to_syntax` + AST extraction
//!   the USD-cosim dispatcher runs (`lunco-usd-sim-cosim/src/lib.rs`); the
//!   reloadable `lint.modelica` policy decides which AST constructs are
//!   actionable. NO compile, NO `ModelicaCommand` dispatch.
//! - `.usda` — parse the layer (`usda_to_data`), compose the file
//!   (`compose_file_to_stage`), then run the SAME `WheelParams::read` the
//!   spawner runs on every `PhysxVehicleWheelAPI` prim — a wheel that would
//!   refuse to spawn fails validation here, with the exact attribute names.
//!   Control bindings are checked against the same authority the loader uses
//!   (`lunco_core::parse_user_intent`): `ControlBinding` load is deliberately
//!   TOLERANT — an unknown intent warns and is skipped — so a typo silently
//!   costs one control at runtime. This is where that becomes a hard error, on
//!   purpose: tolerant load, strict pre-flight.
//! - `.wgsl` — reflect the `Material` param schema (`ParamSchema::parse`).
//!   Full naga module validation is deliberately absent: naga is not a direct
//!   dependency of this crate and the light path adds none.
//! - `.sysml`/`.kerml` — the shared SysML parser/resolver, with no document
//!   mutation or requirement execution.
//! - `.rhai` — `rhai::Engine::compile` only; nothing is executed.
//!
//! Registered as [`ApiQueryProvider`]s (they return data, like
//! `lunco_scene_queries::usd_prim_query`), so one implementation answers rhai `query()`,
//! Python, raw HTTP and MCP:
//! `{"type":"ExecuteCommand","command":"ValidateAsset","params":{"path":"lunco://models/X.mo"}}`.

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_hooks::HookValue as H;
use lunco_usd_bevy_core::{canonical::CanonicalStage, UsdRead};
use serde_json::json;
use std::path::{Path, PathBuf};

/// The structured verdict on one asset file. Serialized verbatim as the
/// query's `data` payload.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ValidationReport {
    /// The path as the caller gave it.
    pub path: String,
    /// Asset kind, from the extension: `modelica` | `usd` | `sysml` | `wgsl` | `rhai`.
    pub kind: String,
    /// True iff `errors` is empty — the file would survive the load path.
    pub ok: bool,
    /// Human-readable, with `line N:` prefixes where the parser gives them.
    pub errors: Vec<String>,
    /// Non-fatal diagnostics produced by the loader or authored lint policy.
    pub warnings: Vec<String>,
    /// Kind-specific extras: `model`/`params`/`inputs` (.mo),
    /// `wheel_prims` (.usda), `shader_params` (.wgsl).
    pub info: serde_json::Value,
    /// Domain-shaped facts handed to the authored lint rules for this kind
    /// (`apply_lint_policy`). Not part of the API payload — it is the linter's
    /// input, not the caller's answer — so it is skipped on serialization and
    /// taken (not cloned) when the rules run.
    #[serde(skip)]
    pub(crate) lint_facts: Option<H>,
}

impl ValidationReport {
    fn new(path: &str, kind: &str) -> Self {
        Self {
            path: path.to_string(),
            kind: kind.to_string(),
            ok: true,
            errors: Vec::new(),
            warnings: Vec::new(),
            info: json!({}),
            lint_facts: None,
        }
    }

    fn error(mut self, msg: impl Into<String>) -> Self {
        self.errors.push(msg.into());
        self.ok = false;
        self
    }

    fn finish(mut self) -> Self {
        self.ok = self.errors.is_empty();
        self
    }
}

/// Resolve the caller's reference to a disk file, mirroring the engine's
/// `lunco://` mount (`lunco-assets`): a path that exists as given (absolute or
/// cwd-relative — the CLI case) wins; otherwise `lunco://x` and bare library
/// paths resolve against `<cwd>/assets` exactly as the `AssetServer` would.
/// Native-only by construction — there is no local filesystem on wasm.
fn resolve(reference: &str) -> Result<PathBuf, String> {
    let as_given = Path::new(reference);
    if as_given.is_file() {
        return Ok(as_given.to_path_buf());
    }
    match lunco_assets_core::engine_asset_local_path(reference) {
        Some(p) if p.is_file() => Ok(p),
        Some(p) => Err(format!(
            "file not found: `{reference}` (tried as given, then {})",
            p.display()
        )),
        None => Err(format!(
            "`{reference}` carries a scheme this pre-flight check cannot resolve \
             without a running instance (twin://, http…) — pass a lunco:// or \
             filesystem path"
        )),
    }
}

/// Validate one asset file, dispatching on its extension. Pure: reads the
/// file (and, for `.usda`, its referenced layers) and nothing else.
pub fn validate_asset(reference: &str) -> ValidationReport {
    let path = match resolve(reference) {
        Ok(p) => p,
        Err(e) => return ValidationReport::new(reference, "unknown").error(e),
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            return ValidationReport::new(reference, "unknown")
                .error(format!("cannot read {}: {e}", path.display()));
        }
    };
    let report = match ext.as_str() {
        "mo" => validate_modelica(reference, &path, &text),
        "usda" => validate_usda(reference, &path, &text),
        "sysml" | "kerml" => validate_sysml(reference, &path, &text),
        "wgsl" => validate_wgsl(reference, &text),
        "rhai" => validate_rhai(reference, &text),
        other => ValidationReport::new(reference, "unknown").error(format!(
            "unsupported extension `.{other}` — supported: .mo, .usda, .sysml, .kerml, .wgsl, .rhai"
        )),
    };
    apply_lint_policy(report, &text)
}

// ─── .sysml / .kerml ───────────────────────────────────────────────────────

/// Parse and resolve one SysML source file through the same pure AST boundary
/// used by the runtime document. Standard-library diagnostics are excluded from
/// the report because this pre-flight call concerns only the supplied file.
fn validate_sysml(reference: &str, path: &Path, text: &str) -> ValidationReport {
    let analysis = lunco_sysml_ast::SysmlAnalysis::build(
        [(path.to_string_lossy().to_string(), text.to_owned())],
        true,
        lunco_hash::fnv1a64(text.as_bytes()),
    );
    finish_sysml_report(reference, &analysis)
}

fn finish_sysml_report(
    reference: &str,
    analysis: &lunco_sysml_ast::SysmlAnalysis,
) -> ValidationReport {
    let mut report = ValidationReport::new(reference, "sysml");
    for diagnostic in analysis.diagnostics() {
        report.errors.push(format!(
            "{}:{}..{}: {}",
            diagnostic.file, diagnostic.start, diagnostic.end, diagnostic.message
        ));
    }
    report.info = json!({
        "source_files": analysis
            .files()
            .iter()
            .map(|file| file.name.clone())
            .collect::<Vec<_>>(),
        "elements": analysis.elements(),
        "references": analysis.references(),
        // Expose the typed semantic projection. Rhai tests consume this
        // snapshot without walking source text or reimplementing parsing.
        "attributes": lunco_sysml_report::attributes(&analysis),
        // The short-name map is convenient for authored Twin contracts.  The
        // qualified projection is the lossless lookup for workspaces where
        // two definitions intentionally reuse a local attribute name.
        "attributes_qualified": lunco_sysml_report::attributes_qualified(&analysis),
        "attribute_collisions": lunco_sysml_report::attribute_collisions(&analysis),
        "attribute_records": analysis.attributes(),
        "requirement_records": analysis.requirements(),
        "verification_cases": analysis.verifications(),
        // A filesystem/Twin caller may attach the manifest-owned registry
        // below.  Keeping an empty value on the single-file path makes the
        // response shape stable for generic Rhai consumers.
        "verification_registry": json!([]),
        "verification_registry_errors": json!([]),
        "components": json!([]),
        "component_registry_errors": json!([]),
        "source_revision": analysis.source_revision(),
        // Keep a lossless textual form alongside the JSON number. Rhai's
        // bounded value bridge represents JSON numbers as f64, which is not
        // sufficient to round-trip every u64 content hash.
        "source_revision_hex": format!("0x{:016x}", analysis.source_revision()),
        "stdlib": analysis.includes_stdlib(),
    });
    // Keep the policy input on the same immutable analysis snapshot as the
    // report. `ValidateAsset` and `ValidateSysml` must not silently diverge:
    // both paths run the same typed SysML facts through `lint.sysml`.
    report.lint_facts = Some(lunco_sysml_ast::lint_facts::sysml_facts(analysis));
    report.finish()
}

/// One Twin-level lint finding in the pre-flight response.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TwinValidationFinding {
    /// Stable authored rule id.
    pub rule: String,
    /// Policy severity.
    pub severity: String,
    /// Name or Twin subject involved.
    pub subject: String,
    /// Actionable diagnostic text.
    pub message: String,
}

/// Read-only Twin-wide resolver namespace pre-flight report.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TwinValidationReport {
    /// Folder supplied by the caller.
    pub path: String,
    /// Twin display name, or the folder name for an unmanifested folder.
    pub twin: String,
    /// Absolute root inspected by the namespace reader.
    pub root: String,
    /// `warn` by default; `error` makes collisions fail the report.
    pub policy: String,
    /// True iff no policy finding has error severity and the Twin opened.
    pub ok: bool,
    /// Error-severity policy findings and open/read failures.
    pub errors: Vec<String>,
    /// Warning/info policy findings.
    pub warnings: Vec<String>,
    /// Every resolver entry used to form the collision index.
    pub entries: Vec<crate::twin_lint::NamespaceEntry>,
    /// Only names ambiguous in an actual resolver scope.
    pub collisions: Vec<crate::twin_lint::NamespaceCollision>,
    /// Source files the read-only index could not inspect.
    pub read_errors: Vec<String>,
    /// Structured policy findings, for callers that do not parse display lines.
    pub findings: Vec<TwinValidationFinding>,
}

fn twin_validation_error(
    reference: &str,
    policy: &str,
    message: impl Into<String>,
) -> TwinValidationReport {
    let message = message.into();
    TwinValidationReport {
        path: reference.to_string(),
        twin: String::new(),
        root: String::new(),
        policy: policy.to_string(),
        ok: false,
        errors: vec![message],
        warnings: Vec::new(),
        entries: Vec::new(),
        collisions: Vec::new(),
        read_errors: Vec::new(),
        findings: Vec::new(),
    }
}

fn resolve_twin_root(reference: &str) -> Result<PathBuf, String> {
    let as_given = Path::new(reference);
    if as_given.is_dir() {
        return Ok(as_given.to_path_buf());
    }
    match lunco_assets_core::engine_asset_local_path(reference) {
        Some(path) if path.is_dir() => Ok(path),
        Some(path) => Err(format!(
            "Twin folder not found: `{reference}` (resolved `{}` is not a directory)",
            path.display()
        )),
        None => Err(format!(
            "`{reference}` is not a local Twin folder; pass a filesystem path or a lunco:// asset directory"
        )),
    }
}

/// Validate all resolver namespaces visible from one Twin folder.
///
/// This is the pure pre-flight counterpart of `RunLint { scope: "twin" }`:
/// both call the same Twin inspector and authored `lint.twin` policy. The
/// explicit folder argument keeps this API independent of an active ECS
/// workspace and makes it suitable for CI.
pub fn validate_twin(reference: &str, requested_policy: &str) -> TwinValidationReport {
    let policy = match crate::twin_lint::policy_name(requested_policy) {
        Ok(policy) => policy,
        Err(message) => return twin_validation_error(reference, requested_policy, message),
    };
    let root = match resolve_twin_root(reference) {
        Ok(root) => root,
        Err(message) => return twin_validation_error(reference, policy, message),
    };
    let mode = match lunco_twin::TwinMode::open(&root) {
        Ok(mode) => mode,
        Err(error) => return twin_validation_error(reference, policy, error.to_string()),
    };
    let twin = match mode {
        lunco_twin::TwinMode::Folder(twin) | lunco_twin::TwinMode::Twin(twin) => twin,
        lunco_twin::TwinMode::Orphan(path) => {
            return twin_validation_error(
                reference,
                policy,
                format!(
                    "Twin validation requires a folder, got file `{}`",
                    path.display()
                ),
            );
        }
    };
    let snapshot = crate::twin_lint::inspect_twin(&twin);
    let lint_findings = lunco_lint::run_lint("twin", crate::twin_lint::facts(&snapshot, policy));
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut findings = Vec::with_capacity(lint_findings.len());
    for finding in lint_findings {
        let line = finding.line();
        match finding.severity {
            lunco_lint::LintSeverity::Error => errors.push(line),
            _ => warnings.push(line),
        }
        findings.push(TwinValidationFinding {
            rule: finding.rule,
            severity: finding.severity.as_str().to_string(),
            subject: finding.subject,
            message: finding.message,
        });
    }
    if twin.manifest.as_ref().is_some_and(|manifest| {
        manifest.sysml.is_some()
            || manifest.verification.is_some()
            || !manifest.components.is_empty()
    }) {
        if let Err(source_errors) = twin.discover_sysml_sources_checked() {
            errors.extend(
                source_errors
                    .into_iter()
                    .map(|error| format!("Twin SysML source set: {error}")),
            );
        }
        errors.extend(
            twin.verification_registry_errors()
                .into_iter()
                .map(|error| format!("Twin verification registry: {error}")),
        );
        errors.extend(
            twin.component_registry_errors()
                .into_iter()
                .map(|error| format!("Twin component registry: {error}")),
        );
    }
    TwinValidationReport {
        path: reference.to_string(),
        twin: snapshot.twin.clone(),
        root: snapshot.root.clone(),
        policy: policy.to_string(),
        ok: errors.is_empty(),
        errors,
        warnings,
        entries: snapshot.entries.clone(),
        collisions: snapshot.collisions.clone(),
        read_errors: snapshot.read_errors,
        findings,
    }
}

/// Consult the DOMAIN's authored lint rules and fold their findings into the
/// report.
///
/// The checks above are what the LOADER itself would refuse — compiled, because
/// they are the loader's own code paths. This is the other half: rules about what
/// is merely WRONG, authored in `assets/scripting/policy/lint_<domain>.rhai` and
/// replaceable on a running sim (`register_hook("lint.usd", …)`). One linter per
/// domain: a USD rule, a Modelica rule and a script rule share no vocabulary.
///
/// The facts are the ones the pre-flight already computed — `report.info` per
/// kind, plus the source text for the text-shaped domains — so a rule author gets
/// the same picture the validator has. The USD domain additionally hands over the
/// full physics projection (see [`usd_lint_facts`]).
///
/// Findings never fail a file that the loader would accept: `error` severities
/// join `errors` (and flip `ok`), everything else joins `warnings`.
fn apply_lint_policy(mut report: ValidationReport, text: &str) -> ValidationReport {
    if report.kind == "unknown" {
        return report;
    }
    let mut facts = vec![
        ("path".to_string(), H::str(report.path.clone())),
        ("kind".to_string(), H::str(report.kind.clone())),
        ("ok".to_string(), H::Bool(report.ok)),
        (
            "errors".to_string(),
            H::Array(report.errors.iter().cloned().map(H::str).collect()),
        ),
        // The source itself, for the text-shaped domains: a rule about a script
        // ("a test scenario that never emits a verdict") needs the text, and
        // shipping it costs one clone of a file already in memory.
        ("source".to_string(), H::str(text.to_string())),
    ];
    // The domain's own facts are MERGED IN AT TOP LEVEL, not nested under a key.
    // A rule must see the identical shape whether it was reached from here or
    // from `RunLint` on the live scene — nest them here and `facts.bodies` is
    // suddenly `facts.subject.bodies`, every USD rule silently matches nothing,
    // and the linter reports a clean bill of health for a broken file. That is
    // exactly what happened the first time this was wired, and what
    // `the_deliberately_broken_scene_still_fails_the_same_gate` now pins.
    if let Some(H::Map(domain_facts)) = report.lint_facts.take() {
        facts.extend(domain_facts);
    }

    for f in lunco_lint::run_lint(&report.kind, H::Map(facts)) {
        match f.severity {
            lunco_lint::LintSeverity::Error => report.errors.push(f.line()),
            _ => report.warnings.push(f.line()),
        }
    }
    report.finish()
}

// ─── .mo ────────────────────────────────────────────────────────────────────

/// Rumoca PARSE phase only — the same call + extraction the USD-cosim
/// dispatcher makes (`dispatch_loaded_modelica_sources`). The authored
/// `lint.modelica` policy receives the resulting AST facts. No compile.
fn validate_modelica(reference: &str, path: &Path, text: &str) -> ValidationReport {
    let mut report = ValidationReport::new(reference, "modelica");

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("validate.mo");
    let syntax = lunco_modelica_ast::parse_to_syntax(text, file_name);
    if syntax.has_errors() {
        match syntax.parse_error() {
            Some(joined) => report.errors.extend(joined.lines().map(|l| l.to_string())),
            None => report
                .errors
                .push("parse failed (no diagnostic text from rumoca)".to_string()),
        }
    }

    // Lenient parsing still yields usable name/parameter/input snapshots —
    // same recovery semantics the cosim dispatcher relies on. The lint facts
    // below are projected from this exact AST, not from a second parse.
    let ast = syntax.best_effort();
    let interface = lunco_modelica_ast::ast_extract::parse_model_interface_from_ast(ast);
    let model_name = interface.model_name.clone();
    let parameters: std::collections::BTreeMap<String, f64> = interface
        .parameters
        .iter()
        .map(|(name, value)| (name.clone(), *value))
        .collect();
    let inputs: std::collections::BTreeMap<String, f64> = interface
        .inputs
        .iter()
        .map(|(name, value)| (name.clone(), *value))
        .collect();

    // The domain's own facts, for the authored rules. Merged at TOP LEVEL by
    // `apply_lint_policy` — see the warning there about nesting. All
    // declaration and equation facts come from the already-recovered AST.
    report.lint_facts =
        Some(lunco_modelica_ast::lint_facts::modelica_facts_from_interface(ast, &interface));

    report.info = json!({
        "model": model_name,
        "params": parameters,
        "inputs": inputs,
        // Outputs are not knowable at the parse phase — they are the model's
        // variables, which exist only after a compile.
        "outputs": serde_json::Value::Null,
    });
    report.finish()
}

// ─── .usda ──────────────────────────────────────────────────────────────────

/// Parse the layer, compose the file, then run the spawner's own
/// `WheelParams::read` over every `PhysxVehicleWheelAPI` prim.
fn validate_usda(reference: &str, path: &Path, text: &str) -> ValidationReport {
    let mut report = ValidationReport::new(reference, "usd");

    // The layer's own syntax first: a compose error on a referenced layer
    // should not mask a typo in THIS file.
    if let Err(e) = lunco_usd_authoring::author::usda_to_data(text) {
        return report.error(format!("usda parse: {e}"));
    }

    let engine_assets = engine_assets_root();
    let stage = match lunco_usd_bevy_core::compose::compose_file_to_stage_with_assets(
        path,
        Some(engine_assets.as_path()),
    ) {
        Ok(s) => s,
        Err(e) => return report.error(format!("compose: {e}")),
    };
    let stage = CanonicalStage::from_stage(stage, path.to_string_lossy().to_string());
    let view = stage.view();

    // The physics projection the `lint.usd` rules read — the SAME complete facts
    // the live lint command hands them, including USD-sim gear drives, so a rule
    // cannot pass here and fire at load, or the reverse.
    let (lint_facts, control_binding_info) =
        crate::lint_command::usd_physics_facts_with_control_info(&view);
    report.lint_facts = Some(lint_facts);

    // Every composed wheel must satisfy the ONE reader both wheel kinds spawn
    // through — `Err(missing)` here is exactly the refusal the spawner logs.
    let attachment_topology = lunco_usd_sim::wheel_params::collect_wheel_attachment_topology(&view);
    let mut wheel_prims = Vec::new();
    for prim in view.prim_paths() {
        if !view.has_api_schema(&prim, "PhysxVehicleWheelAPI") {
            continue;
        }
        let Some(attachment) = attachment_topology.binding_for(prim.as_str()) else {
            let reason = if attachment_topology.is_invalid(prim.as_str()) {
                "has malformed or ambiguous PhysxVehicleWheelAttachmentAPI topology"
            } else {
                "has no PhysxVehicleWheelAttachmentAPI binding"
            };
            report.errors.push(format!(
                "wheel {} would refuse to spawn — {}",
                prim.as_str(),
                reason
            ));
            wheel_prims.push(json!({
                "prim": prim.as_str(),
                "ok": false,
                "missing": [reason],
            }));
            continue;
        };
        let suspension = openusd::sdf::Path::new(&attachment.suspension).ok();
        let tire = openusd::sdf::Path::new(&attachment.tire).ok();
        match lunco_usd_sim::wheel_params::WheelParams::read(
            &view,
            &prim,
            suspension.as_ref(),
            tire.as_ref(),
        ) {
            Ok(_) => wheel_prims.push(json!({ "prim": prim.as_str(), "ok": true })),
            Err(missing) => {
                report.errors.push(format!(
                    "wheel {} would refuse to spawn — missing required attributes: {}",
                    prim.as_str(),
                    missing.join(", ")
                ));
                wheel_prims.push(json!({
                    "prim": prim.as_str(),
                    "ok": false,
                    "missing": missing,
                }));
            }
        }
    }
    report.info = json!({
        "wheel_prims": wheel_prims,
        "control_bindings": control_binding_info,
    });
    report.finish()
}

/// The shipped `lunco://` root for a parse-only tool.
///
/// A deployed binary runs from the workspace/application directory, which is
/// the same CWD-based root the AssetServer uses. Cargo tests instead run from
/// the crate directory, so use the compile-time workspace layout only when the
/// runtime root is absent.
pub(crate) fn engine_assets_root() -> PathBuf {
    let runtime_root = lunco_assets_core::assets_dir_abs();
    if runtime_root.is_dir() {
        return runtime_root;
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

// ─── .wgsl ──────────────────────────────────────────────────────────────────

/// Reflect the dynamic-param schema. Module-level (naga) validation is
/// skipped on purpose — see module docs.
fn validate_wgsl(reference: &str, text: &str) -> ValidationReport {
    let mut report = ValidationReport::new(reference, "wgsl");
    match lunco_materials::ParamSchema::parse(text) {
        Some(schema) => {
            let params: Vec<serde_json::Value> = schema
                .fields
                .iter()
                .map(|f| {
                    json!({
                        "name": f.name,
                        "type": format!("{:?}", f.ty),
                        "offset": f.offset,
                        "ui": format!("{:?}", f.ui),
                        "default": f.default.as_ref().map(|d| format!("{d:?}")),
                    })
                })
                .collect();
            if !lunco_materials::is_prop_pickable_source(text) {
                report.warnings.push(
                    "not prop-pickable: declares an `//!@engine` field that is not \
                     prop-fillable per the engine-param registry — the picker will \
                     skip it (it still loads as a scene shader)"
                        .to_string(),
                );
            }
            report.info = json!({
                "shader_params": params,
                "uniform_size": schema.size,
            });
        }
        None => {
            report.warnings.push(
                "no reflectable `Material` struct — the shader exposes no tunable \
                 params and cannot be driven by SetObjectProperty"
                    .to_string(),
            );
            report.info = json!({ "shader_params": [] });
        }
    }
    report.finish()
}

// ─── .rhai ──────────────────────────────────────────────────────────────────

/// Compile-only — a bare engine with no bindings; nothing runs. Scripts using
/// LunCo's registered functions still compile: rhai resolves names at call
/// time, not compile time.
fn validate_rhai(reference: &str, text: &str) -> ValidationReport {
    let report = ValidationReport::new(reference, "rhai");
    let mut engine = rhai::Engine::new();
    // Preflight and live execution must share the same parser/resource policy.
    // The comparison scenario uses the supported function depth; a bare Rhai
    // engine would reject it before the production scripting engine sees it.
    lunco_hooks_rhai::rhai_limits::apply(&mut engine);
    match engine.compile(text) {
        Ok(_) => report.finish(),
        // rhai's Display includes "line N, position M".
        Err(e) => report.error(format!("rhai compile: {e}")),
    }
}

// ─── CLI ────────────────────────────────────────────────────────────────────

/// One-shot CLI leg (`luncosim --validate <path>…`): print each report
/// human-readably, return the process exit code (0 = all ok, 1 = any failed).
/// No Bevy `App` is ever constructed on this path.
pub fn run_cli(paths: &[String]) -> i32 {
    let mut failed = false;
    for p in paths {
        let report = validate_asset(p);
        let verdict = if report.ok { "OK" } else { "FAIL" };
        println!("{verdict}  {} ({})", report.path, report.kind);
        for e in &report.errors {
            println!("  error: {e}");
        }
        for w in &report.warnings {
            println!("  warning: {w}");
        }
        failed |= !report.ok;
    }
    if failed {
        1
    } else {
        0
    }
}

// ─── API registration ───────────────────────────────────────────────────────

/// `ValidateAsset { path }` → [`ValidationReport`].
struct ValidateAssetProvider;

impl ApiQueryProvider for ValidateAssetProvider {
    fn name(&self) -> &'static str {
        "ValidateAsset"
    }

    fn execute(&self, _world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(path) = params.get("path").and_then(|p| p.as_str()) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "ValidateAsset requires params.path (string): a lunco:// or filesystem path",
            );
        };
        let report = validate_asset(path);
        match serde_json::to_value(&report) {
            Ok(v) => ApiResponse::ok(v),
            Err(e) => ApiResponse::error(ApiErrorCode::InternalError, e.to_string()),
        }
    }
}

/// Compact SysML requirement projection for authored Rhai tests.
///
/// `ValidateAsset` intentionally returns the complete semantic element list
/// for tooling. That payload is too large for the bounded Rhai value surface,
/// so this read-only provider reuses the same validator and returns only
/// requirement/verification names, scalar literals, diagnostics, and the
/// source revision.
struct ValidateSysmlProvider;

impl ApiQueryProvider for ValidateSysmlProvider {
    fn name(&self) -> &'static str {
        "ValidateSysml"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(path) = params.get("path").and_then(|p| p.as_str()) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "ValidateSysml requires params.path (string): a filesystem path or twin:// URI",
            );
        };
        let report = validate_sysml_reference(world, path);
        let requirements = qualified_names(report.info.get("requirement_records"));
        let verification_cases = qualified_names(report.info.get("verification_cases"));
        // Rhai has a deliberately bounded string/value surface.  The normal
        // ValidateAsset report keeps full AST records for IDE tooling, but a
        // test only needs names, scalar values, and verification coverage.
        // `compact=true` therefore projects the same validated snapshot into
        // a small deterministic record set instead of serializing the whole
        // AST through the scripting bridge.
        let compact = params
            .get("compact")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let value = if compact {
            json!({
                "path": report.path,
                "kind": report.kind,
                "ok": report.ok,
                "errors": report.errors,
                "warnings": report.warnings,
                "source_files": report.info.get("source_files").cloned().unwrap_or_else(|| json!([])),
                "requirements": requirements,
                // Keep one compact, lossless qualified-name table. A short-name
                // table cannot represent collisions (for example each
                // component's `namesSource`) and silently drops owners. The
                // Rhai bridge resolves qualified names directly; no duplicate
                // short/qualified maps are serialized through its bounded
                // value budget.
                "attributes": json!({}),
                "attributes_qualified": compact_sysml_attributes(report.info.get("attributes_qualified")),
                "attribute_collisions": report
                    .info
                    .get("attribute_collisions")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
                "requirement_records": compact_requirement_records(report.info.get("requirement_records")),
                "verification_cases": verification_cases,
                "verification_records": compact_verification_records(report.info.get("verification_cases")),
                "verification_registry": report.info.get("verification_registry").cloned().unwrap_or_else(|| json!([])),
                "verification_registry_errors": report.info.get("verification_registry_errors").cloned().unwrap_or_else(|| json!([])),
                "components": report.info.get("components").cloned().unwrap_or_else(|| json!([])),
                "component_registry_errors": report.info.get("component_registry_errors").cloned().unwrap_or_else(|| json!([])),
                "source_revision": report.info.get("source_revision").cloned().unwrap_or(json!(0)),
                "source_revision_hex": report.info.get("source_revision_hex").cloned().unwrap_or_else(|| json!("0x0000000000000000")),
            })
        } else {
            json!({
                "path": report.path,
                "kind": report.kind,
                "ok": report.ok,
                "errors": report.errors,
                "warnings": report.warnings,
                "source_files": report.info.get("source_files").cloned().unwrap_or_else(|| json!([])),
                "requirements": requirements,
                "attributes": report.info.get("attributes").cloned().unwrap_or_else(|| json!({})),
                "attributes_qualified": report
                    .info
                    .get("attributes_qualified")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
                "attribute_collisions": report
                    .info
                    .get("attribute_collisions")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
                "attribute_records": report.info.get("attribute_records").cloned().unwrap_or_else(|| json!([])),
                "requirement_records": report.info.get("requirement_records").cloned().unwrap_or_else(|| json!([])),
                "verification_cases": verification_cases,
                "verification_records": report.info.get("verification_cases").cloned().unwrap_or_else(|| json!([])),
                "verification_registry": report.info.get("verification_registry").cloned().unwrap_or_else(|| json!([])),
                "verification_registry_errors": report.info.get("verification_registry_errors").cloned().unwrap_or_else(|| json!([])),
                "components": report.info.get("components").cloned().unwrap_or_else(|| json!([])),
                "component_registry_errors": report.info.get("component_registry_errors").cloned().unwrap_or_else(|| json!([])),
                "source_revision": report.info.get("source_revision").cloned().unwrap_or(json!(0)),
                "source_revision_hex": report.info.get("source_revision_hex").cloned().unwrap_or_else(|| json!("0x0000000000000000")),
            })
        };
        ApiResponse::ok(value)
    }
}

fn compact_sysml_attributes(value: Option<&serde_json::Value>) -> serde_json::Value {
    // Preserve the qualified identity in every compact record; Rhai uses it
    // to resolve cross-package source references without short-name guessing.
    let mut output = serde_json::Map::new();
    let Some(serde_json::Value::Object(attributes)) = value else {
        return serde_json::Value::Object(output);
    };
    for (name, record) in attributes {
        // Keep only the identity and typed literal needed by the Rhai
        // requirement bridge.  The full ValidateAsset report remains the
        // source-span/IDE projection; duplicating owner/name/start/end for
        // both short and qualified maps can exceed Rhai's 64 KiB string
        // budget as a Twin grows.  `attribute_collisions` still prevents an
        // ambiguous short name from being used accidentally.
        let value = record
            .get("value")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        output.insert(
            name.clone(),
            json!({
                "qualified_name": record
                    .get("qualified_name")
                    .cloned()
                    .unwrap_or_else(|| json!(name)),
                "type_name": record.get("type_name").cloned().unwrap_or(serde_json::Value::Null),
                "value": value,
                "file": record.get("file").cloned().unwrap_or(serde_json::Value::Null),
            }),
        );
    }
    serde_json::Value::Object(output)
}

fn compact_requirement_records(value: Option<&serde_json::Value>) -> serde_json::Value {
    let records = value
        .and_then(serde_json::Value::as_array)
        .map(|records| {
            records
                .iter()
                .filter_map(|record| {
                    let name = record
                        .get("element")
                        .and_then(|element| element.get("qualified_name"))
                        .or_else(|| record.get("qualified_name"))
                        .cloned()?;
                    Some(json!({"qualified_name": name}))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    serde_json::Value::Array(records)
}

fn compact_verification_records(value: Option<&serde_json::Value>) -> serde_json::Value {
    let records = value
        .and_then(serde_json::Value::as_array)
        .map(|records| {
            records
                .iter()
                .filter_map(|record| {
                    let name = record
                        .get("element")
                        .and_then(|element| element.get("qualified_name"))
                        .or_else(|| record.get("qualified_name"))
                        .cloned()?;
                    let verifies = record.get("verifies").cloned().unwrap_or_else(|| json!([]));
                    Some(json!({"qualified_name": name, "verifies": verifies}))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    serde_json::Value::Array(records)
}

fn qualified_names(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|record| {
            record
                .get("element")
                .and_then(|element| element.get("qualified_name"))
                .or_else(|| record.get("qualified_name"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

fn validate_sysml_reference(world: &World, reference: &str) -> ValidationReport {
    if let Some(name) = reference.strip_prefix("twin://") {
        if !name.is_empty() && !name.contains('/') && !name.contains('\\') {
            return validate_sysml_twin(world, name, reference);
        }
    }
    let Some((name, relative)) = lunco_assets_core::parse_twin_uri(reference) else {
        return validate_asset(reference);
    };
    let Some(roots) = world.get_resource::<lunco_assets_core::TwinRoots>() else {
        return ValidationReport::new(reference, "sysml")
            .error("ValidateSysml twin:// requires the TwinRoots asset registry");
    };
    let path = match roots.resolve_file(name, Path::new(relative)) {
        Ok(Some(path)) => path,
        Ok(None) => {
            return ValidationReport::new(reference, "sysml")
                .error(format!("Twin `{name}` is not mounted"));
        }
        Err(error) => {
            return ValidationReport::new(reference, "sysml")
                .error(format!("cannot resolve {reference}: {error}"));
        }
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return ValidationReport::new(reference, "sysml")
                .error(format!("cannot read {}: {error}", path.display()));
        }
    };
    let kind = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if !matches!(kind.to_ascii_lowercase().as_str(), "sysml" | "kerml") {
        return ValidationReport::new(reference, "unknown")
            .error("ValidateSysml twin:// path must end in .sysml or .kerml");
    }
    let report = validate_sysml(reference, &path, &text);
    apply_lint_policy(report, &text)
}

fn validate_sysml_twin(world: &World, name: &str, reference: &str) -> ValidationReport {
    let Some(roots) = world.get_resource::<lunco_assets_core::TwinRoots>() else {
        return ValidationReport::new(reference, "sysml")
            .error("ValidateSysml twin:// requires the TwinRoots asset registry");
    };
    let root = match roots.root_of(name) {
        Ok(Some(root)) => root,
        Ok(None) => {
            return ValidationReport::new(reference, "sysml")
                .error(format!("Twin `{name}` is not mounted"));
        }
        Err(error) => {
            return ValidationReport::new(reference, "sysml")
                .error(format!("cannot resolve {reference}: {error}"));
        }
    };
    let mode = match lunco_twin::TwinMode::open(&root) {
        Ok(mode) => mode,
        Err(error) => {
            return ValidationReport::new(reference, "sysml")
                .error(format!("cannot open Twin `{name}`: {error}"));
        }
    };
    let twin = match mode {
        lunco_twin::TwinMode::Twin(twin) | lunco_twin::TwinMode::Folder(twin) => twin,
        lunco_twin::TwinMode::Orphan(path) => {
            return ValidationReport::new(reference, "sysml").error(format!(
                "Twin `{name}` resolved to a file, not a folder: {}",
                path.display()
            ));
        }
    };
    let relative_sources = match twin.discover_sysml_sources_checked() {
        Ok(sources) => sources,
        Err(errors) => {
            let mut report = ValidationReport::new(reference, "sysml");
            report.errors.extend(
                errors
                    .into_iter()
                    .map(|error| format!("Twin `{name}` SysML source set: {error}")),
            );
            return report.finish();
        }
    };

    let mut sources = Vec::with_capacity(relative_sources.len());
    let mut policy_source = String::new();
    let mut revision_input = Vec::new();
    for relative in relative_sources {
        let path = root.join(&relative);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                return ValidationReport::new(reference, "sysml")
                    .error(format!("cannot read {}: {error}", path.display()));
            }
        };
        let logical = lunco_assets_core::twin_uri(name, &relative);
        revision_input.extend_from_slice(logical.as_bytes());
        revision_input.push(0);
        revision_input.extend_from_slice(text.as_bytes());
        policy_source.push_str(&text);
        policy_source.push('\n');
        sources.push((logical, text));
    }
    let analysis = lunco_sysml_ast::SysmlAnalysis::build_cached(
        sources,
        true,
        lunco_hash::fnv1a64(&revision_input),
    );
    let mut report = finish_sysml_report(reference, &analysis);
    let registry_errors = twin.verification_registry_errors();
    let component_errors = twin.component_registry_errors();
    let component_errors_for_info = component_errors.clone();
    let verification_names = qualified_names(report.info.get("verification_cases"));
    let mut binding_errors = registry_errors.clone();
    binding_errors.extend(component_errors);
    for case in twin.verification_cases() {
        if !verification_names.iter().any(|name| name == &case.name) {
            binding_errors.push(format!(
                "verification `{}` is not declared by the Twin SysML source set",
                case.name
            ));
        }
    }
    if let Some(info) = report.info.as_object_mut() {
        info.insert(
            "verification_registry".to_owned(),
            json!(twin.verification_cases()),
        );
        info.insert(
            "verification_registry_errors".to_owned(),
            json!(binding_errors),
        );
        info.insert("components".to_owned(), json!(twin.components()));
        info.insert(
            "component_registry_errors".to_owned(),
            json!(component_errors_for_info),
        );
    }
    if !binding_errors.is_empty() {
        report.errors.extend(binding_errors);
        report.ok = false;
    }
    apply_lint_policy(report, &policy_source)
}

/// `ValidateTwin { path, policy? }` → [`TwinValidationReport`].
struct ValidateTwinProvider;

impl ApiQueryProvider for ValidateTwinProvider {
    fn name(&self) -> &'static str {
        "ValidateTwin"
    }

    fn execute(&self, _world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(path) = params.get("path").and_then(|p| p.as_str()) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "ValidateTwin requires params.path (string): a Twin folder path",
            );
        };
        let policy = params
            .get("policy")
            .and_then(|value| value.as_str())
            .unwrap_or("warn");
        let report = validate_twin(path, policy);
        match serde_json::to_value(&report) {
            Ok(value) => ApiResponse::ok(value),
            Err(error) => ApiResponse::error(ApiErrorCode::InternalError, error.to_string()),
        }
    }
}

/// Register the providers. Called by [`crate::SceneValidationPlugin`], so any
/// host that installs validation answers `ValidateAsset`, `ValidateSysml`, and
/// `ValidateTwin`.
pub fn register(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(ValidateAssetProvider);
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(ValidateSysmlProvider);
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(ValidateTwinProvider);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a `.usda` under the temp dir and hand back its path — the control
    /// checks run on a COMPOSED stage, so they can only be exercised through the
    /// real `validate_asset` entry point.
    fn temp_usda(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("lunco-validate-controls");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write temp usda");
        path
    }

    fn temp_sysml(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("lunco-validate-sysml");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write temp sysml");
        path
    }

    #[test]
    fn external_twin_scene_resolves_lunco_references_for_preflight() {
        let path = temp_usda(
            "external_twin.usda",
            "#usda 1.0\n\
def Xform \"Battery\" (\n\
    prepend references = @lunco://components/power/battery.usda@</Battery>\n\
)\n{\n}\n",
        );
        let report = validate_asset(path.to_str().unwrap());
        assert!(
            report.ok,
            "an external Twin gets the same lunco:// mount as runtime: {:?}",
            report.errors
        );
    }

    #[test]
    fn unknown_extension_lists_supported() {
        let report = validate_asset("no/such/file.xyz");
        assert!(!report.ok);
    }

    #[test]
    fn valid_sysml_produces_elements_and_no_diagnostics() {
        let path = temp_sysml(
            "example.sysml",
            "package Example { requirement def MassRequirement {} }",
        );
        let report = validate_asset(path.to_str().unwrap());
        assert!(report.ok, "{:?}", report.errors);
        assert_eq!(report.kind, "sysml");
        assert!(report.info["elements"]
            .as_array()
            .is_some_and(|elements| !elements.is_empty()));
    }

    #[test]
    fn sysml_attribute_projection_keeps_qualified_names_and_collisions() {
        let path = temp_sysml(
            "qualified_attributes.sysml",
            "package Example {
                private import ScalarValues::Real;
                part def Lander { attribute mass : Real = 1.0; }
                part def Rover { attribute mass : Real = 2.0; }
            }",
        );
        let report = validate_asset(path.to_str().unwrap());
        assert!(report.ok, "{:?}", report.errors);
        assert!(report.info["attributes_qualified"]
            .get("Example::Lander::mass")
            .is_some());
        assert!(report.info["attributes_qualified"]
            .get("Example::Rover::mass")
            .is_some());
        let collisions = report.info["attribute_collisions"]
            .as_array()
            .expect("collision array");
        assert_eq!(collisions.len(), 1, "{:?}", report.info);
        assert_eq!(collisions[0]["name"], "mass");
    }

    #[test]
    fn compact_sysml_projection_keeps_typed_source_identity() {
        let path = temp_sysml(
            "compact_attributes.sysml",
            "package Example {
                private import ScalarValues::Real;
                part def Lander { attribute mass : Real = 1.0; }
            }",
        );
        let report = validate_asset(path.to_str().unwrap());
        assert!(report.ok, "{:?}", report.errors);
        let compact = compact_sysml_attributes(report.info.get("attributes_qualified"));
        let mass = &compact["Example::Lander::mass"];
        assert_eq!(mass["qualified_name"], "Example::Lander::mass");
        assert_eq!(mass["value"]["kind"], "real");
        assert!(mass["file"]
            .as_str()
            .is_some_and(|file| file.ends_with("/compact_attributes.sysml")));
    }

    #[test]
    fn compact_sysml_projection_does_not_duplicate_short_attribute_map() {
        let path = temp_sysml(
            "compact_colliding_attributes.sysml",
            "package Example {
                private import ScalarValues::Real;
                part def Lander { attribute mass : Real = 1.0; }
                part def Rover { attribute mass : Real = 2.0; }
            }",
        );
        let report = validate_asset(path.to_str().unwrap());
        assert!(report.ok, "{:?}", report.errors);

        // The compact bridge has one identity-preserving table. Short names
        // are intentionally empty: emitting both maps doubles every record
        // and cannot represent colliding component attributes.
        let compact = compact_sysml_attributes(report.info.get("attributes_qualified"));
        assert!(compact["Example::Lander::mass"].is_object());
        assert!(compact["Example::Rover::mass"].is_object());
        assert!(compact.get("mass").is_none());
    }
}
