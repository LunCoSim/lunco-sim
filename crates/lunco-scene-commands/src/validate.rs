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
//!   the USD-cosim dispatcher runs (`lunco-usd-sim/src/cosim.rs`); the
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
//! - `.rhai` — `rhai::Engine::compile` only; nothing is executed.
//!
//! Registered as [`ApiQueryProvider`]s (they return data, like
//! [`crate::usd_prim_query`]), so one implementation answers rhai `query()`,
//! Python, raw HTTP and MCP:
//! `{"type":"ExecuteCommand","command":"ValidateAsset","params":{"path":"lunco://models/X.mo"}}`.

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_hooks::HookValue as H;
use lunco_usd_bevy::{CanonicalStage, UsdRead};
use serde_json::json;
use std::path::{Path, PathBuf};

/// The structured verdict on one asset file. Serialized verbatim as the
/// query's `data` payload.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ValidationReport {
    /// The path as the caller gave it.
    pub path: String,
    /// Asset kind, from the extension: `modelica` | `usd` | `wgsl` | `rhai` |
    /// `behavior_tree`.
    pub kind: String,
    /// True iff `errors` is empty — the file would survive the load path.
    pub ok: bool,
    /// Human-readable, with `line N:` prefixes where the parser gives them.
    pub errors: Vec<String>,
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
    match lunco_assets::engine_asset_local_path(reference) {
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
        "wgsl" => validate_wgsl(reference, &text),
        "rhai" => validate_rhai(reference, &text),
        other => ValidationReport::new(reference, "unknown").error(format!(
            "unsupported extension `.{other}` — supported: .mo, .usda, .wgsl, .rhai"
        )),
    };
    apply_lint_policy(report, &text)
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
    match lunco_assets::engine_asset_local_path(reference) {
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
            )
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
        read_errors: snapshot.read_errors.clone(),
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
    if let Err(e) = lunco_usd_bevy::author::usda_to_data(text) {
        return report.error(format!("usda parse: {e}"));
    }

    let engine_assets = engine_assets_root();
    let stage = match lunco_usd_bevy::compose_file_to_stage_with_assets(
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
    report.lint_facts = Some(crate::lint_command::usd_physics_facts(&view));

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
    // Every control binding's intent must be one `parse_user_intent` knows.
    let mut control_bindings = Vec::new();
    for prim in view.prim_paths() {
        if !is_controls_scope(&view, &prim) {
            continue;
        }
        for bind in view.children(&prim) {
            let Some(name) = bind.name() else { continue };
            if lunco_core::parse_user_intent(name).is_some() {
                control_bindings.push(json!({
                    "prim": bind.as_str(),
                    "intent": name,
                    // `text` not `scalar::<String>`: a port may be authored as a
                    // `string` or a `token` and those are distinct sdf values.
                    "port": view.text(&bind, "lunco:port"),
                }));
                continue;
            }
            let hint = match closest_intent(name) {
                Some(s) => format!(" — did you mean `{s}`?"),
                None => String::new(),
            };
            report.errors.push(format!(
                "control binding {} names `{name}`, which is not a control intent — \
                 the loader skips it, so this binding never actuates anything{hint}",
                bind.as_str()
            ));
            control_bindings.push(json!({
                "prim": bind.as_str(),
                "intent": name,
                "ok": false,
            }));
        }
    }

    report.info = json!({
        "wheel_prims": wheel_prims,
        "control_bindings": control_bindings,
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
    let runtime_root = lunco_assets::assets_dir_abs();
    if runtime_root.is_dir() {
        return runtime_root;
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// A prim whose children carry intent→port bindings. Named `Controls` is the
/// form a vessel composes (`lunco-usd-bevy` matches exactly that name), but the
/// shared profiles in `vessels/control_profiles.usda` are `RoverControls` /
/// `LanderControls` — validating THAT file is the point, since a typo authored
/// once there reaches every vessel referencing it. So the shape decides, not the
/// name: a scope is any prim with a child that authors `lunco:port`.
fn is_controls_scope(view: &impl UsdRead, prim: &openusd::sdf::Path) -> bool {
    view.children(prim)
        .iter()
        .any(|c| c.name().is_some() && view.attr_names(c).iter().any(|a| a == "lunco:port"))
}

/// Canonical names [`lunco_core::parse_user_intent`] accepts. Used ONLY to suggest a
/// correction: the check itself calls `parse_user_intent`, never this list, so a
/// stale entry can never accept or reject a binding — only make a hint worse.
/// `intent_spellings_all_parse` pins every entry against the real parser.
const INTENT_SPELLINGS: &[&str] = &[
    "forward",
    "backward",
    "left",
    "right",
    "yaw_left",
    "yaw_right",
    "action",
    "thrust",
    "release",
    "switch_mode",
    "pause",
    "cancel",
];

/// Nearest accepted spelling, when the name is close enough to be a typo rather
/// than a different word. `None` suppresses the hint.
fn closest_intent(name: &str) -> Option<&'static str> {
    let lower = name.trim().to_ascii_lowercase();
    let (best, dist) = INTENT_SPELLINGS
        .iter()
        .map(|c| (*c, edit_distance(&lower, c)))
        .min_by_key(|(_, d)| *d)?;
    (dist <= 2 && dist < lower.len()).then_some(best)
}

/// Levenshtein distance, two-row DP.
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
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

/// Register the provider. Called by [`crate::commands::SpawnCommandPlugin`],
/// so any binary with the scene verbs answers `ValidateAsset` too — the
/// headless server included.
pub fn register(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(ValidateAssetProvider);
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

    /// A control scope whose children are named `intent`, one per line.
    fn controls_usda(intents: &[&str]) -> String {
        let mut s = String::from("#usda 1.0\n\ndef \"RoverControls\"\n{\n");
        for i in intents {
            s.push_str(&format!(
                "    def \"{i}\"\n    {{\n        uniform string lunco:port = \"throttle\"\n        uniform double lunco:factor = 1\n    }}\n"
            ));
        }
        s.push_str("}\n");
        s
    }

    #[test]
    fn valid_control_profile_produces_no_diagnostics() {
        let path = temp_usda(
            "valid_profile.usda",
            &controls_usda(&["forward", "backward", "left", "right", "thrust"]),
        );
        let report = validate_asset(path.to_str().unwrap());
        assert!(report.ok, "{:?}", report.errors);
        // NOT vacuous: the scope must actually have been walked. Without this the
        // test would still pass if `is_controls_scope` never matched anything.
        let found = report.info["control_bindings"]
            .as_array()
            .expect("control_bindings in info")
            .len();
        assert_eq!(found, 5, "{:?}", report.info);
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
    fn misspelled_intent_is_reported_once_with_a_suggestion() {
        let path = temp_usda(
            "typo_profile.usda",
            &controls_usda(&["forwrad", "backward", "left", "right", "action"]),
        );
        let report = validate_asset(path.to_str().unwrap());
        assert!(!report.ok, "a misspelled intent must fail validation");
        let hits: Vec<&String> = report
            .errors
            .iter()
            .filter(|e| e.contains("not a control intent"))
            .collect();
        assert_eq!(hits.len(), 1, "{:?}", report.errors);
        assert!(hits[0].contains("forwrad"), "{}", hits[0]);
        assert!(hits[0].contains("did you mean `forward`"), "{}", hits[0]);
    }

    /// The suggestion table is a MIRROR of `parse_user_intent`, never an
    /// authority. If an entry stops parsing, the mirror has drifted.
    #[test]
    fn intent_spellings_all_parse() {
        for s in INTENT_SPELLINGS {
            assert!(
                lunco_core::parse_user_intent(s).is_some(),
                "`{s}` is no longer an accepted intent spelling"
            );
        }
    }

    #[test]
    fn unknown_extension_lists_supported() {
        let report = validate_asset("no/such/file.xyz");
        assert!(!report.ok);
    }
}
