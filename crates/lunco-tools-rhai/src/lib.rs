//! rhai adapter for the runtime-agnostic [`lunco_tools`] registry.
//!
//! Provides the two concrete [`Tool`] impls scenarios use today —
//! [`RhaiTool`] (rhai source) and [`NativeRhaiTool`] (native Rust functions) —
//! and [`bind_registered_tools`], which binds every registered tool into a rhai [`Engine`] as
//! a **static module** so it is callable as `name::fn(...)` from anywhere,
//! including inside `on_tick` (script-level `import` aliases are invisible to
//! rhai's pure hook functions; static modules are not).
//!
//! Extensibility: a tool authored in another runtime (Python, …) is exposed to
//! rhai as a [`NativeRhaiTool`] whose builder closure registers bridge functions
//! — so `bind_registered_tools` only ever needs to handle "source-defined" or "native", and
//! every backend funnels through one of those two paths.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::RwLock;

use lunco_tools::Tool;
use rhai::{Engine, EvalAltResult, Module, ModuleResolver, Position, Scope, Shared};

/// A tool authored in **rhai source**. Its functions become a compiled rhai
/// module, so they run with full rhai semantics (closures, the prelude, host
/// verbs) — exactly like the scenario itself.
pub struct RhaiTool {
    name: String,
    source: String,
}

impl RhaiTool {
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: source.into(),
        }
    }
}

impl Tool for RhaiTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn backend(&self) -> &str {
        "rhai"
    }
    fn functions(&self) -> Vec<String> {
        // Syntax-only parse with the production Rhai limits. A bare engine's
        // smaller default expression-depth cap makes valid authored tools
        // disappear from ListToolLibraries even though runtime binding accepts
        // them.
        let mut engine = Engine::new();
        lunco_hooks_rhai::rhai_limits::apply(&mut engine);
        engine
            .compile(&self.source)
            .ok()
            .map(|ast| {
                ast.iter_functions()
                    .map(|f| format!("{}/{}", f.name, f.params.len()))
                    .collect()
            })
            .unwrap_or_default()
    }
    fn source(&self) -> Option<&str> {
        Some(&self.source)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Builder for a [`NativeRhaiTool`]'s module: given the engine, return a rhai
/// [`Module`] populated with the tool's functions (typically via
/// [`Module::set_native_fn`]). This is the universal escape hatch — a Rust tool
/// registers native closures here; a Python/other-runtime tool registers bridge
/// functions that dispatch into its own interpreter.
pub type ModuleBuilder = dyn Fn(&Engine) -> Result<Module, String> + Send + Sync;

/// A tool implemented **natively** (Rust closures, or a bridge to another
/// runtime). It carries no rhai source; its functions are produced by a builder
/// closure when bound.
pub struct NativeRhaiTool {
    name: String,
    backend: String,
    functions: Vec<String>,
    build: Arc<ModuleBuilder>,
}

impl NativeRhaiTool {
    /// `functions` are advisory signature strings for discovery (`"name/arity"`).
    /// `backend` labels the implementation (e.g. `"rust"`, `"python"`).
    pub fn new(
        name: impl Into<String>,
        backend: impl Into<String>,
        functions: Vec<String>,
        build: impl Fn(&Engine) -> Result<Module, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            backend: backend.into(),
            functions,
            build: Arc::new(build),
        }
    }
}

impl Tool for NativeRhaiTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn backend(&self) -> &str {
        &self.backend
    }
    fn functions(&self) -> Vec<String> {
        self.functions.clone()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Build the rhai module for one tool: run a native tool's builder, else compile
/// any source-defined tool.
///
/// Three outcomes, deliberately distinct:
/// - `Ok(Some(module))` — bound, register it.
/// - `Ok(None)` — **the tool has no rhai surface, and is not supposed to have one.**
/// - `Err(e)` — a tool that SHOULD have bound failed to; a real defect.
///
/// `Ok(None)` exists because the registry holds more than rhai tools. A closure
/// tool (`lunco_tools::register_closure_tool`, e.g. `science::take_photo`) is
/// invoked by an engine action through `world.trigger`, and never had a rhai
/// module to build. Reporting that as an error logged
/// `[rhai] tool 'science::take_photo' failed to bind` at ERROR on every twin open
/// — a permanent false alarm in a codebase whose lesson smoke test is "zero
/// `[rhai]` lines", i.e. exactly the noise that trains people to ignore the log.
fn build_module(tool: &Arc<dyn Tool>, engine: &Engine) -> Result<Option<Module>, String> {
    if let Some(native) = tool.as_any().downcast_ref::<NativeRhaiTool>() {
        return (native.build)(engine).map(Some);
    }
    if let Some(src) = tool.source() {
        let ast = engine
            .compile(src)
            .map_err(|e| format!("compile error: {e}"))?;
        return Module::eval_ast_as_new(Scope::new(), &ast, engine)
            .map(Some)
            .map_err(|e| format!("build error: {e}"));
    }
    // Not a rhai tool at all — nothing to bind, nothing wrong.
    Ok(None)
}

/// The result of checking one registered tool against a configured Rhai
/// engine. `callable` is based on the actual module build, not on discovery
/// metadata alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolBindingReport {
    pub functions: Vec<String>,
    pub callable: bool,
    pub diagnostics: Vec<String>,
}

