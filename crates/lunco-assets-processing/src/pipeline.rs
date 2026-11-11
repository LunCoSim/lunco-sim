//! Artifact recipe execution on the existing processing worker and transaction.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use lunco_assets_datasets::{
    ProcessConfig,
    pipeline::{self, ArtifactDigest, PipelineReceipt},
};
use lunco_hooks::{HookInvocation, HookValue, ScriptHook};

use crate::process::{ProcessControl, ProcessorRegistry};

fn invalid(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into())
}

/// Recipe-local callable seam. It consumes no ECS state or simulation clock.
const PLAN_HOOK: &str = "assets.process.plan";
lunco_hooks::declare_hook! {
    id: PLAN_HOOK,
    owner: "lunco-assets-processing",
    description: "Transform an ordered artifact recipe using immutable parameters and declared inputs.",
    signature: [facts: Map],
    output: Map,
    deterministic: true,
    required: false,
    installable: false,
}

fn to_hook(value: toml::Value) -> std::io::Result<HookValue> {
    Ok(match value {
        toml::Value::String(v) => HookValue::Str(v),
        toml::Value::Integer(v) => HookValue::Int(v),
        toml::Value::Float(v) if v.is_finite() => HookValue::Float(v),
        toml::Value::Boolean(v) => HookValue::Bool(v),
        toml::Value::Array(v) => {
            HookValue::Array(v.into_iter().map(to_hook).collect::<Result<_, _>>()?)
        }
        toml::Value::Table(v) => HookValue::Map(
            v.into_iter()
                .map(|(k, v)| Ok((k, to_hook(v)?)))
                .collect::<std::io::Result<_>>()?,
        ),
        _ => {
            return Err(invalid(
                "pipeline facts must contain finite, ordinary TOML values",
            ));
        }
    })
}

fn from_hook(value: HookValue) -> std::io::Result<toml::Value> {
    Ok(match value {
        HookValue::Str(v) => v.into(),
        HookValue::Int(v) => v.into(),
        HookValue::Float(v) if v.is_finite() => v.into(),
        HookValue::Bool(v) => v.into(),
        HookValue::Array(v) => {
            toml::Value::Array(v.into_iter().map(from_hook).collect::<Result<_, _>>()?)
        }
        HookValue::Map(v) => toml::Value::Table(
            v.into_iter()
                .map(|(k, v)| Ok((k, from_hook(v)?)))
                .collect::<std::io::Result<_>>()?,
        ),
        _ => {
            return Err(invalid(
                "pipeline planner returned an unsupported typed value",
            ));
        }
    })
}

