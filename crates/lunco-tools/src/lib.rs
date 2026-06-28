//! Backend-agnostic **tool** registry.
//!
//! A *tool* is a named, reusable bundle of callable functions — a library of
//! selection / behaviour policy a scenario can call as `name::fn(...)`. The key
//! design point: a tool's IMPLEMENTATION is pluggable. It may be authored in
//! rhai source, in native Rust, or (later) in any other runtime — all are the
//! same [`Tool`] to this crate. That extensibility is why this crate is
//! deliberately dependency-free: it owns only the *abstraction* + the global
//! registry + discovery. The actual binding of a tool into a script runtime
//! lives in an adapter crate (e.g. `lunco-tools-rhai`), so non-rhai consumers
//! can still enumerate and describe tools without pulling rhai in.
//!
//! ```ignore
//! // a native Rust adapter registers a tool…
//! lunco_tools::register(Arc::new(MyNativeTool));
//! // …a rhai adapter binds every registered tool into an engine as `name::fn`.
//! ```

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

/// A named bundle of callable functions, independent of implementation language.
///
/// Implementors live in adapter crates: `RhaiTool` / `NativeRhaiTool` in
/// `lunco-tools-rhai`, a future `PythonTool`, etc. The metadata methods are
/// runtime-neutral (used for discovery here); a runtime adapter downcasts via
/// [`Tool::as_any`], or reads [`Tool::source`], to actually bind the tool.
pub trait Tool: Send + Sync + 'static {
    /// Namespace the tool is invoked under (`name::fn(...)`). Unique key.
    fn name(&self) -> &str;
    /// Implementation backend, for discovery: `"rhai"`, `"rust"`, `"python"`, …
    fn backend(&self) -> &str;
    /// Function signatures the tool exposes, as `"fn_name/arity"` strings.
    fn functions(&self) -> Vec<String>;
    /// Textual source, when the tool is source-defined (rhai/python/…); `None`
    /// for native tools. A runtime adapter can bind any source-defined tool
    /// generically by compiling this.
    fn source(&self) -> Option<&str> {
        None
    }
    /// Downcast hook so a runtime adapter can recover a concrete tool type it
    /// knows how to bind (e.g. a native tool carrying a Rust builder closure).
    fn as_any(&self) -> &dyn Any;
}

/// Discovery record for one registered tool (the shape exposed over the API).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolInfo {
    pub name: String,
    pub backend: String,
    pub functions: Vec<String>,
}

fn registry() -> &'static RwLock<HashMap<String, Arc<dyn Tool>>> {
    static R: OnceLock<RwLock<HashMap<String, Arc<dyn Tool>>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(HashMap::new()))
}

fn generation_cell() -> &'static AtomicU64 {
    static G: AtomicU64 = AtomicU64::new(0);
    &G
}

/// Register (or hot-replace) a tool by its [`Tool::name`]. Bumps the generation
/// so runtime adapters know to re-bind. Safe from anywhere (host, command, test).
pub fn register(tool: Arc<dyn Tool>) {
    registry()
        .write()
        .unwrap()
        .insert(tool.name().to_string(), tool);
    generation_cell().fetch_add(1, Ordering::Relaxed);
}

/// Monotonic registry generation — changes on every [`register`]. A runtime
/// adapter compares this against its last-bound value to detect new/changed
/// tools (hot-reload).
pub fn generation() -> u64 {
    generation_cell().load(Ordering::Relaxed)
}

/// Every registered tool (clones the `Arc`s; cheap). Order unspecified.
pub fn all() -> Vec<Arc<dyn Tool>> {
    registry().read().unwrap().values().cloned().collect()
}

/// A registered tool by name, if any.
pub fn get(name: &str) -> Option<Arc<dyn Tool>> {
    registry().read().unwrap().get(name).cloned()
}

/// Sorted names of every registered tool.
pub fn names() -> Vec<String> {
    let mut v: Vec<String> = registry().read().unwrap().keys().cloned().collect();
    v.sort();
    v
}

/// Discovery index (name + backend + function sigs) for every tool, sorted by
/// name — the data behind a `ListTools`/`ListToolLibraries` API query.
pub fn index() -> Vec<ToolInfo> {
    let mut v: Vec<ToolInfo> = registry()
        .read()
        .unwrap()
        .values()
        .map(|t| ToolInfo {
            name: t.name().to_string(),
            backend: t.backend().to_string(),
            functions: t.functions(),
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// The textual source of a registered tool, when it is source-defined.
pub fn source(name: &str) -> Option<String> {
    registry()
        .read()
        .unwrap()
        .get(name)
        .and_then(|t| t.source().map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy;
    impl Tool for Dummy {
        fn name(&self) -> &str {
            "dummy"
        }
        fn backend(&self) -> &str {
            "test"
        }
        fn functions(&self) -> Vec<String> {
            vec!["f/1".into()]
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[test]
    fn register_then_discover() {
        let gen0 = generation();
        register(Arc::new(Dummy));
        assert!(generation() > gen0, "register must bump the generation");
        assert!(names().contains(&"dummy".to_string()));
        let info = index().into_iter().find(|i| i.name == "dummy").unwrap();
        assert_eq!(info.backend, "test");
        assert_eq!(info.functions, vec!["f/1".to_string()]);
        // native tool → no source
        assert_eq!(source("dummy"), None);
        // downcast hook recovers the concrete type
        assert!(get("dummy").unwrap().as_any().is::<Dummy>());
    }
}