/// Inspect a registered tool with the same module builder used by runtime
/// binding. Non-Rhai tools remain discoverable but are reported as not
/// callable through Rhai.
pub fn inspect_tool_with_engine(tool: &Arc<dyn Tool>, engine: &Engine) -> ToolBindingReport {
    let functions = tool.functions();
    match build_module(tool, engine) {
        Ok(Some(_)) => ToolBindingReport {
            functions,
            callable: true,
            diagnostics: Vec::new(),
        },
        Ok(None) => ToolBindingReport {
            functions,
            callable: false,
            diagnostics: vec!["tool has no Rhai module surface".into()],
        },
        Err(error) => ToolBindingReport {
            functions,
            callable: false,
            diagnostics: vec![error],
        },
    }
}

/// Resolve registered tools through Rhai's ordinary `import` mechanism.
///
/// Direct calls such as `assembly_edit::inspect(...)` remain available for
/// existing scripts. A tool that depends on another tool can additionally use
/// `import "assembly_edit" as assembly_edit;`; the dependency is compiled only
/// when Rhai resolves that import. The resolver deliberately accepts registry
/// names only, so it cannot turn tool imports into filesystem reads.
#[derive(Clone, Default)]
pub struct ToolModuleResolver {
    cache: Arc<RwLock<HashMap<String, (String, Shared<Module>)>>>,
    resolving: Arc<RwLock<HashSet<String>>>,
}

impl ToolModuleResolver {
    pub fn new() -> Self {
        Self::default()
    }

    fn tool_name(path: &str) -> &str {
        path.strip_suffix(".rhai").unwrap_or(path)
    }
}

impl ModuleResolver for ToolModuleResolver {
    fn resolve(
        &self,
        engine: &Engine,
        _source: Option<&str>,
        path: &str,
        pos: Position,
    ) -> Result<Shared<Module>, Box<EvalAltResult>> {
        let name = Self::tool_name(path);
        let Some(tool) = lunco_tools::get(name) else {
            return Err(Box::new(EvalAltResult::ErrorModuleNotFound(
                path.into(),
                pos,
            )));
        };
        let source = tool.source().map(str::to_string);

        if let Some(source) = source.as_deref() {
            if let Some((cached_source, module)) = self
                .cache
                .read()
                .ok()
                .and_then(|cache| cache.get(name).cloned())
            {
                if cached_source == source {
                    return Ok(module);
                }
            }
        }

        if let Ok(mut resolving) = self.resolving.write() {
            if !resolving.insert(name.to_string()) {
                return Err(Box::new(EvalAltResult::ErrorInModule(
                    name.into(),
                    Box::new(EvalAltResult::ErrorRuntime(
                        "tool import cycle detected".into(),
                        pos,
                    )),
                    pos,
                )));
            }
        }

        let result = build_module(&tool, engine)
            .map_err(|error| {
                Box::new(EvalAltResult::ErrorInModule(
                    name.into(),
                    Box::new(EvalAltResult::ErrorRuntime(error.into(), pos)),
                    pos,
                ))
            })
            .and_then(|module| {
                module.ok_or_else(|| Box::new(EvalAltResult::ErrorModuleNotFound(path.into(), pos)))
            });

        if let Ok(mut resolving) = self.resolving.write() {
            resolving.remove(name);
        }
        let module = result?;
        let shared: Shared<Module> = module.into();
        if let Some(source) = source {
            if let Ok(mut cache) = self.cache.write() {
                cache.insert(name.to_string(), (source, shared.clone()));
            }
        }
        Ok(shared)
    }
}

/// Validate a source-defined tool before it is persisted or published.
///
/// Validation uses the same bounded Rhai parser and registry-backed import
/// mechanism as runtime tool binding. It is intentionally a preflight seam;
/// the production world engine still owns the actual world/prelude bindings.
pub fn validate_rhai_tool(name: &str, source: &str) -> Result<Vec<String>, String> {
    let mut engine = Engine::new();
    lunco_hooks_rhai::rhai_limits::apply(&mut engine);
    engine.set_module_resolver(ToolModuleResolver::new());
    validate_rhai_tool_with_engine(name, source, &engine)
}

/// Validate a source-defined tool against an already configured runtime
/// engine. The caller supplies the production prelude, host verbs, asset
/// resolver, and current registered tools; this function only checks the
/// candidate module without publishing it.
pub fn validate_rhai_tool_with_engine(
    name: &str,
    source: &str,
    engine: &Engine,
) -> Result<Vec<String>, String> {
    let tool = RhaiTool::new(name, source);
    let functions = tool.functions();
    let tool: Arc<dyn Tool> = Arc::new(tool);
    let module = build_module(&tool, &engine)?
        .ok_or_else(|| format!("tool '{name}' does not expose a Rhai module"))?;
    let _ = module;
    Ok(functions)
}