pub(crate) fn process(
    source: &Path,
    config: &ProcessConfig,
    output: &Path,
    cache: &Path,
    owner: Option<&Path>,
    control: &ProcessControl,
    registry: &ProcessorRegistry,
    cache_scope: &str,
) -> std::io::Result<()> {
    let declared = config
        .pipeline
        .as_ref()
        .ok_or_else(|| invalid("kind=\"pipeline\" requires [process.pipeline]"))?;
    let paths = pipeline::input_paths(declared, source, cache, owner)?;
    let output_identity =
        lunco_storage::FilePathIdentity::prepare(output).map_err(|e| invalid(e.to_string()))?;
    for path in paths.values() {
        let identity =
            lunco_storage::FilePathIdentity::prepare(path).map_err(|e| invalid(e.to_string()))?;
        if output_identity
            .overlaps(&identity)
            .map_err(|e| invalid(e.to_string()))?
        {
            return Err(invalid(
                "pipeline output overlaps an admitted source or code asset",
            ));
        }
    }
    let input_hashes = pipeline::input_digests(&paths)?;
    let declaration = pipeline::declaration_key(config)?;
    let mut effective = declared.clone();
    let mut settings = toml::Table::try_from(config).map_err(|e| invalid(e.to_string()))?;
    settings.remove("pipeline");
    let facts = HookValue::map([
        (
            "platform",
            HookValue::map([
                ("os", HookValue::str(std::env::consts::OS)),
                ("arch", HookValue::str(std::env::consts::ARCH)),
            ]),
        ),
        (
            "plan",
            to_hook(toml::Value::try_from(declared).map_err(|e| invalid(e.to_string()))?)?,
        ),
        ("settings", to_hook(toml::Value::Table(settings))?),
        (
            "inputs",
            HookValue::map(
                input_hashes
                    .iter()
                    .map(|(id, hash)| (id.clone(), HookValue::str(hash))),
            ),
        ),
        (
            "processors",
            HookValue::Array(registry.kinds().map(HookValue::str).collect()),
        ),
    ]);
    if let Some(planner) = &declared.planner {
        let code = std::fs::read_to_string(
            paths
                .get(&planner.source)
                .ok_or_else(|| invalid("missing planner source"))?,
        )?;
        let cancellation = control.clone();
        let hook = lunco_hooks_rhai::RhaiHook::compile_with(&code, &planner.entry, move |engine| {
            engine.on_progress(move |_| {
                if cancellation.check().is_err() {
                    Some("pipeline cancelled".into())
                } else {
                    None
                }
            });
        })
        .map_err(invalid)?;
        let result = hook
            .invoke(&HookInvocation::unclassified(&[facts]))
            .map_err(|e| invalid(e.to_string()))?;
        effective = from_hook(result)?
            .try_into()
            .map_err(|e| invalid(format!("pipeline planner: {e}")))?;
    }
    effective.validate()?;
    // A planning hook may change computation, never the admitted input/code set.
    if toml::to_string(&effective.sources).ok() != toml::to_string(&declared.sources).ok()
        || effective.files != declared.files
        || effective.planner != declared.planner
        || effective.artifacts != declared.artifacts
    {
        return Err(invalid(
            "pipeline planner cannot replace declared input or code ownership",
        ));
    }
    let mut identity = declaration.as_bytes().to_vec();
    identity.extend(
        toml::to_string(&input_hashes)
            .map_err(|e| invalid(e.to_string()))?
            .bytes(),
    );
    identity.extend(
        toml::to_string(&effective)
            .map_err(|e| invalid(e.to_string()))?
            .bytes(),
    );
    let key = pipeline::digest(&identity);
    crate::process::transaction(
        output,
        config,
        &key,
        &[],
        control,
        "processed_asset",
        cache_scope,
        &format!("{}/{}", config.output_root, config.output),
        |stage| {
            std::fs::create_dir_all(stage)?;
            let work = stage.join(".work");
            std::fs::create_dir(&work)?;
            let snapshot = work.join("inputs");
            std::fs::create_dir(&snapshot)?;
            let raw_parents: std::collections::BTreeSet<_> = paths
                .iter()
                .filter(|(id, _)| !declared.files.contains_key(*id))
                .map(|(_, path)| {
                    path.parent()
                        .ok_or_else(|| invalid("raw input has no parent"))
                })
                .collect::<std::io::Result<_>>()?;
            let raw_parents: BTreeMap<_, _> = raw_parents
                .into_iter()
                .enumerate()
                .map(|(index, path)| (path, index))
                .collect();
            let mut values = BTreeMap::new();
            for (id, path) in &paths {
                let copy = if let Some(relative) = declared.files.get(id) {
                    match lunco_assets_core::parse_lunco_uri(relative) {
                        Some(relative) => snapshot.join("engine").join(relative),
                        None => snapshot.join("authored").join(relative),
                    }
                } else {
                    let parent = path
                        .parent()
                        .ok_or_else(|| invalid("raw input has no parent"))?;
                    let group = raw_parents
                        .get(parent)
                        .ok_or_else(|| invalid("raw input parent is not admitted"))?;
                    snapshot.join("raw").join(group.to_string()).join(
                        path.file_name()
                            .ok_or_else(|| invalid("pipeline input has no filename"))?,
                    )
                };
                if let Some(directory) = copy.parent() {
                    std::fs::create_dir_all(directory)?;
                }
                std::fs::copy(path, &copy)?;
                values.insert(id.clone(), copy);
            }
            if pipeline::input_digests(&values)? != input_hashes {
                return Err(invalid(
                    "pipeline input changed while capturing its snapshot",
                ));
            }
            let snapshots = values.clone();
            let mut executed_steps = Vec::new();
            for step in &effective.steps {
                control.check()?;
                let input = values
                    .get(&step.input)
                    .ok_or_else(|| invalid(format!("missing step input '{}'", step.input)))?;
                let target = work.join(&step.output);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let cfg = effective.step_config(config, step)?;
                println!("  pipeline step {} ({})", step.id, step.kind);
                control.report_step(&format!("{} ({})", step.id, step.kind));
                let run = || -> std::io::Result<()> {
                    if step.kind == "external" {
                        external(input, &target, &cfg, &values, &work, control)?;
                    } else if step.kind == "shader" {
                        #[cfg(feature = "shader-processing")]
                        crate::shader::process(input, &target, &cfg, &values, control)?;
                        #[cfg(not(feature = "shader-processing"))]
                        return Err(invalid(
                            "shader step requires the native shader-processing feature",
                        ));
                    } else if step.kind == "modelica" {
                        #[cfg(feature = "modelica-processing")]
                        crate::modelica::process(input, &target, &cfg, &values, control)?;
                        #[cfg(not(feature = "modelica-processing"))]
                        return Err(invalid(
                            "modelica step requires the native modelica-processing feature",
                        ));
                    } else {
                        let processor = registry.get(&step.kind).ok_or_else(|| {
                            invalid(format!("unregistered pipeline operation '{}'", step.kind))
                        })?;
                        (processor.run)(input, &target, &cfg, control)?;
                    }
                    Ok(())
                };
                run().map_err(|e| {
                    std::io::Error::new(
                        e.kind(),
                        format!("pipeline step '{}' ({}): {e}", step.id, step.kind),
                    )
                })?;
                if !target.exists() {
                    return Err(invalid(format!("step '{}' produced no output", step.id)));
                }
                values.insert(step.id.clone(), target);
                executed_steps.push(pipeline::ExecutedStep {
                    id: step.id.clone(),
                    input: step.input.clone(),
                    config: cfg,
                });
            }
            let mut artifacts = Vec::new();
            for artifact in &effective.artifacts {
                control.check()?;
                let base = values
                    .get(&artifact.from)
                    .ok_or_else(|| invalid("artifact producer is unavailable"))?;
                let file = match &artifact.source {
                    Some(relative) => confined_file(base, Path::new(relative))?,
                    None => confined_file(
                        &work,
                        base.strip_prefix(&work)
                            .map_err(|_| invalid("artifact output escaped scratch ownership"))?,
                    )?,
                };
                validate_artifact(&file, artifact)?;
                let destination = stage.join(&artifact.path);
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&file, &destination)?;
                artifacts.push(ArtifactDigest {
                    path: artifact.path.clone(),
                    sha256: pipeline::digest(&std::fs::read(destination)?),
                });
            }
            // Sources may be edited by another operation while this worker is running.
            if pipeline::input_digests(&paths)? != input_hashes
                || pipeline::input_digests(&snapshots)? != input_hashes
            {
                return Err(invalid(
                    "pipeline inputs changed during processing; result was not published",
                ));
            }
            std::fs::remove_dir_all(&work)?;
            let receipt = PipelineReceipt {
                declaration,
                inputs: input_hashes,
                steps: executed_steps,
                artifacts,
            };
            std::fs::write(
                stage.join(pipeline::RECEIPT),
                toml::to_string(&receipt).map_err(|e| invalid(e.to_string()))?,
            )?;
            Ok(())
        },
    )
}

fn confined_file(root: &Path, relative: &Path) -> std::io::Result<PathBuf> {
    lunco_assets_core::resolve_native_asset(
        &[root.to_path_buf()],
        relative,
        Some(lunco_storage::StorageEntryKind::File),
    )?
    .map(|(_, path)| path)
    .ok_or_else(|| invalid(format!("missing artifact '{}'", relative.display())))
}

fn validate_artifact(path: &Path, artifact: &pipeline::PipelineArtifact) -> std::io::Result<()> {
    if !path.is_file() || path.metadata()?.len() == 0 {
        return Err(invalid(format!(
            "artifact '{}' is missing or empty",
            artifact.path
        )));
    }
    let dimensions = match artifact.kind.as_str() {
        "dem" => {
            let (w, h, values) = lunco_geotiff::decode_gray_f64(std::fs::File::open(path)?)
                .map_err(|e| invalid(format!("DEM artifact: {e}")))?;
            if !values.iter().all(|v| v.is_finite()) {
                return Err(invalid("DEM artifact contains unresolved samples"));
            }
            let mut decoder = tiff::decoder::Decoder::new(std::fs::File::open(path)?)
                .map_err(|e| invalid(e.to_string()))?;
            lunco_geotiff::read_geo_tags(&mut decoder).map_err(|e| invalid(e.to_string()))?;
            Some([w as u32, h as u32])
        }
        "image" => {
            let image = image::open(path).map_err(|e| invalid(e.to_string()))?;
            Some([image.width(), image.height()])
        }
        "file" => None,
        _ => return Err(invalid("unknown artifact format")),
    };
    if artifact.dimensions.is_some() && dimensions != artifact.dimensions {
        return Err(invalid(format!(
            "artifact '{}' dimensions {:?}, expected {:?}",
            artifact.path, dimensions, artifact.dimensions
        )));
    }
    Ok(())
}