/// Bind every tool in the global registry into `engine` as a static module
/// (`name::fn(...)`). Returns `(tool_name, error)` for any tool that failed to
/// bind — one bad tool never blocks the others. Call AFTER the prelude global
/// module is registered, so tools can resolve prelude helpers + host verbs.
#[must_use]
pub fn bind_registered_tools(engine: &mut Engine) -> Vec<(String, String)> {
    let mut errors = Vec::new();
    for tool in lunco_tools::all() {
        match build_module(&tool, engine) {
            Ok(Some(module)) => {
                engine.register_static_module(tool.name(), module.into());
            }
            // No rhai surface by design (e.g. a behaviour-tree closure tool) — skip.
            Ok(None) => {}
            Err(e) => errors.push((tool.name().to_string(), e)),
        }
    }
    errors
}

/// Convenience: register a rhai-source tool into the global registry.
pub fn register_rhai_tool(name: &str, source: &str) {
    lunco_tools::register(Arc::new(RhaiTool::new(name, source)));
}

/// Convenience: register a native (Rust) tool whose `build` closure populates a
/// rhai module with native functions.
pub fn register_native_tool(
    name: &str,
    functions: Vec<String>,
    build: impl Fn(&Engine) -> Result<Module, String> + Send + Sync + 'static,
) {
    lunco_tools::register(Arc::new(NativeRhaiTool::new(
        name, "rust", functions, build,
    )));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rhai_and_native_tools_bind_and_run() {
        // A rhai-source tool that calls a native tool — proves both backends
        // bind into one engine and interoperate across static-module namespaces.
        register_native_tool("nat", vec!["square/1".into()], |_engine| {
            let mut m = Module::new();
            m.set_native_fn("square", |x: i64| Ok(x * x));
            Ok(m)
        });
        register_rhai_tool("lib", "fn quad(x) { nat::square(x) * 2 }");

        let mut engine = Engine::new();
        let errs = bind_registered_tools(&mut engine);
        assert!(errs.is_empty(), "binding errors: {errs:?}");

        // native directly
        let n: i64 = engine.eval("nat::square(5)").unwrap();
        assert_eq!(n, 25);
        // rhai tool calling the native tool
        let q: i64 = engine.eval("lib::quad(3)").unwrap();
        assert_eq!(q, 18);

        // discovery sees both, with backends
        let backends: std::collections::HashMap<_, _> = lunco_tools::index()
            .into_iter()
            .map(|i| (i.name, i.backend))
            .collect();
        assert_eq!(backends.get("nat").map(String::as_str), Some("rust"));
        assert_eq!(backends.get("lib").map(String::as_str), Some("rhai"));
    }

    /// A tool with no Rhai surface is skipped, not reported as a failure.
    ///
    /// Engine-only tools are invoked through typed dispatch, never as a Rhai
    /// module. Treating them as bind failures would put a permanent binding
    /// error in every session's log.
    #[test]
    fn a_tool_with_no_rhai_surface_is_skipped_not_an_error() {
        struct BtOnlyTool;
        impl Tool for BtOnlyTool {
            fn name(&self) -> &str {
                "bt_only"
            }
            fn backend(&self) -> &str {
                "rust"
            }
            fn functions(&self) -> Vec<String> {
                vec!["fire/0".into()]
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
        }
        lunco_tools::register(Arc::new(BtOnlyTool));

        let mut engine = Engine::new();
        let errs = bind_registered_tools(&mut engine);
        assert!(
            !errs.iter().any(|(n, _)| n == "bt_only"),
            "an engine-only tool must not be reported as a bind failure: {errs:?}"
        );
        // It is still discoverable — skipping the rhai binding is not deregistering.
        assert!(lunco_tools::index().iter().any(|i| i.name == "bt_only"));
    }

    #[test]
    fn tool_imports_are_loaded_on_demand_through_rhai() {
        register_rhai_tool("lazy_child", "fn value() { 21 }");
        register_rhai_tool(
            "lazy_parent",
            r#"import "lazy_child" as child; fn answer() { child::value() * 2 }"#,
        );

        let mut engine = Engine::new();
        lunco_hooks_rhai::rhai_limits::apply(&mut engine);
        engine.set_module_resolver(ToolModuleResolver::new());
        let answer: i64 = engine
            .eval(r#"import "lazy_parent" as parent; parent::answer()"#)
            .unwrap();
        assert_eq!(answer, 42);
    }

    #[test]
    fn tool_validation_rejects_missing_dynamic_dependencies() {
        let error = validate_rhai_tool(
            "invalid_tool",
            r#"import "missing_tool" as missing; fn run() { missing::run() }"#,
        )
        .unwrap_err();
        assert!(error.contains("missing_tool"), "unexpected error: {error}");
    }
}