fn external(
    input: &Path,
    output: &Path,
    config: &ProcessConfig,
    values: &BTreeMap<String, PathBuf>,
    work: &Path,
    control: &ProcessControl,
) -> std::io::Result<()> {
    let program = config
        .parameters
        .get("program")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| invalid("external step requires program"))?;
    let args = config
        .parameters
        .get("args")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| invalid("external step requires an argument array"))?;
    let timeout = config
        .parameters
        .get("timeout_s")
        .and_then(toml::Value::as_integer)
        .filter(|v| *v > 0)
        .ok_or_else(|| invalid("external step requires a positive timeout_s"))?;
    if config
        .parameters
        .get("tool_version")
        .and_then(toml::Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(invalid(
            "external step requires an explicit tool_version for its bake identity",
        ));
    }
    let resolve = |arg: &str| -> std::io::Result<std::ffi::OsString> {
        if arg == "{input}" {
            return Ok(input.as_os_str().to_owned());
        }
        if arg == "{output}" {
            return Ok(output.as_os_str().to_owned());
        }
        if let Some(id) = arg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            return values
                .get(id)
                .map(|p| p.as_os_str().to_owned())
                .ok_or_else(|| invalid(format!("unknown external input '{id}'")));
        }
        Ok(arg.into())
    };
    let logs = work.join(".logs");
    std::fs::create_dir_all(&logs)?;
    let log_path = logs.join(pipeline::digest(output.as_os_str().as_encoded_bytes()));
    let log = std::fs::File::create(&log_path)?;
    let mut command = Command::new(resolve(program)?);
    command.env_clear();
    if let Some(environment) = config.parameters.get("env") {
        let table = environment
            .as_table()
            .ok_or_else(|| invalid("external env must be a table"))?;
        for (key, value) in table {
            command.env(
                key,
                resolve(
                    value
                        .as_str()
                        .ok_or_else(|| invalid("external env values must be strings"))?,
                )?,
            );
        }
    }
    for arg in args {
        command.arg(resolve(
            arg.as_str()
                .ok_or_else(|| invalid("external args must be strings"))?,
        )?);
    }
    let mut child = command
        .current_dir(work)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    let start = Instant::now();
    loop {
        if control.check().is_err() || start.elapsed() > Duration::from_secs(timeout as u64) {
            let _ = child.kill();
            let _ = child.wait();
            control.check()?;
            return Err(invalid(format!(
                "external processing timed out after {timeout}s"
            )));
        }
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            let log = std::fs::read(&log_path)?;
            let tail = &log[log.len().saturating_sub(4096)..];
            return Err(invalid(format!(
                "external processing exited {status}: {}",
                String::from_utf8_lossy(tail)
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::process_asset;

    fn recipe() -> ProcessConfig {
        toml::from_str(
            r#"
kind = "pipeline"
output = "product.v1"
target_resolution = [2, 2]
color_space = "linear"
[pipeline]
artifacts = [{from="pixels", path="image.png", kind="image", dimensions=[2,2]}]
[[pipeline.steps]]
id = "pixels"
input = "source"
kind = "texture"
output = "pixels.png"
"#,
        )
        .unwrap()
    }

    #[test]
    fn recipe_transaction_customization_integrity_and_failed_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.png");
        image::GrayImage::from_raw(2, 2, vec![1, 2, 3, 4])
            .unwrap()
            .save(&source)
            .unwrap();
        let mut cfg = recipe();
        let run = |cfg: &ProcessConfig| {
            process_asset(
                &source,
                cfg,
                dir.path(),
                Some(dir.path()),
                &ProcessControl::unrestricted(),
            )
        };
        run(&cfg).unwrap();
        let product = dir.path().join("product.v1");
        assert!(pipeline::bundle_present(&product, &cfg));
        let original = std::fs::read(product.join("image.png")).unwrap();
        let stamp = std::fs::read(product.join(".bakekey")).unwrap();
        run(&cfg).unwrap();
        assert_eq!(std::fs::read(product.join(".bakekey")).unwrap(), stamp);
        std::fs::write(product.join("image.png"), b"broken").unwrap();
        assert!(!pipeline::bundle_present(&product, &cfg));
        run(&cfg).unwrap();
        assert_eq!(std::fs::read(product.join("image.png")).unwrap(), original);
        std::fs::write(
            dir.path().join("plan.rhai"),
            "fn plan(facts) { facts.plan.steps[0].kind = \"missing-operation\"; facts.plan }",
        )
        .unwrap();
        let plan = cfg.pipeline.as_mut().unwrap();
        plan.files.insert("planner".into(), "plan.rhai".into());
        plan.planner = Some(pipeline::PipelineCallable {
            source: "planner".into(),
            entry: "plan".into(),
        });
        assert!(run(&cfg).unwrap_err().to_string().contains("unregistered"));
        assert_eq!(std::fs::read(product.join("image.png")).unwrap(), original);
        assert_eq!(std::fs::read(product.join(".bakekey")).unwrap(), stamp);
        std::fs::write(
            dir.path().join("plan.rhai"),
            "fn plan(facts) { facts.plan.steps[0].color_space = \"linear\"; facts.plan }",
        )
        .unwrap();
        run(&cfg).unwrap();
        assert_ne!(std::fs::read(product.join(".bakekey")).unwrap(), stamp);
        assert!(pipeline::inputs_current(
            &product,
            &cfg,
            &source,
            dir.path(),
            Some(dir.path())
        ));
        std::fs::write(
            dir.path().join("plan.rhai"),
            "fn plan(facts) { throw \"rejected\"; }",
        )
        .unwrap();
        assert!(!pipeline::inputs_current(
            &product,
            &cfg,
            &source,
            dir.path(),
            Some(dir.path())
        ));
        assert!(run(&cfg).unwrap_err().to_string().contains("rejected"));
    }

    #[test]
    fn external_child() {
        if let Some(input) = std::env::var_os("LUNCO_PIPELINE_COPY_INPUT") {
            std::fs::copy(
                input,
                std::env::var_os("LUNCO_PIPELINE_COPY_OUTPUT").unwrap(),
            )
            .unwrap();
        }
    }

    #[test]
    fn external_process_uses_declared_inputs_and_validates_its_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.png");
        image::GrayImage::from_raw(2, 2, vec![1, 2, 3, 4])
            .unwrap()
            .save(&source)
            .unwrap();
        let mut cfg = recipe();
        let step = &mut cfg.pipeline.as_mut().unwrap().steps[0];
        step.kind = "external".into();
        step.parameters.insert(
            "program".into(),
            std::env::current_exe().unwrap().to_str().unwrap().into(),
        );
        step.parameters
            .insert("tool_version".into(), "test-copy-v1".into());
        step.parameters.insert("timeout_s".into(), 10i64.into());
        step.parameters.insert(
            "args".into(),
            toml::Value::Array(vec![
                "--exact".into(),
                "pipeline::tests::external_child".into(),
                "--nocapture".into(),
            ]),
        );
        step.parameters.insert(
            "env".into(),
            toml::Value::Table(toml::Table::from_iter([
                ("LUNCO_PIPELINE_COPY_INPUT".into(), "{input}".into()),
                ("LUNCO_PIPELINE_COPY_OUTPUT".into(), "{output}".into()),
            ])),
        );
        process_asset(
            &source,
            &cfg,
            dir.path(),
            Some(dir.path()),
            &ProcessControl::unrestricted(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&source).unwrap(),
            std::fs::read(dir.path().join("product.v1/image.png")).unwrap()
        );
    }
}
